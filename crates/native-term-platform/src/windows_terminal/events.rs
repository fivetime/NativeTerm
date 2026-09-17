//! Change notifications from Windows Terminal, so NativeTerm rescans only
//! when something happened instead of polling.
//!
//! - Win32 WinEvents (out of context, never blocked by a hung Terminal):
//!   Terminal windows shown or hidden, and foreground changes.
//! - UIA events on each responsive Terminal window: a tab was selected, or
//!   the tree changed (tabs opened, closed, moved). Registered on their own
//!   thread; if that thread hangs on a Terminal that stopped responding, it
//!   is abandoned and a new one takes over.
//!
//! Title changes are deliberately not watched: tabs of AI tools and shells
//! retitle constantly, and every notification costs a rescan.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationEventHandler, IUIAutomationEventHandler_Impl,
    IUIAutomationStructureChangedEventHandler, IUIAutomationStructureChangedEventHandler_Impl, SetWinEventHook,
    StructureChangeType, UnhookWinEvent, HWINEVENTHOOK, TreeScope_Subtree, UIA_EVENT_ID,
    UIA_SelectionItem_ElementSelectedEventId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetClassNameW, GetMessageW, GetWindowThreadProcessId, PostThreadMessageW, TranslateMessage,
    EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, EVENT_OBJECT_SHOW, EVENT_SYSTEM_FOREGROUND, MSG, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WM_QUIT,
};
use windows_core::{implement, Ref};

use super::install::Install;
use super::window::{self, terminal_windows, WINDOW_CLASS};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Change {
    /// A Terminal window appeared or went away.
    Windows,
    /// A Terminal window (or something else) became the foreground window.
    Foreground,
    /// A tab was selected, opened, closed or moved.
    Tabs,
}

pub type Notify = Arc<dyn Fn(Change) + Send + Sync>;

/// How many notifications of each kind were delivered (diagnostics).
#[derive(Default)]
pub struct Counts {
    pub windows: AtomicU64,
    pub foreground: AtomicU64,
    pub selected: AtomicU64,
    pub structure: AtomicU64,
}

/// A UIA registration taking longer than this means the thread is stuck.
const STUCK: Duration = Duration::from_secs(5);

pub struct Watcher {
    counts: Arc<Counts>,
    win_event_thread: u32,
    uia: Arc<Mutex<UiaThread>>,
    stopped: Arc<AtomicBool>,
}

struct UiaThread {
    requests: Sender<()>,
    /// When the current sync started, if one is running.
    busy_since: Arc<Mutex<Option<Instant>>>,
    abandoned: usize,
}

impl Watcher {
    pub fn start(install: Install, notify: Notify) -> Watcher {
        let counts = Arc::new(Counts::default());
        let stopped = Arc::new(AtomicBool::new(false));
        let uia = Arc::new(Mutex::new(UiaThread::spawn(install.clone(), Arc::clone(&notify), Arc::clone(&counts))));

        // window changes also re-sync the UIA subscriptions
        let sync_uia = {
            let uia = Arc::clone(&uia);
            let install = install.clone();
            let notify = Arc::clone(&notify);
            let counts = Arc::clone(&counts);
            move || UiaThread::request(&uia, &install, &notify, &counts)
        };
        let (thread_id_tx, thread_id_rx) = mpsc::channel();
        let hook_counts = Arc::clone(&counts);
        std::thread::Builder::new()
            .name("terminal-win-events".into())
            .spawn(move || win_event_loop(install, notify, hook_counts, sync_uia, thread_id_tx))
            .expect("spawn WinEvent thread");
        let win_event_thread = thread_id_rx.recv().unwrap_or(0);
        Watcher { counts, win_event_thread, uia, stopped }
    }

    pub fn counts(&self) -> &Counts {
        &self.counts
    }

    /// UIA threads left behind because a Terminal hung during registration.
    pub fn abandoned(&self) -> usize {
        self.uia.lock().unwrap_or_else(|e| e.into_inner()).abandoned
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if self.win_event_thread != 0 {
            unsafe {
                let _ = PostThreadMessageW(self.win_event_thread, WM_QUIT, Default::default(), Default::default());
            }
        }
        // the UIA thread ends when its request channel is dropped with us
    }
}

impl UiaThread {
    fn spawn(install: Install, notify: Notify, counts: Arc<Counts>) -> UiaThread {
        let (requests, incoming) = mpsc::channel();
        let busy_since = Arc::new(Mutex::new(None));
        let busy = Arc::clone(&busy_since);
        std::thread::Builder::new()
            .name("terminal-uia-events".into())
            .spawn(move || uia_event_loop(install, notify, counts, incoming, busy))
            .expect("spawn UIA event thread");
        let thread = UiaThread { requests, busy_since, abandoned: 0 };
        let _ = thread.requests.send(());
        thread
    }

    /// Ask for a re-sync; replace the thread if it is stuck.
    fn request(this: &Mutex<UiaThread>, install: &Install, notify: &Notify, counts: &Arc<Counts>) {
        let mut thread = this.lock().unwrap_or_else(|e| e.into_inner());
        let stuck = thread.busy_since.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|t| t.elapsed() > STUCK);
        if stuck {
            let abandoned = thread.abandoned + 1;
            *thread = UiaThread::spawn(install.clone(), Arc::clone(notify), Arc::clone(counts));
            thread.abandoned = abandoned;
            return;
        }
        let _ = thread.requests.send(());
    }
}

// ---- WinEvents ----------------------------------------------------------

type HookCallback = Box<dyn FnMut(u32, HWND)>;

thread_local! {
    static HOOK_CALLBACK: RefCell<Option<HookCallback>> = const { RefCell::new(None) };
}

unsafe extern "system" fn on_win_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // OBJID_WINDOW, the window itself
    if id_object != 0 || id_child != 0 || hwnd.is_invalid() {
        return;
    }
    HOOK_CALLBACK.with(|callback| {
        if let Some(callback) = callback.borrow_mut().as_mut() {
            callback(event, hwnd);
        }
    });
}

fn is_terminal_class(hwnd: HWND) -> bool {
    let mut class = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut class) };
    n > 0 && String::from_utf16_lossy(&class[..n as usize]) == WINDOW_CLASS
}

fn win_event_loop(
    install: Install,
    notify: Notify,
    counts: Arc<Counts>,
    sync_uia: impl Fn() + 'static,
    thread_id: Sender<u32>,
) {
    let mut known: HashSet<isize> = terminal_windows(&install).iter().map(|w| w.handle).collect();
    let mut ours_by_pid: HashMap<u32, bool> = HashMap::new();
    let callback = move |event: u32, hwnd: HWND| {
        let handle = hwnd.0 as isize;
        let mut ours = |hwnd: HWND| {
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            *ours_by_pid
                .entry(pid)
                .or_insert_with(|| window::process_image(pid).is_some_and(|image| install.owns_image(&image)))
        };
        match event {
            EVENT_OBJECT_SHOW if !known.contains(&handle) && is_terminal_class(hwnd) && ours(hwnd) => {
                known.insert(handle);
                counts.windows.fetch_add(1, Ordering::Relaxed);
                sync_uia();
                notify(Change::Windows);
            }
            EVENT_OBJECT_HIDE | EVENT_OBJECT_DESTROY if known.remove(&handle) => {
                counts.windows.fetch_add(1, Ordering::Relaxed);
                sync_uia();
                notify(Change::Windows);
            }
            EVENT_SYSTEM_FOREGROUND => {
                counts.foreground.fetch_add(1, Ordering::Relaxed);
                notify(Change::Foreground);
            }
            _ => {}
        }
    };
    HOOK_CALLBACK.with(|c| *c.borrow_mut() = Some(Box::new(callback)));
    let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
    let hooks = unsafe {
        [
            SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_win_event), 0, 0, flags),
            SetWinEventHook(EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, None, Some(on_win_event), 0, 0, flags),
        ]
    };
    let _ = thread_id.send(unsafe { GetCurrentThreadId() });
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    for hook in hooks {
        unsafe {
            let _ = UnhookWinEvent(hook);
        }
    }
}

// ---- UIA events ----------------------------------------------------------

#[implement(IUIAutomationEventHandler)]
struct OnSelected(Notify, Arc<Counts>);

impl IUIAutomationEventHandler_Impl for OnSelected_Impl {
    fn HandleAutomationEvent(&self, _sender: Ref<IUIAutomationElement>, _id: UIA_EVENT_ID) -> windows_core::Result<()> {
        self.1.selected.fetch_add(1, Ordering::Relaxed);
        (self.0)(Change::Tabs);
        Ok(())
    }
}

#[implement(IUIAutomationStructureChangedEventHandler)]
struct OnStructure(Notify, Arc<Counts>);

impl IUIAutomationStructureChangedEventHandler_Impl for OnStructure_Impl {
    fn HandleStructureChangedEvent(
        &self,
        _sender: Ref<IUIAutomationElement>,
        _change: StructureChangeType,
        _runtime_id: *const windows::Win32::System::Com::SAFEARRAY,
    ) -> windows_core::Result<()> {
        self.1.structure.fetch_add(1, Ordering::Relaxed);
        (self.0)(Change::Tabs);
        Ok(())
    }
}

fn uia_event_loop(
    install: Install,
    notify: Notify,
    counts: Arc<Counts>,
    requests: Receiver<()>,
    busy: Arc<Mutex<Option<Instant>>>,
) {
    let automation: IUIAutomation = unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            return;
        }
        match CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) {
            Ok(a) => a,
            Err(_) => return,
        }
    };
    let selected: IUIAutomationEventHandler = OnSelected(Arc::clone(&notify), Arc::clone(&counts)).into();
    let structure: IUIAutomationStructureChangedEventHandler = OnStructure(notify, counts).into();
    let mut subscribed: HashMap<isize, IUIAutomationElement> = HashMap::new();
    while requests.recv().is_ok() {
        // coalesce bursts
        while requests.try_recv().is_ok() {}
        *busy.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        let current = terminal_windows(&install);
        let live: HashSet<isize> = current.iter().map(|w| w.handle).collect();
        subscribed.retain(|handle, element| {
            if live.contains(handle) {
                return true;
            }
            unsafe {
                let _ = automation.RemoveAutomationEventHandler(UIA_SelectionItem_ElementSelectedEventId, &*element, &selected);
                let _ = automation.RemoveStructureChangedEventHandler(&*element, &structure);
            }
            false
        });
        let new: Vec<_> = current.iter().filter(|w| w.responsive && !subscribed.contains_key(&w.handle)).collect();
        for w in new {
            let Ok(element) = (unsafe { automation.ElementFromHandle(window::hwnd(w.handle)) }) else { continue };
            let ok = unsafe {
                automation
                    .AddAutomationEventHandler(
                        UIA_SelectionItem_ElementSelectedEventId,
                        &element,
                        TreeScope_Subtree,
                        None,
                        &selected,
                    )
                    .is_ok()
                    & automation.AddStructureChangedEventHandler(&element, TreeScope_Subtree, None, &structure).is_ok()
            };
            if ok {
                subscribed.insert(w.handle, element);
            }
        }
        *busy.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    unsafe {
        let _ = automation.RemoveAllEventHandlers();
    }
}
