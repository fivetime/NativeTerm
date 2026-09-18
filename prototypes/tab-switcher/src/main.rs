//! Prototype: take over Ctrl+Tab in the test Windows Terminal and show a
//! grid of tab thumbnails (see "Taking over Ctrl+Tab" in ARCHITECTURE.md).
//!
//! tab-switcher [seconds]   run the switcher (default 300 s); hold Ctrl and
//!                          press Tab / Shift+Tab in the test Terminal,
//!                          release Ctrl to switch, Esc to cancel
//! tab-switcher selftest    the same, driven by injected keys — only ever
//!                          while the test Terminal is the foreground window
//!
//! Measures the time spent in the keyboard hook, the capture cost, the
//! time from Ctrl+Tab to the grid, and from Ctrl release to the switch.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use uiautomation::patterns::UISelectionItemPattern;
use uiautomation::types::ControlType;
use uiautomation::{UIAutomation, UIElement};
use windows::core::{w, PWSTR};
use windows::Win32::Foundation::{CloseHandle, COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush, DeleteDC,
    DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, InvalidateRect, ReleaseDC, SelectObject, SetBkMode,
    SetBrushOrgEx, SetStretchBltMode, SetTextColor, StretchBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, HALFTONE,
    HBITMAP, HDC, HGDIOBJ, OUT_DEFAULT_PRECIS, PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_LCONTROL, VK_LEFT, VK_MENU, VK_RCONTROL, VK_RIGHT, VK_SHIFT,
    VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumWindows, GetClassNameW,
    GetForegroundWindow, GetMessageW, GetWindowRect, GetWindowThreadProcessId, PostMessageW, PostQuitMessage,
    RegisterClassW, SetForegroundWindow, SetTimer, SetWindowPos, SetWindowsHookExW, ShowWindow, TranslateMessage,
    UnhookWindowsHookEx, HC_ACTION, HWND_TOPMOST, KBDLLHOOKSTRUCT, MSG, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE,
    WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_KEYUP, WM_PAINT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const TEST_TERMINAL_DIR: &str = r"C:\MyProjects\RustProjects\terminal-1.26.2581.0";
const WT_WINDOW_CLASS: &str = "CASCADIA_HOSTING_WINDOW_CLASS";

const WM_OPEN: u32 = WM_APP + 1;
const WM_MOVE: u32 = WM_APP + 2;
const WM_COMMIT: u32 = WM_APP + 3;
const WM_CANCEL: u32 = WM_APP + 4;
const WM_LISTED: u32 = WM_APP + 5;
const WM_DONE: u32 = WM_APP + 6;

const THUMB_W: i32 = 320;
const TITLE_H: i32 = 30;
const GAP: i32 = 14;
const COLUMNS: usize = 4;

static OVERLAY: AtomicIsize = AtomicIsize::new(0);
static OPEN: AtomicBool = AtomicBool::new(false);
static TAB_SWALLOWED: AtomicBool = AtomicBool::new(false);
static HOOK_CALLS: AtomicU64 = AtomicU64::new(0);
static HOOK_NANOS: AtomicU64 = AtomicU64::new(0);
static HOOK_MAX_NANOS: AtomicU64 = AtomicU64::new(0);
static FOREGROUND_CACHE: Mutex<Option<(isize, bool)>> = Mutex::new(None);
/// (window, tab name) → thumbnail (an HBITMAP; GDI handles are process-wide).
/// (window, tab name) → (HBITMAP, height).
type Thumbs = HashMap<(isize, String), (isize, i32)>;
static THUMBS: Mutex<Option<Thumbs>> = Mutex::new(None);
static LISTED: Mutex<Option<Listed>> = Mutex::new(None);
static WORKER: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();
static TIMINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn note(line: String) {
    eprintln!("{line}");
    TIMINGS.lock().unwrap().push(line);
}

struct Listed {
    window: isize,
    names: Vec<String>,
    selected: usize,
    list_ms: u128,
    capture_ms: Option<u128>,
}

enum Job {
    List { window: isize, asked: Instant },
    Select { window: isize, name: String, index: usize, asked: Instant },
}

fn process_image(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        result.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Only the test Terminal; never the Store Terminal the user works in.
fn is_test_terminal(hwnd: HWND) -> bool {
    let mut class = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
    if String::from_utf16_lossy(&class[..len]) != WT_WINDOW_CLASS {
        return false;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    process_image(pid).is_some_and(|p| p.to_lowercase().starts_with(&format!("{}\\", TEST_TERMINAL_DIR.to_lowercase())))
}

/// The foreground window if it is the test Terminal (cached per window, the
/// hook must stay quick).
fn foreground_test_terminal() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    let key = hwnd.0 as isize;
    let mut cache = FOREGROUND_CACHE.lock().unwrap();
    let verdict = match *cache {
        Some((k, v)) if k == key => v,
        _ => {
            let v = is_test_terminal(hwnd);
            *cache = Some((key, v));
            v
        }
    };
    verdict.then_some(hwnd)
}

fn post(message: u32, wparam: usize) {
    let overlay = HWND(OVERLAY.load(Ordering::SeqCst) as *mut _);
    unsafe {
        let _ = PostMessageW(Some(overlay), message, WPARAM(wparam), LPARAM(0));
    }
}

fn down(key: VIRTUAL_KEY) -> bool {
    let state = unsafe { GetAsyncKeyState(i32::from(key.0)) };
    state < 0
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let started = Instant::now();
    let mut swallow = false;
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let key_down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let key_up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
        let vk = VIRTUAL_KEY(info.vkCode as u16);
        let open = OPEN.load(Ordering::SeqCst);
        if vk == VK_TAB && key_down && down(VK_CONTROL) && !down(VK_MENU) {
            if open {
                post(WM_MOVE, usize::from(down(VK_SHIFT)));
                swallow = true;
            } else if foreground_test_terminal().is_some() {
                OPEN.store(true, Ordering::SeqCst);
                post(WM_OPEN, usize::from(down(VK_SHIFT)));
                swallow = true;
            }
            if swallow {
                TAB_SWALLOWED.store(true, Ordering::SeqCst);
            }
        } else if vk == VK_TAB && key_up && TAB_SWALLOWED.swap(false, Ordering::SeqCst) {
            swallow = true;
        } else if open {
            if vk == VK_ESCAPE && key_down {
                post(WM_CANCEL, 0);
                swallow = true;
            } else if [VK_LEFT, VK_UP].contains(&vk) && key_down {
                post(WM_MOVE, 1);
                swallow = true;
            } else if [VK_RIGHT, VK_DOWN].contains(&vk) && key_down {
                post(WM_MOVE, 0);
                swallow = true;
            } else if [VK_CONTROL, VK_LCONTROL, VK_RCONTROL].contains(&vk) && key_up {
                // the Ctrl key-up still goes to Terminal, which saw the key-down
                post(WM_COMMIT, 0);
            }
        }
        if swallow || vk == VK_TAB {
            let nanos = started.elapsed().as_nanos() as u64;
            HOOK_CALLS.fetch_add(1, Ordering::Relaxed);
            HOOK_NANOS.fetch_add(nanos, Ordering::Relaxed);
            HOOK_MAX_NANOS.fetch_max(nanos, Ordering::Relaxed);
        }
    }
    if swallow {
        return LRESULT(1);
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// The window scaled to THUMB_W wide: PrintWindow draws it even while it is
/// covered by other windows.
fn capture(window: HWND) -> Option<(HBITMAP, i32, Duration)> {
    let started = Instant::now();
    unsafe {
        let mut rect = RECT::default();
        GetWindowRect(window, &mut rect).ok()?;
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        if w <= 0 || h <= 0 {
            return None;
        }
        let screen = GetDC(None);
        let full_dc = CreateCompatibleDC(Some(screen));
        let full = CreateCompatibleBitmap(screen, w, h);
        let old_full = SelectObject(full_dc, HGDIOBJ(full.0));
        let printed = PrintWindow(window, full_dc, PRINT_WINDOW_FLAGS(2)).as_bool(); // PW_RENDERFULLCONTENT
        let th = h * THUMB_W / w;
        let small_dc = CreateCompatibleDC(Some(screen));
        let small = CreateCompatibleBitmap(screen, THUMB_W, th);
        let old_small = SelectObject(small_dc, HGDIOBJ(small.0));
        SetStretchBltMode(small_dc, HALFTONE);
        let _ = SetBrushOrgEx(small_dc, 0, 0, None);
        let _ = StretchBlt(small_dc, 0, 0, THUMB_W, th, Some(full_dc), 0, 0, w, h, SRCCOPY);
        SelectObject(small_dc, old_small);
        SelectObject(full_dc, old_full);
        let _ = DeleteDC(small_dc);
        let _ = DeleteDC(full_dc);
        let _ = DeleteObject(HGDIOBJ(full.0));
        ReleaseDC(None, screen);
        if !printed {
            let _ = DeleteObject(HGDIOBJ(small.0));
            return None;
        }
        Some((small, th, started.elapsed()))
    }
}

fn store_thumb(window: isize, name: &str, thumb: HBITMAP, height: i32) {
    let mut thumbs = THUMBS.lock().unwrap();
    let map = thumbs.get_or_insert_with(HashMap::new);
    if let Some((old, _)) = map.insert((window, name.to_string()), (thumb.0 as isize, height)) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(old as *mut _));
        }
    }
}

// FindAll(Descendants) doesn't cross into Terminal's XAML island, so walk
// the control view (tree order is the tab strip order).
fn tab_items(automation: &UIAutomation, window: &UIElement) -> uiautomation::Result<Vec<UIElement>> {
    fn visit(walker: &uiautomation::UITreeWalker, e: &UIElement, out: &mut Vec<UIElement>) {
        let mut child = walker.get_first_child(e).ok();
        while let Some(c) = child {
            if c.get_control_type().map(|t| t == ControlType::TabItem).unwrap_or(false) {
                out.push(c.clone());
            } else {
                visit(walker, &c, out);
            }
            child = walker.get_next_sibling(&c).ok();
        }
    }
    let walker = automation.get_control_view_walker()?;
    let mut out = Vec::new();
    visit(&walker, window, &mut out);
    Ok(out)
}

fn worker(jobs: mpsc::Receiver<Job>) {
    let automation = UIAutomation::new().expect("UI Automation");
    for job in jobs {
        match job {
            Job::List { window, asked } => {
                let hwnd = HWND(window as *mut _);
                let started = Instant::now();
                let tabs = automation
                    .element_from_handle(hwnd.into())
                    .and_then(|w| tab_items(&automation, &w))
                    .unwrap_or_default();
                let names: Vec<String> = tabs.iter().map(|t| t.get_name().unwrap_or_default()).collect();
                let selected = tabs
                    .iter()
                    .position(|t| {
                        t.get_pattern::<UISelectionItemPattern>().and_then(|p| p.is_selected()).unwrap_or(false)
                    })
                    .unwrap_or(0);
                let list_ms = started.elapsed().as_millis();
                // the tab in front can be captured now
                let capture_ms = capture(hwnd).map(|(thumb, h, took)| {
                    if let Some(name) = names.get(selected) {
                        store_thumb(window, name, thumb, h);
                    }
                    took.as_millis()
                });
                note(format!(
                    "listed {} tabs in {list_ms} ms, captured the selected one in {capture_ms:?} ms (asked {} ms ago)",
                    names.len(),
                    asked.elapsed().as_millis()
                ));
                *LISTED.lock().unwrap() = Some(Listed { window, names, selected, list_ms, capture_ms });
                post(WM_LISTED, 0);
            }
            Job::Select { window, name, index, asked } => {
                let hwnd = HWND(window as *mut _);
                let started = Instant::now();
                let tabs = automation
                    .element_from_handle(hwnd.into())
                    .and_then(|w| tab_items(&automation, &w))
                    .unwrap_or_default();
                let target = tabs
                    .get(index)
                    .filter(|t| t.get_name().unwrap_or_default() == name)
                    .or_else(|| tabs.iter().find(|t| t.get_name().unwrap_or_default() == name));
                let selected = target
                    .map(|t| t.get_pattern::<UISelectionItemPattern>().and_then(|p| p.select()).is_ok())
                    .unwrap_or(false);
                note(format!(
                    "selected {name:?}: {selected} in {} ms ({} ms after Ctrl was released)",
                    started.elapsed().as_millis(),
                    asked.elapsed().as_millis()
                ));
                // the newly selected tab is visible: keep its picture
                std::thread::sleep(Duration::from_millis(300));
                if let Some((thumb, h, took)) = capture(hwnd) {
                    store_thumb(window, &name, thumb, h);
                    note(format!("captured {name:?} in {} ms", took.as_millis()));
                }
                post(WM_DONE, 0);
            }
        }
    }
}

fn send_job(job: Job) {
    if let Some(worker) = WORKER.get() {
        let _ = worker.lock().unwrap().send(job);
    }
}

/// The switcher's state, owned by the UI thread.
struct Grid {
    window: isize,
    names: Vec<String>,
    current: usize,
    backwards_first: bool,
    opened: Option<Instant>,
}

thread_local! {
    static GRID: std::cell::RefCell<Grid> = const { std::cell::RefCell::new(Grid {
        window: 0, names: Vec::new(), current: 0, backwards_first: false, opened: None,
    }) };
}

fn layout(count: usize) -> (i32, i32, i32, i32) {
    let columns = count.clamp(1, COLUMNS) as i32;
    let rows = count.div_ceil(COLUMNS).max(1) as i32;
    let tile_h = THUMB_W * 9 / 16 + TITLE_H;
    let width = columns * THUMB_W + (columns + 1) * GAP;
    let height = rows * tile_h + (rows + 1) * GAP;
    (columns, tile_h, width, height)
}

fn show_grid(overlay: HWND) {
    GRID.with(|g| {
        let g = g.borrow();
        let (_, _, width, height) = layout(g.names.len());
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(HWND(g.window as *mut _), &mut rect);
            let x = rect.left + ((rect.right - rect.left) - width) / 2;
            let y = rect.top + ((rect.bottom - rect.top) - height) / 2;
            let _ = SetWindowPos(overlay, Some(HWND_TOPMOST), x, y, width, height, SWP_NOACTIVATE);
            let _ = ShowWindow(overlay, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(overlay), None, false);
        }
    });
}

fn hide(overlay: HWND) {
    unsafe {
        let _ = ShowWindow(overlay, SW_HIDE);
    }
    OPEN.store(false, Ordering::SeqCst);
}

fn paint(overlay: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(overlay, &mut ps);
        let mut client = RECT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(overlay, &mut client);
        // draw off-screen, then copy: no flicker while cycling
        let mem = CreateCompatibleDC(Some(hdc));
        let bitmap = CreateCompatibleBitmap(hdc, client.right, client.bottom);
        let old = SelectObject(mem, HGDIOBJ(bitmap.0));
        let background = CreateSolidBrush(COLORREF(0x0020_2020));
        FillRect(mem, &client, background);
        let _ = DeleteObject(HGDIOBJ(background.0));
        GRID.with(|g| draw_tiles(mem, &g.borrow()));
        let _ = BitBlt(hdc, 0, 0, client.right, client.bottom, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem);
        let _ = EndPaint(overlay, &ps);
    }
}

unsafe fn draw_tiles(dc: HDC, g: &Grid) {
    let (columns, tile_h, _, _) = layout(g.names.len());
    let font = unsafe {
        CreateFontW(
            -18, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY, 0, w!("Segoe UI"),
        )
    };
    unsafe {
        let old_font = SelectObject(dc, HGDIOBJ(font.0));
        SetBkMode(dc, TRANSPARENT);
        let thumbs = THUMBS.lock().unwrap();
        for (i, name) in g.names.iter().enumerate() {
            let col = (i as i32) % columns;
            let row = (i as i32) / columns;
            let x = GAP + col * (THUMB_W + GAP);
            let y = GAP + row * (tile_h + GAP);
            let picture = RECT { left: x, top: y, right: x + THUMB_W, bottom: y + tile_h - TITLE_H };
            if i == g.current {
                let accent = CreateSolidBrush(COLORREF(0x00D0_7A2A)); // BGR
                let frame = RECT { left: x - 5, top: y - 5, right: x + THUMB_W + 5, bottom: y + tile_h + 5 };
                FillRect(dc, &frame, accent);
                let _ = DeleteObject(HGDIOBJ(accent.0));
                let inner = CreateSolidBrush(COLORREF(0x0020_2020));
                let hole = RECT { left: x - 2, top: y - 2, right: x + THUMB_W + 2, bottom: y + tile_h + 2 };
                FillRect(dc, &hole, inner);
                let _ = DeleteObject(HGDIOBJ(inner.0));
            }
            match thumbs.as_ref().and_then(|m| m.get(&(g.window, name.clone()))) {
                Some(&(thumb, height)) => {
                    let src = CreateCompatibleDC(Some(dc));
                    let old = SelectObject(src, HGDIOBJ(thumb as *mut _));
                    let shown = (picture.bottom - picture.top).min(height);
                    let _ = BitBlt(dc, x, y, THUMB_W, shown, Some(src), 0, 0, SRCCOPY);
                    SelectObject(src, old);
                    let _ = DeleteDC(src);
                }
                None => {
                    let empty = CreateSolidBrush(COLORREF(0x0030_3030));
                    FillRect(dc, &picture, empty);
                    let _ = DeleteObject(HGDIOBJ(empty.0));
                    SetTextColor(dc, COLORREF(0x0090_9090));
                    let mut text: Vec<u16> = "no picture yet".encode_utf16().collect();
                    let mut r = picture;
                    DrawTextW(dc, &mut text, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                }
            }
            SetTextColor(dc, COLORREF(0x00F0_F0F0));
            let mut title: Vec<u16> = name.encode_utf16().collect();
            let mut r = RECT { left: x + 4, top: picture.bottom, right: x + THUMB_W - 4, bottom: y + tile_h };
            DrawTextW(dc, &mut title, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
        }
        drop(thumbs);
        SelectObject(dc, old_font);
        let _ = DeleteObject(HGDIOBJ(font.0));
    }
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_OPEN => {
            let Some(window) = foreground_test_terminal() else {
                OPEN.store(false, Ordering::SeqCst);
                return LRESULT(0);
            };
            GRID.with(|g| {
                let mut g = g.borrow_mut();
                g.window = window.0 as isize;
                g.backwards_first = wparam.0 == 1;
                g.opened = Some(Instant::now());
            });
            send_job(Job::List { window: window.0 as isize, asked: Instant::now() });
            LRESULT(0)
        }
        WM_LISTED => {
            let Some(listed) = LISTED.lock().unwrap().take() else { return LRESULT(0) };
            if !OPEN.load(Ordering::SeqCst) {
                return LRESULT(0); // Ctrl was released before the list arrived
            }
            GRID.with(|g| {
                let mut g = g.borrow_mut();
                let count = listed.names.len().max(1);
                g.current = if g.backwards_first {
                    (listed.selected + count - 1) % count
                } else {
                    (listed.selected + 1) % count
                };
                g.window = listed.window;
                g.names = listed.names;
                if let Some(opened) = g.opened {
                    note(format!(
                        "grid shown {} ms after Ctrl+Tab (list {} ms, capture {:?} ms)",
                        opened.elapsed().as_millis(),
                        listed.list_ms,
                        listed.capture_ms
                    ));
                }
            });
            show_grid(hwnd);
            LRESULT(0)
        }
        WM_MOVE => {
            GRID.with(|g| {
                let mut g = g.borrow_mut();
                let count = g.names.len();
                if count > 0 {
                    g.current = if wparam.0 == 1 { (g.current + count - 1) % count } else { (g.current + 1) % count };
                }
            });
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_COMMIT => {
            let chosen = GRID.with(|g| {
                let g = g.borrow();
                g.names.get(g.current).map(|n| (g.window, n.clone(), g.current))
            });
            hide(hwnd);
            if let Some((window, name, index)) = chosen {
                send_job(Job::Select { window, name, index, asked: Instant::now() });
            }
            LRESULT(0)
        }
        WM_CANCEL => {
            hide(hwnd);
            note("cancelled".into());
            LRESULT(0)
        }
        WM_DONE => LRESULT(0),
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_TIMER => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn send_key(key: VIRTUAL_KEY, up: bool) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
}

fn test_terminal_window() -> Option<HWND> {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> windows::core::BOOL {
        if is_test_terminal(hwnd) {
            unsafe { *(found.0 as *mut isize) = hwnd.0 as isize };
            return false.into();
        }
        true.into()
    }
    let mut found: isize = 0;
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut found as *mut isize as isize));
    }
    (found != 0).then_some(HWND(found as *mut _))
}

/// Injected keys only while the test Terminal is in front; otherwise stop
/// (they would land in whatever window the user has in front).
fn selftest() {
    std::thread::sleep(Duration::from_millis(800));
    let Some(terminal) = foreground_test_terminal().or_else(test_terminal_window) else {
        note("selftest: the test Terminal isn't running".into());
        post(WM_TIMER, 0);
        return;
    };
    unsafe {
        let _ = SetForegroundWindow(terminal);
    }
    std::thread::sleep(Duration::from_millis(400));
    let safe = || unsafe { GetForegroundWindow() } == terminal;
    let step = |key: VIRTUAL_KEY, up: bool| -> bool {
        if !safe() {
            send_key(VK_CONTROL, true);
            note("selftest: the test Terminal isn't in front any more; stopped, no keys sent".into());
            return false;
        }
        send_key(key, up);
        std::thread::sleep(Duration::from_millis(250));
        true
    };
    let rounds: [&[(VIRTUAL_KEY, bool)]; 3] = [
        // Ctrl+Tab, Tab, release: two tabs further
        &[(VK_CONTROL, false), (VK_TAB, false), (VK_TAB, true), (VK_TAB, false), (VK_TAB, true), (VK_CONTROL, true)],
        // Ctrl+Tab, Esc: nothing changes
        &[(VK_CONTROL, false), (VK_TAB, false), (VK_TAB, true), (VK_ESCAPE, false), (VK_ESCAPE, true), (VK_CONTROL, true)],
        // Ctrl+Shift+Tab, release: one tab back
        &[
            (VK_CONTROL, false),
            (VK_SHIFT, false),
            (VK_TAB, false),
            (VK_TAB, true),
            (VK_SHIFT, true),
            (VK_CONTROL, true),
        ],
    ];
    if !safe() {
        note("selftest: couldn't bring the test Terminal to the front; no keys sent".into());
        post(WM_TIMER, 0);
        return;
    }
    // TAB_SWITCHER_HOLD_MS: keep Ctrl down that long in the last round, so
    // the grid can be looked at
    let hold: u64 = std::env::var("TAB_SWITCHER_HOLD_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for (n, round) in rounds.iter().enumerate() {
        note(format!("selftest round {}", n + 1));
        for &(key, up) in round.iter() {
            if n + 1 == rounds.len() && key == VK_CONTROL && up {
                std::thread::sleep(Duration::from_millis(hold));
            }
            if !step(key, up) {
                post(WM_TIMER, 0);
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(1200));
    }
    post(WM_TIMER, 0);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let selftest_mode = args.first().map(String::as_str) == Some("selftest");
    let seconds: u32 = args.first().and_then(|a| a.parse().ok()).unwrap_or(300);
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let (jobs, receiver) = mpsc::channel();
    let _ = WORKER.set(Mutex::new(jobs));
    std::thread::spawn(move || worker(receiver));

    unsafe {
        let instance = GetModuleHandleW(None).expect("module");
        let class = WNDCLASSW {
            lpfnWndProc: Some(overlay_proc),
            hInstance: instance.into(),
            lpszClassName: w!("NativeTermTabSwitcherPrototype"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let overlay = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("NativeTermTabSwitcherPrototype"),
            w!("tab switcher"),
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .expect("overlay window");
        OVERLAY.store(overlay.0 as isize, Ordering::SeqCst);
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(instance.into()), 0).expect("hook");
        if selftest_mode {
            std::thread::spawn(selftest);
            SetTimer(Some(overlay), 1, 60_000, None); // safety net
        } else {
            eprintln!("hold Ctrl and press Tab in the test Terminal; ends in {seconds} s");
            SetTimer(Some(overlay), 1, seconds * 1000, None);
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(hook);
    }
    let calls = HOOK_CALLS.load(Ordering::Relaxed).max(1);
    println!("--- summary");
    for line in TIMINGS.lock().unwrap().iter() {
        println!("{line}");
    }
    println!(
        "keyboard hook, Tab-related calls: {calls}, average {:.1} µs, max {:.1} µs",
        HOOK_NANOS.load(Ordering::Relaxed) as f64 / calls as f64 / 1000.0,
        HOOK_MAX_NANOS.load(Ordering::Relaxed) as f64 / 1000.0
    );
}
