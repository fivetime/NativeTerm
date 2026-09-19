//! Prototype: NativeTerm's own right-click menu on its Windows Terminal tabs.
//!
//! Only windows of the test Terminal are touched (see TEST_TERMINAL_DIR).
//!
//! menu-hook run <secs> <prefix> [--auto-dismiss]
//!     Install low-level mouse and keyboard hooks. A right-click on a
//!     NativeTerm tab is swallowed and answered with our own popup;
//!     Terminal's own tab menu is blocked there by design. Every other
//!     right-click passes through.
//!
//!     Tab identity: a tab whose title starts with <prefix> becomes tracked
//!     by its UIA RuntimeId and stays tracked when its title changes (split
//!     panes show the focused pane's title, users rename tabs) until it
//!     disappears. A tracked tab whose selected view has more than one
//!     terminal control is "mixed" (the user split it). Tab rectangles come
//!     from a UIA cache refreshed on a background thread; the hooks do no
//!     UIA work.
//!
//!     The popup is custom-drawn in the style of a WinUI menu flyout, follows
//!     the Terminal's theme, and never activates.
//! menu-hook click-tab <title> | click <index> [--shift]
//!     Test driver: right-click a tab (by title, or by position in the first
//!     test window) with SendInput, restore the cursor, report open menus.
//! menu-hook menus [--close]
//!     Test driver: report open Terminal menus; --close sends Escape.

mod theme;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uiautomation::patterns::UISelectionItemPattern;
use uiautomation::types::{ControlType, TreeScope};
use uiautomation::{UIAutomation, UIElement};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, TrackMouseEvent, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT, MOUSE_EVENT_FLAGS, TME_LEAVE, TRACKMOUSEEVENT, VIRTUAL_KEY,
    VK_DOWN, VK_ESCAPE, VK_RETURN, VK_SHIFT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const WT_WINDOW_CLASS: &str = "CASCADIA_HOSTING_WINDOW_CLASS";
const POPUP_CLASS: PCWSTR = w!("NativeTermMenuPopup");
const WM_SHOW_MENU: u32 = WM_APP + 1;
const WM_CLOSE_MENU: u32 = WM_APP + 2;
const WM_MENU_KEY: u32 = WM_APP + 3;
const TIMER_END: usize = 1;
const TIMER_DISMISS: usize = 2;
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Test Terminal install; set NT_PROBE_WT_DIR to override, or to "*" for all.
/// Never hook or click the Store Terminal the user works in.
const TEST_TERMINAL_DIR: &str = r"C:\MyProjects\RustProjects\terminal-1.26.2581.0";

enum Item {
    Action(i32, char, String),
    Header(String),
    Separator,
}

// Glyphs from Segoe Fluent Icons (Windows 11); Windows 10 has the same code
// points in Segoe MDL2 Assets.
fn items_for(tab: &TabEntry) -> Vec<Item> {
    let action = |id, glyph, text: &str| Item::Action(id, glyph, text.to_string());
    let mut items = Vec::new();
    if tab.mixed {
        items.push(Item::Header(format!("{} · 此标签还有其他窗格", tab.label)));
        items.push(Item::Separator);
    } else if tab.title != tab.label {
        items.push(Item::Header(format!("{} · 当前标题“{}”", tab.label, tab.title)));
        items.push(Item::Separator);
    }
    items.push(action(1, '\u{E72C}', "重新连接"));
    items.push(action(2, '\u{F5ED}', "克隆会话"));
    items.push(Item::Separator);
    if tab.mixed {
        // ending the shim closes only its own pane
        items.push(action(7, '\u{E711}', "关闭此会话（保留其他窗格）"));
    }
    items.push(action(3, '\u{E89F}', "关闭其他 NativeTerm 标签"));
    items.push(action(4, '\u{E711}', "关闭已断开的标签"));
    items.push(action(5, '\u{E72A}', "关闭右侧标签"));
    items.push(Item::Separator);
    items.push(action(6, '\u{E724}', "发送命令…"));
    items
}

// Layout in effective pixels, after WinUI's MenuFlyout (measured against
// Windows Terminal's tab menu).
const PAD_V: f32 = 4.0;
const ROW_H: f32 = 32.0;
const SEP_H: f32 = 9.0;
const ROW_INSET: f32 = 4.0;
const ICON_X: f32 = 16.0;
const TEXT_X: f32 = 44.0;
const PAD_RIGHT: f32 = 28.0;
const MIN_W: f32 = 164.0;

#[derive(Clone)]
struct TabEntry {
    root: isize,
    rect: RECT,
    /// current tab title
    title: String,
    /// the NativeTerm session title it was claimed with
    label: String,
    mixed: bool,
    index: usize,
}

struct Cache {
    windows: Vec<isize>,
    /// tracked (NativeTerm) tabs only
    tabs: Vec<TabEntry>,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache { windows: Vec::new(), tabs: Vec::new() });
static PENDING: Mutex<Option<(TabEntry, POINT)>> = Mutex::new(None);
static OWNER: AtomicIsize = AtomicIsize::new(0);
static MENU_OPEN: AtomicBool = AtomicBool::new(false);
static MENU_RECT: [AtomicI32; 4] = [AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0)];
static SWALLOWED_DOWN: AtomicBool = AtomicBool::new(false);
static SWALLOWED_KEY: AtomicU32 = AtomicU32::new(0);
static AUTO_DISMISS: AtomicBool = AtomicBool::new(false);
static HOOK_CALLS: AtomicU32 = AtomicU32::new(0);
static HOOK_MAX_MICROS: AtomicU64 = AtomicU64::new(0);
static HOOK_TOTAL_MICROS: AtomicU64 = AtomicU64::new(0);
static PASSED: AtomicU32 = AtomicU32::new(0);
static SWALLOWED: AtomicU32 = AtomicU32::new(0);
static INJECTED: AtomicU32 = AtomicU32::new(0);

struct Menu {
    popup: HWND,
    tab: TabEntry,
    items: Vec<Item>,
    hover: Option<usize>,
    scale: f32,
    look: theme::Look,
    size: SIZE,
    text_font: HFONT,
    icon_font: HFONT,
    // (top, bottom) per item, in physical pixels
    rows: Vec<(i32, i32)>,
}

thread_local! {
    static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
}

fn now() -> String {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("{}.{:03}", d.as_secs() % 100_000, d.subsec_millis())
}

fn contains(r: &RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

fn owner() -> HWND {
    HWND(OWNER.load(Ordering::SeqCst) as *mut _)
}

fn post(msg: u32, wparam: usize) {
    let _ = unsafe { PostMessageW(Some(owner()), msg, WPARAM(wparam), LPARAM(0)) };
}

fn inside_menu(p: POINT) -> bool {
    let r = RECT {
        left: MENU_RECT[0].load(Ordering::SeqCst),
        top: MENU_RECT[1].load(Ordering::SeqCst),
        right: MENU_RECT[2].load(Ordering::SeqCst),
        bottom: MENU_RECT[3].load(Ordering::SeqCst),
    };
    contains(&r, p)
}

// Runs inside the hook: no UIA, no blocking, fail safe (pass through) on doubt.
fn hit_test(pt: POINT) -> Option<TabEntry> {
    let root = unsafe { GetAncestor(WindowFromPoint(pt), GA_ROOT) }.0 as isize;
    let cache = CACHE.try_lock().ok()?;
    if !cache.windows.contains(&root) {
        return None;
    }
    cache.tabs.iter().find(|t| t.root == root && contains(&t.rect, pt)).cloned()
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let msg = wparam.0 as u32;
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let open = MENU_OPEN.load(Ordering::SeqCst);
        let injected = info.flags & LLMHF_INJECTED != 0;
        match msg {
            WM_RBUTTONDOWN | WM_RBUTTONUP => {
                let started = Instant::now();
                let swallow = if msg == WM_RBUTTONDOWN {
                    // Terminal's own menu is blocked on NativeTerm tabs by design
                    // (no replay item, no Shift pass-through).
                    let swallow = if open && inside_menu(info.pt) {
                        true // right-click inside our popup: ignore
                    } else if let Some(tab) = hit_test(info.pt) {
                        if let Ok(mut p) = PENDING.try_lock() {
                            *p = Some((tab, info.pt));
                        }
                        post(WM_SHOW_MENU, 0); // replaces an open popup
                        true
                    } else {
                        if open {
                            post(WM_CLOSE_MENU, 0);
                        }
                        false
                    };
                    SWALLOWED_DOWN.store(swallow, Ordering::SeqCst);
                    swallow
                } else {
                    // swallow the up only if we swallowed its down
                    SWALLOWED_DOWN.swap(false, Ordering::SeqCst)
                };
                let micros = started.elapsed().as_micros() as u64;
                HOOK_CALLS.fetch_add(1, Ordering::Relaxed);
                HOOK_TOTAL_MICROS.fetch_add(micros, Ordering::Relaxed);
                HOOK_MAX_MICROS.fetch_max(micros, Ordering::Relaxed);
                if injected {
                    INJECTED.fetch_add(1, Ordering::Relaxed);
                }
                if swallow {
                    SWALLOWED.fetch_add(1, Ordering::Relaxed);
                    return LRESULT(1);
                }
                PASSED.fetch_add(1, Ordering::Relaxed);
            }
            WM_LBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEHWHEEL
                if open && !inside_menu(info.pt) =>
            {
                // outside click closes the popup; the click itself goes on
                post(WM_CLOSE_MENU, 0);
            }
            _ => {}
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let msg = wparam.0 as u32;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let vk = VIRTUAL_KEY(info.vkCode as u16);
        if MENU_OPEN.load(Ordering::SeqCst) {
            if [VK_UP, VK_DOWN, VK_RETURN, VK_ESCAPE].contains(&vk) {
                if down {
                    SWALLOWED_KEY.store(info.vkCode, Ordering::SeqCst);
                    post(WM_MENU_KEY, info.vkCode as usize);
                }
                return LRESULT(1);
            }
            if down {
                post(WM_CLOSE_MENU, 0);
            }
        } else if !down && SWALLOWED_KEY.load(Ordering::SeqCst) == info.vkCode {
            // the key-up of a key that closed the menu
            SWALLOWED_KEY.store(0, Ordering::SeqCst);
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn is_test_terminal(window: &UIElement) -> bool {
    let dir = std::env::var("NT_PROBE_WT_DIR").unwrap_or_else(|_| TEST_TERMINAL_DIR.into());
    if dir == "*" {
        return true;
    }
    let Ok(handle) = window.get_native_window_handle() else { return false };
    theme::process_image(handle.into())
        .is_some_and(|p| p.to_string_lossy().to_lowercase().starts_with(&format!("{}\\", dir.to_lowercase())))
}

fn terminal_windows(automation: &UIAutomation) -> uiautomation::Result<Vec<UIElement>> {
    let root = automation.get_root_element()?;
    let all = root.find_all(TreeScope::Children, &automation.create_true_condition()?)?;
    Ok(all
        .into_iter()
        .filter(|w| w.get_classname().map(|c| c == WT_WINDOW_CLASS).unwrap_or(false))
        .filter(is_test_terminal)
        .collect())
}

/// Realized tab items in strip order, the live titles of the selected tab's
/// panes (`TermControl` HelpText; other tabs' controls aren't in the tree),
/// and the tab list element.
type Scan = (Vec<UIElement>, Vec<String>, Option<UIElement>);

fn scan(automation: &UIAutomation, window: &UIElement) -> uiautomation::Result<Scan> {
    fn visit(walker: &uiautomation::UITreeWalker, e: &UIElement, out: &mut Scan) {
        let mut child = walker.get_first_child(e).ok();
        while let Some(c) = child {
            let ty = c.get_control_type().ok();
            if ty == Some(ControlType::TabItem) {
                out.0.push(c.clone());
            } else {
                if ty == Some(ControlType::List) && out.2.is_none() {
                    out.2 = Some(c.clone());
                }
                if c.get_classname().map(|n| n == "TermControl").unwrap_or(false) {
                    out.1.push(c.get_help_text().unwrap_or_default());
                }
                visit(walker, &c, out);
            }
            child = walker.get_next_sibling(&c).ok();
        }
    }
    let walker = automation.get_control_view_walker()?;
    let mut out: Scan = (Vec::new(), Vec::new(), None);
    visit(&walker, window, &mut out);
    Ok(out)
}

fn rect_of(e: &UIElement) -> Option<RECT> {
    let r = e.get_bounding_rectangle().ok()?;
    let rect = RECT { left: r.get_left(), top: r.get_top(), right: r.get_right(), bottom: r.get_bottom() };
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

fn is_selected(tab: &UIElement) -> bool {
    tab.get_pattern::<UISelectionItemPattern>().and_then(|p| p.is_selected()).unwrap_or(false)
}

#[derive(Clone)]
struct Tracked {
    label: String,
    mixed: bool,
}

/// Every tab name of a window in strip order, through ItemContainerPattern
/// on the tab list: includes tabs virtualized away, whose names are readable
/// without realizing them.
fn all_tab_names(list: &UIElement) -> Option<Vec<String>> {
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationItemContainerPattern, UIA_ItemContainerPatternId, UIA_PROPERTY_ID,
    };
    let raw: &IUIAutomationElement = list.as_ref();
    let container: IUIAutomationItemContainerPattern =
        unsafe { raw.GetCurrentPatternAs(UIA_ItemContainerPatternId) }.ok()?;
    let empty = VARIANT::default();
    let mut names = Vec::new();
    let mut prev: Option<IUIAutomationElement> = None;
    while let Ok(item) = unsafe { container.FindItemByProperty(prev.as_ref(), UIA_PROPERTY_ID(0), &empty) } {
        names.push(unsafe { item.CurrentName() }.map(|s| s.to_string()).unwrap_or_default());
        prev = Some(item);
    }
    Some(names)
}

/// Carry claims from the previous snapshot of a window's full tab list to
/// the current one, instead of relying on RuntimeIds (which change when
/// containers are recycled). Order-preserving name matches first (LCS), then
/// unique names that moved, then in-place changes when the list length is
/// unchanged (rename, a split tab's focus moving to another pane).
fn carry(prev: &[(String, Option<Tracked>)], cur: &[String]) -> Vec<Option<Tracked>> {
    let (n, m) = (prev.len(), cur.len());
    let mut out: Vec<Option<Tracked>> = vec![None; m];
    let mut used_prev = vec![false; n];
    let mut done_cur = vec![false; m];
    let mut dp = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if prev[i].0 == cur[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if prev[i].0 == cur[j] {
            out[j] = prev[i].1.clone();
            used_prev[i] = true;
            done_cur[j] = true;
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    for j in 0..m {
        if done_cur[j] {
            continue;
        }
        if let Some(i) = (0..n).find(|&i| !used_prev[i] && prev[i].0 == cur[j]) {
            out[j] = prev[i].1.clone();
            used_prev[i] = true;
            done_cur[j] = true;
        }
    }
    if n == m {
        for j in 0..m {
            if !done_cur[j] && !used_prev[j] {
                out[j] = prev[j].1.clone();
                used_prev[j] = true;
            }
        }
    }
    out
}

fn refresher(prefix: String) {
    let automation = UIAutomation::new().expect("UIA");
    // per window: full tab list with claims, from the previous round
    let mut snapshots: HashMap<isize, Vec<(String, Option<Tracked>)>> = HashMap::new();
    let mut last = String::new();
    loop {
        let started = Instant::now();
        let mut windows_seen = Vec::new();
        let mut tabs = Vec::new();
        let mut complete = true;
        match terminal_windows(&automation) {
            Ok(windows) => {
                for w in windows {
                    let Ok(handle) = w.get_native_window_handle() else { continue };
                    let hwnd: HWND = handle.into();
                    let key = hwnd.0 as isize;
                    windows_seen.push(key);
                    let Ok((realized, panes, list)) = scan(&automation, &w) else {
                        complete = false;
                        continue;
                    };
                    let realized_names: Vec<String> =
                        realized.iter().map(|t| t.get_name().unwrap_or_default()).collect();
                    let names = list.as_ref().and_then(all_tab_names).unwrap_or_else(|| realized_names.clone());
                    let mut claims = carry(snapshots.get(&key).map(Vec::as_slice).unwrap_or(&[]), &names);
                    // rule 1 for every tab, realized or not
                    for (k, name) in names.iter().enumerate() {
                        if name.starts_with(&prefix) {
                            let mixed = claims[k].as_ref().map(|c| c.mixed).unwrap_or(false);
                            claims[k] = Some(Tracked { label: name.clone(), mixed });
                        }
                    }
                    // realized tabs are in strip order: map them onto the full list
                    let mut k = 0;
                    for (t, title) in realized.iter().zip(&realized_names) {
                        while k < names.len() && &names[k] != title {
                            k += 1;
                        }
                        if k >= names.len() {
                            break;
                        }
                        if is_selected(t) {
                            // rule 2: a session pane's HelpText, even with another pane focused
                            if claims[k].is_none() {
                                if let Some(label) = panes.iter().find(|p| p.starts_with(&prefix)) {
                                    claims[k] = Some(Tracked { label: label.clone(), mixed: false });
                                }
                            }
                            if let Some(c) = claims[k].as_mut() {
                                c.mixed = panes.len() > 1;
                            }
                        }
                        if let (Some(c), Some(rect)) = (claims[k].as_ref(), rect_of(t)) {
                            tabs.push(TabEntry {
                                root: key,
                                rect,
                                title: title.clone(),
                                label: c.label.clone(),
                                mixed: c.mixed,
                                index: k,
                            });
                        }
                        k += 1;
                    }
                    snapshots.insert(key, names.into_iter().zip(claims).collect());
                }
            }
            Err(_) => complete = false,
        }
        if complete {
            snapshots.retain(|w, _| windows_seen.contains(w));
        }
        let tracked: usize = snapshots.values().map(|s| s.iter().filter(|(_, c)| c.is_some()).count()).sum();
        let mixed: Vec<String> = snapshots
            .values()
            .flat_map(|s| {
                s.iter().filter_map(|(n, c)| {
                    c.as_ref()
                        .filter(|c| c.mixed || *n != c.label)
                        .map(|c| format!("{} as {:?}{}", c.label, n, if c.mixed { " [mixed]" } else { "" }))
                })
            })
            .collect();
        let summary = format!("{tracked} tracked, {} with rects; renamed/mixed: {}", tabs.len(), mixed.join("; "));
        if summary != last {
            println!(
                "{} cache: {} windows, refresh {} ms: {summary}",
                now(),
                windows_seen.len(),
                started.elapsed().as_millis()
            );
            last = summary;
        }
        *CACHE.lock().unwrap() = Cache { windows: windows_seen, tabs };
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn font(px: i32, face: PCWSTR) -> HFONT {
    unsafe {
        CreateFontW(
            -px,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            face,
        )
    }
}

fn open_menu(tab: TabEntry, pt: POINT) {
    unsafe {
        let started = Instant::now();
        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mut dpi, mut _dpi_y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut _dpi_y);
        let scale = dpi as f32 / 96.0;
        let px = |v: f32| (v * scale).round() as i32;
        // read on every open, so theme changes apply to the next menu
        let look = theme::look_for(HWND(tab.root as *mut _));
        let ts = look.text_scale;
        let text_font = font(px(14.0 * ts), w!("Segoe UI"));
        let icon_font = font(px(16.0), w!("Segoe Fluent Icons"));
        let items = items_for(&tab);

        // measure
        let hdc = GetDC(None);
        let old = SelectObject(hdc, text_font.into());
        let mut text_w = 0;
        for item in &items {
            let (text, x) = match item {
                Item::Action(_, _, label) => (label, px(TEXT_X)),
                Item::Header(text) => (text, px(ICON_X)),
                Item::Separator => continue,
            };
            let mut size = SIZE::default();
            let _ = GetTextExtentPoint32W(hdc, &wide(text), &mut size);
            text_w = text_w.max(x + size.cx);
        }
        SelectObject(hdc, old);
        ReleaseDC(None, hdc);

        let mut rows = Vec::new();
        let mut y = px(PAD_V);
        for item in &items {
            let h = match item {
                // WinUI rows grow with the text
                Item::Action(..) | Item::Header(_) => px(ROW_H + 14.0 * (ts - 1.0) * 1.4),
                Item::Separator => px(SEP_H),
            };
            rows.push((y, y + h));
            y += h;
        }
        let size = SIZE { cx: (text_w + px(PAD_RIGHT)).max(px(MIN_W)), cy: y + px(PAD_V) };

        // keep it on the monitor's work area
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let x = pt.x.min(work.right - size.cx).max(work.left);
        let y = if pt.y + size.cy > work.bottom { pt.y - size.cy } else { pt.y };

        let instance = GetModuleHandleW(None).unwrap_or_default();
        let Ok(popup) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            POPUP_CLASS,
            w!(""),
            WS_POPUP,
            x,
            y,
            size.cx,
            size.cy,
            Some(owner()),
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            popup,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const _,
            std::mem::size_of_val(&corner) as u32,
        );
        let border = look.palette.border;
        let _ = DwmSetWindowAttribute(popup, DWMWA_BORDER_COLOR, &border as *const _ as *const _, 4);
        let description = look.description.clone();
        let (label, title, mixed, index) = (tab.label.clone(), tab.title.clone(), tab.mixed, tab.index);

        MENU_RECT[0].store(x, Ordering::SeqCst);
        MENU_RECT[1].store(y, Ordering::SeqCst);
        MENU_RECT[2].store(x + size.cx, Ordering::SeqCst);
        MENU_RECT[3].store(y + size.cy, Ordering::SeqCst);
        MENU.with(|m| {
            *m.borrow_mut() =
                Some(Menu { popup, tab, items, hover: None, scale, look, size, text_font, icon_font, rows })
        });
        MENU_OPEN.store(true, Ordering::SeqCst);
        let _ = ShowWindow(popup, SW_SHOWNOACTIVATE);

        let fg = GetForegroundWindow();
        let mut class = [0u16; 64];
        let n = GetClassNameW(fg, &mut class);
        println!(
            "{} menu for {label:?} (tab #{index}, title {title:?}, mixed {mixed}) at ({x},{y}) {}x{} look: {description}; built in {} ms; foreground {}",
            now(),
            size.cx,
            size.cy,
            started.elapsed().as_millis(),
            String::from_utf16_lossy(&class[..n as usize])
        );
        if AUTO_DISMISS.load(Ordering::SeqCst) {
            SetTimer(Some(owner()), TIMER_DISMISS, 1500, None);
        }
    }
}

fn close_menu(reason: &str) -> Option<Menu> {
    let menu = MENU.with(|m| m.borrow_mut().take())?;
    MENU_OPEN.store(false, Ordering::SeqCst);
    MENU_RECT.iter().for_each(|v| v.store(0, Ordering::SeqCst));
    unsafe {
        let _ = DestroyWindow(menu.popup);
        let _ = DeleteObject(menu.text_font.into());
        let _ = DeleteObject(menu.icon_font.into());
    }
    if !reason.is_empty() {
        println!("{} menu closed ({reason}) for {:?}", now(), menu.tab.label);
    }
    Some(menu)
}

fn choose(index: usize) {
    let chosen = MENU.with(|m| {
        let menu = m.borrow();
        match menu.as_ref()?.items.get(index)? {
            Item::Action(id, _, label) => Some((*id, label.clone())),
            _ => None,
        }
    });
    let Some((id, label)) = chosen else { return };
    let Some(menu) = close_menu("") else { return };
    println!("{} chose {id} {label:?} for {:?} (mixed {})", now(), menu.tab.label, menu.tab.mixed);
}

fn set_hover(hover: Option<usize>) {
    MENU.with(|m| {
        if let Some(menu) = m.borrow_mut().as_mut() {
            if menu.hover != hover {
                menu.hover = hover;
                unsafe {
                    let _ = InvalidateRect(Some(menu.popup), None, false);
                }
            }
        }
    });
}

fn menu_key(vk: VIRTUAL_KEY) {
    let state = MENU.with(|m| {
        let menu = m.borrow();
        let menu = menu.as_ref()?;
        let actions: Vec<usize> =
            menu.items.iter().enumerate().filter(|(_, i)| matches!(i, Item::Action(..))).map(|(n, _)| n).collect();
        Some((actions, menu.hover))
    });
    let Some((actions, hover)) = state else { return };
    let pos = hover.and_then(|h| actions.iter().position(|a| *a == h));
    match vk {
        VK_DOWN => set_hover(Some(actions[pos.map(|p| (p + 1) % actions.len()).unwrap_or(0)])),
        VK_UP => {
            set_hover(Some(actions[pos.map(|p| (p + actions.len() - 1) % actions.len()).unwrap_or(actions.len() - 1)]))
        }
        VK_RETURN => {
            if let Some(h) = hover {
                choose(h);
            }
        }
        VK_ESCAPE => {
            close_menu("Esc");
        }
        _ => {}
    }
}

fn paint(hwnd: HWND) {
    MENU.with(|m| {
        let menu = m.borrow();
        let Some(menu) = menu.as_ref() else { return };
        let colors = &menu.look.palette;
        let px = |v: f32| (v * menu.scale).round() as i32;
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let (w, h) = (menu.size.cx, menu.size.cy);
            let mem = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old_bmp = SelectObject(mem, bmp.into());

            let background = CreateSolidBrush(colors.background);
            FillRect(mem, &RECT { left: 0, top: 0, right: w, bottom: h }, background);
            SetBkMode(mem, TRANSPARENT);
            let flags = DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX;
            let old_font = SelectObject(mem, menu.text_font.into());

            for (n, (item, (top, bottom))) in menu.items.iter().zip(&menu.rows).enumerate() {
                match item {
                    Item::Separator => {
                        let mid = (top + bottom) / 2;
                        let line = CreateSolidBrush(colors.separator);
                        FillRect(mem, &RECT { left: 0, top: mid, right: w, bottom: mid + px(1.0).max(1) }, line);
                        let _ = DeleteObject(line.into());
                    }
                    Item::Header(text) => {
                        SetTextColor(mem, colors.dim);
                        SelectObject(mem, menu.text_font.into());
                        let mut text = wide(text);
                        let mut r = RECT { left: px(ICON_X), top: *top, right: w, bottom: *bottom };
                        DrawTextW(mem, &mut text, &mut r, flags);
                    }
                    Item::Action(_, glyph, label) => {
                        let hovered = menu.hover == Some(n);
                        if hovered {
                            let brush = CreateSolidBrush(colors.hover);
                            let old_brush = SelectObject(mem, brush.into());
                            let old_pen = SelectObject(mem, GetStockObject(NULL_PEN));
                            let _ = RoundRect(
                                mem,
                                px(ROW_INSET),
                                top + px(2.0),
                                w - px(ROW_INSET) + 1,
                                bottom - px(2.0) + 1,
                                px(8.0),
                                px(8.0),
                            );
                            SelectObject(mem, old_pen);
                            SelectObject(mem, old_brush);
                            let _ = DeleteObject(brush.into());
                        }
                        SetTextColor(mem, if hovered { colors.hover_text } else { colors.text });
                        SelectObject(mem, menu.icon_font.into());
                        let mut icon = wide(&glyph.to_string());
                        let mut r = RECT { left: px(ICON_X), top: *top, right: px(TEXT_X), bottom: *bottom };
                        DrawTextW(mem, &mut icon, &mut r, flags);
                        SelectObject(mem, menu.text_font.into());
                        let mut text = wide(label);
                        let mut r = RECT { left: px(TEXT_X), top: *top, right: w, bottom: *bottom };
                        DrawTextW(mem, &mut text, &mut r, flags);
                    }
                }
            }
            SelectObject(mem, old_font);
            let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
            let _ = DeleteObject(background.into());
            SelectObject(mem, old_bmp);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
            let _ = EndPaint(hwnd, &ps);
        }
    });
}

fn item_at(y: i32) -> Option<usize> {
    MENU.with(|m| {
        let menu = m.borrow();
        let menu = menu.as_ref()?;
        menu.items
            .iter()
            .zip(&menu.rows)
            .position(|(item, (top, bottom))| matches!(item, Item::Action(..)) && y >= *top && y < *bottom)
    })
}

unsafe extern "system" fn popup_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = unsafe { TrackMouseEvent(&mut tme) };
            set_hover(item_at(y));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            set_hover(None);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(n) = item_at(y) {
                choose(n);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe extern "system" fn owner_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SHOW_MENU => {
            close_menu("replaced");
            if let Some((tab, pt)) = PENDING.lock().unwrap().take() {
                open_menu(tab, pt);
            }
            LRESULT(0)
        }
        WM_CLOSE_MENU => {
            close_menu("outside click or other key");
            LRESULT(0)
        }
        WM_MENU_KEY => {
            menu_key(VIRTUAL_KEY(wparam.0 as u16));
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_DISMISS => unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_DISMISS);
            close_menu("auto-dismiss");
            LRESULT(0)
        },
        WM_TIMER if wparam.0 == TIMER_END => unsafe {
            PostQuitMessage(0);
            LRESULT(0)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn run(seconds: u32, prefix: String, auto_dismiss: bool) -> windows::core::Result<()> {
    AUTO_DISMISS.store(auto_dismiss, Ordering::SeqCst);
    std::thread::spawn(move || refresher(prefix));
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let owner_class = w!("NativeTermMenuOwner");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(owner_proc),
            hInstance: instance.into(),
            lpszClassName: owner_class,
            ..Default::default()
        });
        RegisterClassW(&WNDCLASSW {
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(popup_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: POPUP_CLASS,
            ..Default::default()
        });
        let owner = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            owner_class,
            w!("NativeTerm menu owner"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        OWNER.store(owner.0 as isize, Ordering::SeqCst);
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(instance.into()), 0)?;
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(instance.into()), 0)?;
        SetTimer(Some(owner), TIMER_END, seconds * 1000, None);
        println!("{} hooks installed for {seconds}s", now());
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        close_menu("exit");
        let _ = UnhookWindowsHookEx(mouse);
        let _ = UnhookWindowsHookEx(keyboard);
    }
    let calls = HOOK_CALLS.load(Ordering::Relaxed).max(1);
    println!(
        "{} done: right-button events {} (passed {}, swallowed {}, injected {}), hook time avg {} us, max {} us",
        now(),
        HOOK_CALLS.load(Ordering::Relaxed),
        PASSED.load(Ordering::Relaxed),
        SWALLOWED.load(Ordering::Relaxed),
        INJECTED.load(Ordering::Relaxed),
        HOOK_TOTAL_MICROS.load(Ordering::Relaxed) / calls as u64,
        HOOK_MAX_MICROS.load(Ordering::Relaxed)
    );
    Ok(())
}

fn send(inputs: &[INPUT]) {
    unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
}

fn mouse(flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: flags, ..Default::default() } } }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    let flags = if up { KEYEVENTF_KEYUP } else { Default::default() };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: flags, ..Default::default() } },
    }
}

/// Terminal's own menus are XAML flyouts: Menu elements in the window.
fn open_terminal_menus(automation: &UIAutomation) -> uiautomation::Result<Vec<Vec<String>>> {
    let mut found = Vec::new();
    let walker = automation.get_control_view_walker()?;
    for w in terminal_windows(automation)? {
        let mut stack = vec![w];
        while let Some(e) = stack.pop() {
            if e.get_control_type().map(|t| t == ControlType::Menu).unwrap_or(false) {
                let items = e
                    .find_all(TreeScope::Descendants, &automation.create_true_condition()?)
                    .unwrap_or_default()
                    .iter()
                    .filter(|i| i.get_control_type().map(|t| t == ControlType::MenuItem).unwrap_or(false))
                    .map(|i| i.get_name().unwrap_or_default())
                    .collect();
                found.push(items);
            }
            let mut c = walker.get_first_child(&e).ok();
            while let Some(ch) = c {
                stack.push(ch.clone());
                c = walker.get_next_sibling(&ch).ok();
            }
        }
    }
    Ok(found)
}

fn report_menus(automation: &UIAutomation, close: bool) -> uiautomation::Result<()> {
    let menus = open_terminal_menus(automation)?;
    let ours = unsafe { FindWindowW(POPUP_CLASS, PCWSTR::null()) }
        .map(|m| unsafe { IsWindowVisible(m) }.as_bool())
        .unwrap_or(false);
    let fg = unsafe { GetForegroundWindow() };
    let mut class = [0u16; 64];
    let n = unsafe { GetClassNameW(fg, &mut class) };
    println!(
        "{} Terminal menus open: {}, our popup visible: {ours}, foreground: {}",
        now(),
        menus.len(),
        String::from_utf16_lossy(&class[..n as usize])
    );
    for m in &menus {
        println!("    Terminal menu items: {m:?}");
    }
    if close && !menus.is_empty() {
        send(&[key(VK_ESCAPE, false), key(VK_ESCAPE, true)]);
        println!("{} sent Escape to close Terminal's menu", now());
    }
    Ok(())
}

fn right_click(tab: &UIElement, shift: bool) -> bool {
    let Some(r) = rect_of(tab) else { return false };
    let (x, y) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
    unsafe {
        let mut saved = POINT::default();
        let _ = GetCursorPos(&mut saved);
        let _ = SetCursorPos(x, y);
        if shift {
            send(&[
                key(VK_SHIFT, false),
                mouse(MOUSEEVENTF_RIGHTDOWN),
                mouse(MOUSEEVENTF_RIGHTUP),
                key(VK_SHIFT, true),
            ]);
        } else {
            send(&[mouse(MOUSEEVENTF_RIGHTDOWN), mouse(MOUSEEVENTF_RIGHTUP)]);
        }
        std::thread::sleep(Duration::from_millis(50));
        let _ = SetCursorPos(saved.x, saved.y);
    }
    println!(
        "{} right-clicked{} {:?} at ({x},{y})",
        now(),
        if shift { " (Shift)" } else { "" },
        tab.get_name().unwrap_or_default()
    );
    true
}

fn click(title: Option<&str>, index: Option<usize>, shift: bool) -> uiautomation::Result<()> {
    let automation = UIAutomation::new()?;
    let windows = terminal_windows(&automation)?;
    let target = match (title, index) {
        (Some(title), _) => windows
            .iter()
            .flat_map(|w| scan(&automation, w).map(|(t, _, _)| t).unwrap_or_default())
            .find(|t| t.get_name().unwrap_or_default() == title),
        (None, Some(i)) => {
            windows.first().and_then(|w| scan(&automation, w).ok()).and_then(|(t, _, _)| t.into_iter().nth(i))
        }
        _ => None,
    };
    match target {
        Some(tab) if right_click(&tab, shift) => {
            std::thread::sleep(Duration::from_millis(800));
            report_menus(&automation, false)
        }
        _ => {
            println!("no such tab in the test Terminal");
            Ok(())
        }
    }
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let result = match args.first().map(String::as_str) {
        Some("run") => {
            let secs = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(60);
            let prefix = args.get(2).cloned().unwrap_or_else(|| "nt-".into());
            if let Err(e) = run(secs, prefix, flag("--auto-dismiss")) {
                eprintln!("run failed: {e}");
            }
            Ok(())
        }
        Some("click-tab") => click(args.get(1).map(String::as_str), None, flag("--shift")),
        Some("click") => click(None, args.get(1).and_then(|s| s.parse().ok()), flag("--shift")),
        Some("menus") => UIAutomation::new().and_then(|a| report_menus(&a, flag("--close"))),
        _ => {
            eprintln!("usage: menu-hook run <secs> <prefix> [--auto-dismiss] | click-tab <title> | click <index> [--shift] | menus [--close]");
            Ok(())
        }
    };
    if let Err(e) = result {
        eprintln!("failed: {e}");
    }
}
