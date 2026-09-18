//! NativeTerm's own right-click menu on its Windows Terminal tabs (see
//! "Context menus" in `docs/ARCHITECTURE.md`).
//!
//! - A low-level mouse hook swallows right-clicks on NativeTerm's tabs and
//!   opens this menu instead; Terminal's own tab menu is blocked there by
//!   design. Every other right-click passes through.
//! - The hook only uses the tab rectangles NativeTerm last scanned, and
//!   only while they are current: after a change notification it lets
//!   clicks through until the next scan. It is installed only while
//!   NativeTerm has tabs.
//! - The popup is custom-drawn like a WinUI menu flyout (Direct2D and
//!   DirectWrite, see `menu_draw`), follows the Terminal's theme, and
//!   never activates (the Terminal keeps focus). A keyboard hook handles
//!   Up/Down/Enter/Esc while it is open.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
};
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EndPaint, GetDC, GetMonitorInfoW,
    InvalidateRect, MonitorFromPoint, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VIRTUAL_KEY, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetAncestor, GetMessageW,
    LoadCursorW, PostMessageW, PostThreadMessageW, RegisterClassW, SetWindowsHookExW, ShowWindow, TranslateMessage,
    UnhookWindowsHookEx, UpdateLayeredWindow, WindowFromPoint, CS_DROPSHADOW, GA_ROOT, ULW_ALPHA, WS_EX_LAYERED, HC_ACTION, HHOOK, IDC_ARROW, KBDLLHOOKSTRUCT,
    MA_NOACTIVATE, MSG, MSLLHOOKSTRUCT, SW_SHOWNOACTIVATE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MOUSEACTIVATE, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_PAINT, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_XBUTTONDOWN, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::menu_draw::Painter;
use super::theme::{self, Look};
use crate::Rect;

/// Window class of the popup (tests look for it).
pub const POPUP_CLASS: PCWSTR = w!("NativeTermMenuPopup");
/// The same popup drawing its own rounded shape (Windows 10): no system
/// drop shadow, which is rectangular and would show at the corners.
const LAYERED_CLASS: PCWSTR = w!("NativeTermMenuPopupLayered");
const OWNER_CLASS: PCWSTR = w!("NativeTermMenuOwner");
const WM_SHOW_MENU: u32 = WM_APP + 1;
const WM_CLOSE_MENU: u32 = WM_APP + 2;
const WM_MENU_KEY: u32 = WM_APP + 3;
const WM_SYNC_HOOKS: u32 = WM_APP + 4;
const WM_CHOOSE_ID: u32 = WM_APP + 5;
const WM_MOUSELEAVE: u32 = 0x02A3;

/// A NativeTerm tab as the menu knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuTab {
    pub window: isize,
    pub rect: Rect,
    /// The session label the tab was claimed for.
    pub label: String,
    /// The tab's current title.
    pub title: String,
    pub mixed: bool,
    pub index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A glyph from Segoe Fluent Icons (Segoe MDL2 Assets on Windows 10).
    Action { id: u32, glyph: char, text: String, enabled: bool },
    Header(String),
    Separator,
}

/// What the menu shows for a tab, and what happens when an item is chosen.
/// Called on the menu thread: don't block.
pub trait Provider: Send + Sync {
    fn entries(&self, tab: &MenuTab) -> Vec<Entry>;
    fn chosen(&self, tab: &MenuTab, id: u32);
}

struct Shared {
    tabs: Mutex<Vec<MenuTab>>,
    /// Something changed since the tabs were scanned: don't trust them.
    stale: AtomicBool,
    provider: Arc<dyn Provider>,
    settings: PathBuf,
    owner: AtomicIsize,
    open: AtomicBool,
    menu_rect: [AtomicI32; 4],
    swallowed_down: AtomicBool,
    swallowed_key: AtomicU32,
    opened: AtomicU32,
    hooked: AtomicBool,
    right_clicks: AtomicU32,
    last_chosen: AtomicU32,
    /// Id of the highlighted item, 0 for none.
    hovered: AtomicU32,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

fn shared() -> Option<&'static Arc<Shared>> {
    SHARED.get()
}

pub struct TabMenu {
    shared: Arc<Shared>,
    thread: u32,
}

impl TabMenu {
    /// One per process. `settings` is the Terminal's `settings.json` (for
    /// its theme).
    pub fn start(settings: PathBuf, provider: Arc<dyn Provider>) -> std::io::Result<TabMenu> {
        let shared = Arc::new(Shared {
            tabs: Mutex::new(Vec::new()),
            stale: AtomicBool::new(true),
            provider,
            settings,
            owner: AtomicIsize::new(0),
            open: AtomicBool::new(false),
            menu_rect: Default::default(),
            swallowed_down: AtomicBool::new(false),
            swallowed_key: AtomicU32::new(0),
            opened: AtomicU32::new(0),
            hooked: AtomicBool::new(false),
            right_clicks: AtomicU32::new(0),
            last_chosen: AtomicU32::new(0),
            hovered: AtomicU32::new(0),
        });
        if SHARED.set(Arc::clone(&shared)).is_err() {
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "the tab menu is already running"));
        }
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new().name("tab-menu".into()).spawn(move || menu_thread(tx))?;
        let thread = rx.recv().map_err(|_| std::io::Error::other("tab menu thread failed"))?;
        Ok(TabMenu { shared, thread })
    }

    /// The current NativeTerm tabs, fresh from a scan.
    pub fn set_tabs(&self, tabs: Vec<MenuTab>) {
        *self.shared.tabs.lock().unwrap_or_else(|e| e.into_inner()) = tabs;
        self.shared.stale.store(false, Ordering::SeqCst);
        self.post(WM_SYNC_HOOKS);
    }

    /// Tabs may have moved: pass right-clicks through until the next scan.
    pub fn invalidate(&self) {
        self.shared.stale.store(true, Ordering::SeqCst);
    }

    pub fn is_open(&self) -> bool {
        self.shared.open.load(Ordering::SeqCst)
    }

    /// Diagnostics: known tabs, stale flag, hooks installed, right-clicks seen.
    pub fn debug_state(&self) -> String {
        let tabs = self.shared.tabs.lock().unwrap_or_else(|e| e.into_inner());
        format!(
            "tabs {:?}, stale {}, hooked {}, right-clicks {}, last chosen {}",
            tabs.iter().map(|t| (&t.label, t.rect)).collect::<Vec<_>>(),
            self.shared.stale.load(Ordering::SeqCst),
            self.shared.hooked.load(Ordering::SeqCst),
            self.shared.right_clicks.load(Ordering::SeqCst),
            self.shared.last_chosen.load(Ordering::SeqCst)
        )
    }

    /// Choose an item of the open menu by id, as a click would
    /// (automation). Ignored if it isn't there or is disabled.
    pub fn choose(&self, id: u32) {
        let owner = HWND(self.shared.owner.load(Ordering::SeqCst) as *mut _);
        unsafe {
            let _ = PostMessageW(Some(owner), WM_CHOOSE_ID, WPARAM(id as usize), LPARAM(0));
        }
    }

    /// Id of the highlighted item of the open menu, if any.
    pub fn hovered(&self) -> Option<u32> {
        Some(self.shared.hovered.load(Ordering::SeqCst)).filter(|&id| id != 0)
    }

    /// How many menus were opened (diagnostics, tests).
    pub fn opened(&self) -> u32 {
        self.shared.opened.load(Ordering::SeqCst)
    }

    fn post(&self, message: u32) {
        let owner = HWND(self.shared.owner.load(Ordering::SeqCst) as *mut _);
        if !owner.is_invalid() {
            unsafe {
                let _ = PostMessageW(Some(owner), message, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl Drop for TabMenu {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

// ---- layout (effective pixels, after WinUI's MenuFlyout) -----------------

const PAD_V: f32 = 4.0;
const ROW_H: f32 = 32.0;
const SEP_H: f32 = 9.0;
const ROW_INSET: f32 = 4.0;
const ICON_X: f32 = 16.0;
const TEXT_X: f32 = 44.0;
const PAD_RIGHT: f32 = 28.0;
const MIN_W: f32 = 164.0;

struct Menu {
    popup: HWND,
    tab: MenuTab,
    entries: Vec<Entry>,
    hover: Option<usize>,
    /// The highlight follows the mouse (vs. the keyboard).
    hover_by_mouse: bool,
    scale: f32,
    look: Look,
    size: SIZE,
    /// Draws its own rounded shape (`UpdateLayeredWindow`) where DWM
    /// doesn't round popups (Windows 10).
    layered: bool,
    text_format: IDWriteTextFormat,
    icon_format: IDWriteTextFormat,
    /// (top, bottom) per entry, physical pixels.
    rows: Vec<(i32, i32)>,
}

struct Hooks {
    mouse: HHOOK,
    keyboard: HHOOK,
}

thread_local! {
    static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
    static HOOKS: RefCell<Option<Hooks>> = const { RefCell::new(None) };
}

/// Handed from the mouse hook to the owner window (both on the menu
/// thread; the hook itself stays trivial).
static PENDING_TAB: Mutex<Option<(MenuTab, POINT)>> = Mutex::new(None);

fn contains(r: &Rect, p: POINT) -> bool {
    r.contains(p.x, p.y)
}

fn post(message: u32, wparam: usize) {
    if let Some(s) = shared() {
        let owner = HWND(s.owner.load(Ordering::SeqCst) as *mut _);
        unsafe {
            let _ = PostMessageW(Some(owner), message, WPARAM(wparam), LPARAM(0));
        }
    }
}

fn inside_menu(s: &Shared, p: POINT) -> bool {
    let r = Rect {
        left: s.menu_rect[0].load(Ordering::SeqCst),
        top: s.menu_rect[1].load(Ordering::SeqCst),
        right: s.menu_rect[2].load(Ordering::SeqCst),
        bottom: s.menu_rect[3].load(Ordering::SeqCst),
    };
    contains(&r, p)
}

// Runs inside the hook: no UIA, no blocking, pass through when in doubt.
fn hit_test(s: &Shared, pt: POINT) -> Option<MenuTab> {
    if s.stale.load(Ordering::SeqCst) {
        return None;
    }
    let root = unsafe { GetAncestor(WindowFromPoint(pt), GA_ROOT) }.0 as isize;
    let tabs = s.tabs.try_lock().ok()?;
    tabs.iter().find(|t| t.window == root && contains(&t.rect, pt)).cloned()
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some(s) = shared() {
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            let open = s.open.load(Ordering::SeqCst);
            match wparam.0 as u32 {
                WM_RBUTTONDOWN => {
                    s.right_clicks.fetch_add(1, Ordering::SeqCst);
                    let swallow = if open && inside_menu(s, info.pt) {
                        true
                    } else if let Some(tab) = hit_test(s, info.pt) {
                        if let Ok(mut pending) = PENDING_TAB.try_lock() {
                            *pending = Some((tab, info.pt));
                        }
                        post(WM_SHOW_MENU, 0);
                        true
                    } else {
                        if open {
                            post(WM_CLOSE_MENU, 0);
                        }
                        false
                    };
                    s.swallowed_down.store(swallow, Ordering::SeqCst);
                    if swallow {
                        return LRESULT(1);
                    }
                }
                WM_RBUTTONUP => {
                    // only the up of a swallowed down
                    if s.swallowed_down.swap(false, Ordering::SeqCst) {
                        return LRESULT(1);
                    }
                }
                WM_LBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEHWHEEL
                    if open && !inside_menu(s, info.pt) =>
                {
                    // an outside click closes the popup and still happens
                    post(WM_CLOSE_MENU, 0);
                }
                _ => {}
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some(s) = shared() {
            let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let vk = VIRTUAL_KEY(info.vkCode as u16);
            if s.open.load(Ordering::SeqCst) {
                if [VK_UP, VK_DOWN, VK_RETURN, VK_ESCAPE].contains(&vk) {
                    if down {
                        s.swallowed_key.store(info.vkCode, Ordering::SeqCst);
                        post(WM_MENU_KEY, info.vkCode as usize);
                    }
                    return LRESULT(1);
                }
                if down {
                    post(WM_CLOSE_MENU, 0);
                }
            } else if !down && s.swallowed_key.load(Ordering::SeqCst) == info.vkCode {
                // the key-up of a key that closed the menu
                s.swallowed_key.store(0, Ordering::SeqCst);
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Install the hooks while there are tabs, remove them otherwise: a
/// low-level mouse hook is called for every mouse event on the desktop.
fn sync_hooks() {
    let Some(s) = shared() else { return };
    let wanted = !s.tabs.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
    HOOKS.with(|hooks| {
        let mut hooks = hooks.borrow_mut();
        match (wanted, hooks.is_some()) {
            (true, false) => unsafe {
                let instance = GetModuleHandleW(None).ok().map(|h| h.into());
                if let (Ok(mouse), Ok(keyboard)) = (
                    SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), instance, 0),
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), instance, 0),
                ) {
                    *hooks = Some(Hooks { mouse, keyboard });
                    s.hooked.store(true, Ordering::SeqCst);
                }
            },
            (false, true) => {
                s.hooked.store(false, Ordering::SeqCst);
                if let Some(h) = hooks.take() {
                    unsafe {
                        let _ = UnhookWindowsHookEx(h.mouse);
                        let _ = UnhookWindowsHookEx(h.keyboard);
                    }
                }
                close_menu();
            }
            _ => {}
        }
    });
}

thread_local! {
    /// Direct2D and DirectWrite, made on the menu thread when first needed.
    static PAINTER: RefCell<Option<Rc<Painter>>> = const { RefCell::new(None) };
}

fn painter() -> Option<Rc<Painter>> {
    PAINTER.with(|p| {
        let mut p = p.borrow_mut();
        if p.is_none() {
            *p = Painter::new().ok().map(Rc::new);
        }
        p.clone()
    })
}

/// Windows 11 (build 22000) rounds popups through DWM; Windows 10 doesn't,
/// so the popup draws its own shape there. `NATIVETERM_MENU_LAYERED=1`
/// forces that path (for trying it on Windows 11).
fn draws_own_shape() -> bool {
    if std::env::var("NATIVETERM_MENU_LAYERED").as_deref() == Ok("1") {
        return true;
    }
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let mut buffer = [0u16; 32];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return false;
    }
    let text = String::from_utf16_lossy(&buffer[..(size as usize / 2).saturating_sub(1)]);
    text.trim().parse::<u32>().is_ok_and(|build| build < 22000)
}

fn icon_face() -> &'static str {
    // Windows 11 has Segoe Fluent Icons; Windows 10 the same code points in
    // Segoe MDL2 Assets
    let fonts = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    if fonts.join(r"Fonts\SegoeIcons.ttf").exists() {
        "Segoe Fluent Icons"
    } else {
        "Segoe MDL2 Assets"
    }
}

fn open_menu(tab: MenuTab, pt: POINT) {
    let Some(s) = shared() else { return };
    let entries = s.provider.entries(&tab);
    if entries.is_empty() {
        return;
    }
    unsafe {
        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mut dpi, mut dpi_y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y);
        let scale = dpi as f32 / 96.0;
        let px = |v: f32| (v * scale).round() as i32;
        let look = theme::look(&s.settings);
        let ts = look.text_scale;
        let Some(painter) = painter() else { return };
        let (Ok(text_format), Ok(icon_format)) =
            (painter.format("Segoe UI", px(14.0 * ts) as f32), painter.format(icon_face(), px(16.0) as f32))
        else {
            return;
        };

        // measure
        let mut text_w = 0;
        for entry in &entries {
            let (text, x) = match entry {
                Entry::Action { text, .. } => (text, px(TEXT_X)),
                Entry::Header(text) => (text, px(ICON_X)),
                Entry::Separator => continue,
            };
            text_w = text_w.max(x + painter.width(&text_format, text).ceil() as i32);
        }

        let mut rows = Vec::new();
        let mut y = px(PAD_V);
        for entry in &entries {
            let h = match entry {
                // WinUI rows grow with the text
                Entry::Action { .. } | Entry::Header(_) => px(ROW_H + 14.0 * (ts - 1.0) * 1.4),
                Entry::Separator => px(SEP_H),
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

        let owner = HWND(s.owner.load(Ordering::SeqCst) as *mut _);
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let layered = draws_own_shape();
        let (class, extra) = if layered { (LAYERED_CLASS, WS_EX_LAYERED) } else { (POPUP_CLASS, Default::default()) };
        let Ok(popup) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | extra,
            class,
            w!("NativeTerm tab menu"),
            WS_POPUP,
            x,
            y,
            size.cx,
            size.cy,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };
        // the layered popup draws its own shape and border
        let corner = if layered { DWMWCP_DONOTROUND } else { DWMWCP_ROUND };
        let _ = DwmSetWindowAttribute(
            popup,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const _,
            std::mem::size_of_val(&corner) as u32,
        );
        if !layered {
            let border = look.palette.border;
            let _ = DwmSetWindowAttribute(popup, DWMWA_BORDER_COLOR, &border as *const _ as *const _, 4);
        }

        for (i, v) in [x, y, x + size.cx, y + size.cy].into_iter().enumerate() {
            s.menu_rect[i].store(v, Ordering::SeqCst);
        }
        MENU.with(|m| {
            *m.borrow_mut() = Some(Menu {
                popup,
                tab,
                entries,
                hover: None,
                hover_by_mouse: false,
                scale,
                look,
                size,
                layered,
                text_format,
                icon_format,
                rows,
            })
        });
        if layered {
            present_layered();
        }
        s.open.store(true, Ordering::SeqCst);
        s.opened.fetch_add(1, Ordering::SeqCst);
        let _ = ShowWindow(popup, SW_SHOWNOACTIVATE);
    }
}

fn close_menu() -> Option<Menu> {
    let menu = MENU.with(|m| m.borrow_mut().take())?;
    if let Some(s) = shared() {
        s.open.store(false, Ordering::SeqCst);
        s.hovered.store(0, Ordering::SeqCst);
        s.menu_rect.iter().for_each(|v| v.store(0, Ordering::SeqCst));
    }
    unsafe {
        let _ = DestroyWindow(menu.popup);
    }
    Some(menu)
}

fn enabled_action(entry: &Entry) -> Option<u32> {
    match entry {
        Entry::Action { id, enabled: true, .. } => Some(*id),
        _ => None,
    }
}

fn choose(index: usize) {
    let chosen = MENU.with(|m| m.borrow().as_ref().and_then(|menu| menu.entries.get(index).and_then(enabled_action)));
    let Some(id) = chosen else { return };
    let Some(menu) = close_menu() else { return };
    if let Some(s) = shared() {
        s.last_chosen.store(id, Ordering::SeqCst);
        s.provider.chosen(&menu.tab, id);
    }
}

fn set_hover(hover: Option<usize>, by_mouse: bool) {
    MENU.with(|m| {
        if let Some(menu) = m.borrow_mut().as_mut() {
            menu.hover_by_mouse = by_mouse;
            if menu.hover != hover {
                menu.hover = hover;
                let id = hover.and_then(|h| menu.entries.get(h)).and_then(enabled_action).unwrap_or(0);
                if let Some(s) = shared() {
                    s.hovered.store(id, Ordering::SeqCst);
                }
                if !menu.layered {
                    unsafe {
                        let _ = InvalidateRect(Some(menu.popup), None, false);
                    }
                }
            }
        }
    });
    // a layered popup gets no WM_PAINT: it is drawn again right away
    if MENU.with(|m| m.borrow().as_ref().is_some_and(|menu| menu.layered)) {
        present_layered();
    }
}

fn menu_key(vk: VIRTUAL_KEY) {
    let state = MENU.with(|m| {
        let menu = m.borrow();
        let menu = menu.as_ref()?;
        let actions: Vec<usize> =
            menu.entries.iter().enumerate().filter(|(_, e)| enabled_action(e).is_some()).map(|(n, _)| n).collect();
        Some((actions, menu.hover))
    });
    let Some((actions, hover)) = state else { return };
    if actions.is_empty() {
        if vk == VK_ESCAPE {
            close_menu();
        }
        return;
    }
    let pos = hover.and_then(|h| actions.iter().position(|a| *a == h));
    match vk {
        VK_DOWN => set_hover(Some(actions[pos.map_or(0, |p| (p + 1) % actions.len())]), false),
        VK_UP => set_hover(Some(actions[pos.map_or(actions.len() - 1, |p| (p + actions.len() - 1) % actions.len())]), false),
        VK_RETURN => {
            if let Some(h) = hover {
                choose(h);
            }
        }
        VK_ESCAPE => {
            close_menu();
        }
        _ => {}
    }
}

fn paint(hwnd: HWND) {
    MENU.with(|m| {
        let menu = m.borrow();
        let Some(menu) = menu.as_ref() else { return };
        let (w, h) = (menu.size.cx, menu.size.cy);
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if let (Some(painter), false) = (painter(), menu.layered) {
                let _ = painter.paint(hdc, w, h, menu.look.palette.background, |canvas| draw_entries(canvas, menu));
            }
            let _ = EndPaint(hwnd, &ps);
        }
    });
}

/// Draw a layered popup: its rounded shape with a border, then the rows,
/// into a 32-bit DIB that `UpdateLayeredWindow` shows with its alpha.
fn present_layered() {
    MENU.with(|m| {
        let menu = m.borrow();
        let Some(menu) = menu.as_ref() else { return };
        let Some(painter) = painter() else { return };
        let colors = &menu.look.palette;
        let (w, h) = (menu.size.cx, menu.size.cy);
        let radius = 8.0 * menu.scale;
        unsafe {
            let screen = GetDC(None);
            let memory = CreateCompatibleDC(Some(screen));
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    // top-down
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            if let Ok(bitmap) = CreateDIBSection(Some(memory), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                let old = SelectObject(memory, bitmap.into());
                let drawn = painter.paint_alpha(memory, w, h, |canvas| {
                    canvas.fill_rounded(0, 0, w, h, radius, colors.border);
                    canvas.fill_rounded(1, 1, w - 1, h - 1, radius - 1.0, colors.background);
                    draw_entries(canvas, menu);
                });
                if drawn.is_ok() {
                    let mut rect = windows::Win32::Foundation::RECT::default();
                    let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(menu.popup, &mut rect);
                    let position = POINT { x: rect.left, y: rect.top };
                    let size = SIZE { cx: w, cy: h };
                    let source = POINT { x: 0, y: 0 };
                    let blend = windows::Win32::Graphics::Gdi::BLENDFUNCTION {
                        BlendOp: 0,  // AC_SRC_OVER
                        BlendFlags: 0,
                        SourceConstantAlpha: 255,
                        AlphaFormat: 1, // AC_SRC_ALPHA
                    };
                    let _ = UpdateLayeredWindow(
                        menu.popup,
                        Some(screen),
                        Some(&position),
                        Some(&size),
                        Some(memory),
                        Some(&source),
                        windows::Win32::Foundation::COLORREF(0),
                        Some(&blend),
                        ULW_ALPHA,
                    );
                }
                SelectObject(memory, old);
                let _ = DeleteObject(bitmap.into());
            }
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
        }
    });
}

/// The rows of the menu (header, items, separators).
fn draw_entries(canvas: &super::menu_draw::Canvas, menu: &Menu) {
    let colors = &menu.look.palette;
    let px = |v: f32| (v * menu.scale).round() as i32;
    let w = menu.size.cx;
    for (n, (entry, (top, bottom))) in menu.entries.iter().zip(&menu.rows).enumerate() {
        let (top, bottom) = (*top, *bottom);
        match entry {
            Entry::Separator => {
                let mid = (top + bottom) / 2;
                canvas.fill(0, mid, w, mid + px(1.0).max(1), colors.separator);
            }
            Entry::Header(text) => {
                canvas.text(&menu.text_format, text, px(ICON_X), top, w, bottom, colors.dim);
            }
            Entry::Action { glyph, text, enabled, .. } => {
                let hovered = *enabled && menu.hover == Some(n);
                if hovered {
                    let (left, right) = (px(ROW_INSET), w - px(ROW_INSET));
                    canvas.fill_rounded(left, top + px(2.0), right, bottom - px(2.0), px(4.0) as f32, colors.hover);
                }
                let color = match (enabled, hovered) {
                    (false, _) => colors.dim,
                    (true, true) => colors.hover_text,
                    (true, false) => colors.text,
                };
                canvas.text(&menu.icon_format, &glyph.to_string(), px(ICON_X), top, px(TEXT_X), bottom, color);
                canvas.text(&menu.text_format, text, px(TEXT_X), top, w, bottom, color);
            }
        }
    }
}

fn entry_at(y: i32) -> Option<usize> {
    MENU.with(|m| {
        let menu = m.borrow();
        let menu = menu.as_ref()?;
        menu.entries
            .iter()
            .zip(&menu.rows)
            .position(|(e, (top, bottom))| enabled_action(e).is_some() && y >= *top && y < *bottom)
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
            // over an item only: a move elsewhere keeps a keyboard highlight
            if let Some(n) = entry_at(y) {
                set_hover(Some(n), true);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let by_mouse = MENU.with(|m| m.borrow().as_ref().is_some_and(|menu| menu.hover_by_mouse));
            if by_mouse {
                set_hover(None, true);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(n) = entry_at(y) {
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
            close_menu();
            let pending = PENDING_TAB.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some((tab, pt)) = pending {
                open_menu(tab, pt);
            }
            LRESULT(0)
        }
        WM_CLOSE_MENU => {
            close_menu();
            LRESULT(0)
        }
        WM_MENU_KEY => {
            menu_key(VIRTUAL_KEY(wparam.0 as u16));
            LRESULT(0)
        }
        WM_SYNC_HOOKS => {
            sync_hooks();
            LRESULT(0)
        }
        WM_CHOOSE_ID => {
            let id = wparam.0 as u32;
            let index = MENU.with(|m| {
                m.borrow().as_ref().and_then(|menu| menu.entries.iter().position(|e| enabled_action(e) == Some(id)))
            });
            if let Some(index) = index {
                choose(index);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn menu_thread(ready: mpsc::Sender<u32>) {
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None).unwrap_or_default();
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(owner_proc),
            hInstance: instance.into(),
            lpszClassName: OWNER_CLASS,
            ..Default::default()
        });
        RegisterClassW(&WNDCLASSW {
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(popup_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: POPUP_CLASS,
            ..Default::default()
        });
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(popup_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: LAYERED_CLASS,
            ..Default::default()
        });
        let Ok(owner) = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            OWNER_CLASS,
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
        ) else {
            return;
        };
        if let Some(s) = shared() {
            s.owner.store(owner.0 as isize, Ordering::SeqCst);
        }
        let _ = ready.send(GetCurrentThreadId());
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        close_menu();
        HOOKS.with(|h| {
            if let Some(h) = h.borrow_mut().take() {
                let _ = UnhookWindowsHookEx(h.mouse);
                let _ = UnhookWindowsHookEx(h.keyboard);
            }
        });
        let _ = DestroyWindow(owner);
    }
}
