//! Global keyboard shortcuts (`RegisterHotKey`), on a thread of their own:
//! registered with no window, so `WM_HOTKEY` arrives in that thread's
//! message queue. No keyboard hook: Windows hands over only the
//! combinations registered, and refuses one another program holds.
//!
//! The owner gives the whole set (`set`); the thread unregisters the old
//! ones and registers the new, and keeps whether each worked (`results`).
//! A press calls `on_press` with its id, on the hotkey thread.

use std::collections::HashMap;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_APP, WM_HOTKEY, WM_QUIT,
};

/// One shortcut: an id of the owner's, `RegisterHotKey`'s modifiers and
/// virtual key.
pub type Hotkey = (i32, u32, u32);

/// What registering each id gave: `Err` with Windows' reason (a
/// combination another program holds: "Hot key is already registered").
pub type Results = HashMap<i32, Result<(), String>>;

pub struct Hotkeys {
    thread: u32,
    tx: Sender<Vec<Hotkey>>,
    results: Arc<Mutex<Results>>,
}

impl Hotkeys {
    /// Starts the thread; `on_press(id)` is called there for each press.
    pub fn start(on_press: impl Fn(i32) + Send + 'static) -> io::Result<Hotkeys> {
        let (tx, rx) = mpsc::channel::<Vec<Hotkey>>();
        let (ready_tx, ready_rx) = mpsc::channel::<u32>();
        let results: Arc<Mutex<Results>> = Arc::default();
        let shared = Arc::clone(&results);
        std::thread::Builder::new()
            .name("nativeterm-hotkeys".into())
            .spawn(move || run(rx, ready_tx, shared, on_press))?;
        let thread = ready_rx.recv().map_err(|_| io::Error::other("the hotkey thread didn't start"))?;
        Ok(Hotkeys { thread, tx, results })
    }

    /// Replaces the registered shortcuts with `keys` (none: all go).
    pub fn set(&self, keys: Vec<Hotkey>) {
        if self.tx.send(keys).is_ok() {
            // SAFETY: a thread id and plain numbers; a thread gone makes
            // the call fail, nothing else.
            let _ = unsafe { PostThreadMessageW(self.thread, WM_APP, Default::default(), Default::default()) };
        }
    }

    /// Whether each id's registration worked (after the last `set` was
    /// handled).
    pub fn results(&self) -> Results {
        self.results.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        // SAFETY: as in `set`; the thread unregisters everything on WM_QUIT.
        let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, Default::default(), Default::default()) };
    }
}

fn run(rx: Receiver<Vec<Hotkey>>, ready: Sender<u32>, results: Arc<Mutex<Results>>, on_press: impl Fn(i32)) {
    let mut msg = MSG::default();
    // SAFETY: `msg` is ours; PeekMessage with PM_NOREMOVE only makes the
    // thread's message queue exist before its id is handed out (a message
    // posted to a thread without one is lost).
    unsafe {
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
    }
    // SAFETY: no parameters.
    let _ = ready.send(unsafe { GetCurrentThreadId() });
    let mut registered: Vec<i32> = Vec::new();
    loop {
        // SAFETY: `msg` is ours; no window filter (thread messages too).
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if got.0 <= 0 {
            break; // WM_QUIT (0) or an error (-1)
        }
        match msg.message {
            WM_HOTKEY => on_press(msg.wParam.0 as i32),
            WM_APP => {
                // only the newest set counts
                let Some(keys) = rx.try_iter().last() else { continue };
                for id in registered.drain(..) {
                    // SAFETY: registered earlier on this thread, no window.
                    let _ = unsafe { UnregisterHotKey(None, id) };
                }
                let mut out = Results::new();
                for (id, mods, vk) in keys {
                    // SAFETY: plain numbers, no window (the thread's queue).
                    let result = unsafe { RegisterHotKey(None, id, HOT_KEY_MODIFIERS(mods), vk) };
                    if result.is_ok() {
                        registered.push(id);
                    }
                    out.insert(id, result.map_err(|e| e.message()));
                }
                *results.lock().unwrap_or_else(|e| e.into_inner()) = out;
            }
            _ => {}
        }
    }
    for id in registered {
        // SAFETY: as above.
        let _ = unsafe { UnregisterHotKey(None, id) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn settled(h: &Hotkeys, n: usize) -> Results {
        let start = Instant::now();
        loop {
            let r = h.results();
            if r.len() == n || start.elapsed() > Duration::from_secs(5) {
                return r;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Registered, refused when the combination is already held (a second
    /// registration of the same one), freed with an empty set. A rare
    /// combination (Ctrl+Alt+Shift+F24) nothing else holds.
    #[test]
    fn registered_refused_and_freed() {
        const F24: u32 = 0x87;
        const MODS: u32 = 1 | 2 | 4 | 0x4000;
        let first = Hotkeys::start(|_| {}).unwrap();
        first.set(vec![(1, MODS, F24)]);
        assert_eq!(settled(&first, 1).get(&1), Some(&Ok(())));
        let second = Hotkeys::start(|_| {}).unwrap();
        second.set(vec![(7, MODS, F24)]);
        assert!(matches!(settled(&second, 1).get(&7), Some(Err(_))), "held by the first");
        first.set(Vec::new());
        assert!(settled(&first, 0).is_empty());
        second.set(vec![(7, MODS, F24)]);
        let start = Instant::now();
        while second.results().get(&7) != Some(&Ok(())) && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(second.results().get(&7), Some(&Ok(())), "free again");
    }
}
