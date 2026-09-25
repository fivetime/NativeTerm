//! Other windows' places on the screen, for docking NativeTerm's window
//! to the screen's edge: Windows has it all; on X11 the same questions
//! go to the X server (the frame the window manager put around the
//! window, the work area it publishes, the pointer), on macOS to AppKit;
//! on Wayland every question answers "not known", so nothing docks.
//!
//! Coordinates are physical pixels of the whole screen. On X11 and macOS
//! `window_bounds` and `frame_bounds` are the same rectangle — the frame
//! the window manager drew, which is also what `move_window` places (a
//! window's requested position is its frame's corner, ICCCM's north-west
//! gravity) — so the docking geometry's inset for Windows' invisible
//! borders is zero.

#[cfg(windows)]
pub use native_term_win::dock::{
    cursor, frame_bounds, monitor_bounds, mouse_button_down, move_window, on_a_monitor, round_corners, set_topmost,
    window_bounds, work_area, work_area_at, Bounds,
};

/// Keep a floating window of ours in sight over another application's
/// full-screen window. Only macOS needs telling: a full-screen window
/// there gets a Space of its own, where another application's windows
/// are shown only if they may join every Space and stand beside a
/// full-screen window (`NSWindowCollectionBehavior`: `CanJoinAllSpaces`,
/// `FullScreenAuxiliary`) — so the docked window, its strip and the
/// floating button ask for that, and a window undocked again gives it
/// up (an ordinary window belongs to one Space). Windows and X11 have
/// no such Spaces: a topmost window is in sight over a full-screen one.
///
/// That behaviour alone is not enough: macOS (13.5, measured) shows a
/// window of a regular application (one with a Dock icon) in another
/// application's full-screen Space only if the window was *made* while
/// the application was an accessory (no Dock icon) — whatever its level,
/// and even when the application is regular again by the time the
/// window is ordered in. So NativeTerm starts as an accessory, makes
/// its windows, and becomes regular (`regular_application`) before it
/// shows them; a window made after that is an ordinary one.
#[cfg(not(target_os = "macos"))]
pub fn over_fullscreen(_handle: isize, _on: bool) {}

/// See `over_fullscreen`: the application takes its Dock icon and its
/// place in Cmd-Tab, once its floating windows are made. Nothing to do
/// elsewhere.
#[cfg(not(target_os = "macos"))]
pub fn regular_application() {}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[cfg(unix)]
impl Bounds {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod x11 {
    use std::sync::OnceLock;

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        Atom, AtomEnum, ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt, KeyButMask, Window,
    };
    use x11rb::rust_connection::RustConnection;

    use super::Bounds;

    /// The X server, connected once for the process; `None` where there
    /// is none (Wayland without XWayland, no desktop at all).
    struct Display {
        conn: RustConnection,
        root: Window,
        width: i32,
        height: i32,
        workarea: Atom,
        current_desktop: Atom,
        frame_extents: Atom,
    }

    static DISPLAY: OnceLock<Option<Display>> = OnceLock::new();

    fn display() -> Option<&'static Display> {
        DISPLAY
            .get_or_init(|| {
                // an X11 session, or Wayland with XWayland for the window:
                // the handle winit gives is an X window either way
                let (conn, screen) = x11rb::connect(None).ok()?;
                let screen = &conn.setup().roots[screen];
                let (root, width, height) =
                    (screen.root, i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
                let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom);
                let workarea = atom("_NET_WORKAREA")?;
                let current_desktop = atom("_NET_CURRENT_DESKTOP")?;
                let frame_extents = atom("_NET_FRAME_EXTENTS")?;
                Some(Display { conn, root, width, height, workarea, current_desktop, frame_extents })
            })
            .as_ref()
    }

    fn window(handle: isize) -> Option<Window> {
        u32::try_from(handle).ok().filter(|w| *w != 0)
    }

    /// A window's CARDINAL property, up to `count` values.
    fn cardinals(d: &Display, window: Window, property: Atom, count: u32) -> Option<Vec<u32>> {
        let reply = d.conn.get_property(false, window, property, AtomEnum::CARDINAL, 0, count).ok()?.reply().ok()?;
        let values: Vec<u32> = reply.value32()?.collect();
        (!values.is_empty()).then_some(values)
    }

    /// The window as the window manager framed it, on the screen.
    pub fn window_bounds(handle: isize) -> Option<Bounds> {
        let d = display()?;
        let w = window(handle)?;
        let geometry = d.conn.get_geometry(w).ok()?.reply().ok()?;
        let at = d.conn.translate_coordinates(w, d.root, 0, 0).ok()?.reply().ok()?;
        let (left, top) = (i32::from(at.dst_x), i32::from(at.dst_y));
        let mut bounds =
            Bounds { left, top, right: left + i32::from(geometry.width), bottom: top + i32::from(geometry.height) };
        // the frame around it: left, right, top, bottom
        if let Some(extents) = cardinals(d, w, d.frame_extents, 4) {
            if let [l, r, t, b] = extents[..] {
                bounds.left -= l as i32;
                bounds.right += r as i32;
                bounds.top -= t as i32;
                bounds.bottom += b as i32;
            }
        }
        Some(bounds)
    }

    pub fn frame_bounds(handle: isize) -> Option<Bounds> {
        window_bounds(handle)
    }

    /// The work area (without panels) the window manager publishes for
    /// the current desktop.
    fn published_work_area() -> Option<Bounds> {
        let d = display()?;
        let desktop = cardinals(d, d.root, d.current_desktop, 1).map_or(0, |v| v[0] as usize);
        let areas = cardinals(d, d.root, d.workarea, 4 * 64)?;
        let area = areas.chunks(4).nth(desktop).or_else(|| areas.chunks(4).next())?;
        let [x, y, w, h] = area[..] else { return None };
        Some(Bounds { left: x as i32, top: y as i32, right: (x + w) as i32, bottom: (y + h) as i32 })
    }

    /// The whole screen as it is now (it changes when monitors come and go).
    fn screen(d: &Display) -> Bounds {
        match d.conn.get_geometry(d.root).ok().and_then(|c| c.reply().ok()) {
            Some(g) => Bounds { left: 0, top: 0, right: i32::from(g.width), bottom: i32::from(g.height) },
            None => Bounds { left: 0, top: 0, right: d.width, bottom: d.height },
        }
    }

    /// The monitors (RandR's), else the whole screen as one.
    fn monitors(d: &Display) -> Vec<Bounds> {
        use x11rb::protocol::randr::ConnectionExt as _;
        let listed: Vec<Bounds> = d
            .conn
            .randr_get_monitors(d.root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.monitors)
            .unwrap_or_default()
            .into_iter()
            .map(|m| Bounds {
                left: i32::from(m.x),
                top: i32::from(m.y),
                right: i32::from(m.x) + i32::from(m.width),
                bottom: i32::from(m.y) + i32::from(m.height),
            })
            .collect();
        if listed.is_empty() {
            vec![screen(d)]
        } else {
            listed
        }
    }

    /// The monitor showing a point, else the nearest one.
    fn monitor_at(d: &Display, x: i32, y: i32) -> Bounds {
        let all = monitors(d);
        let distance = |b: &Bounds| {
            let dx = (b.left - x).max(0).max(x - (b.right - 1));
            let dy = (b.top - y).max(0).max(y - (b.bottom - 1));
            i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy)
        };
        all.iter().copied().min_by_key(distance).unwrap_or_else(|| screen(d))
    }

    /// The part of the published work area on `monitor` (the window
    /// manager publishes one area spanning every monitor; its panels are
    /// taken off the monitor they are on), or the monitor itself.
    fn work_on(monitor: Bounds) -> Bounds {
        match published_work_area() {
            Some(w) => {
                let cut = Bounds {
                    left: w.left.max(monitor.left),
                    top: w.top.max(monitor.top),
                    right: w.right.min(monitor.right),
                    bottom: w.bottom.min(monitor.bottom),
                };
                if cut.width() > 0 && cut.height() > 0 {
                    cut
                } else {
                    monitor
                }
            }
            None => monitor,
        }
    }

    /// The work area of the monitor the window is mostly on.
    pub fn work_area(handle: isize) -> Option<Bounds> {
        Some(work_on(monitor_bounds(handle)?))
    }

    /// The work area of the monitor showing a point (else the nearest).
    pub fn work_area_at(x: i32, y: i32) -> Option<Bounds> {
        let d = display()?;
        Some(work_on(monitor_at(d, x, y)))
    }

    /// The monitor the window is mostly on (RandR's monitors), else the
    /// whole screen.
    pub fn monitor_bounds(handle: isize) -> Option<Bounds> {
        let d = display()?;
        let Some(w) = window_bounds(handle) else { return Some(screen(d)) };
        Some(monitor_at(d, (w.left + w.right) / 2, (w.top + w.bottom) / 2))
    }

    /// Whether a monitor shows this point (not only the screen: monitors
    /// of different sizes leave parts of the screen nobody sees).
    pub fn on_a_monitor(x: i32, y: i32) -> bool {
        display().is_some_and(|d| monitors(d).iter().any(|m| m.contains(x, y)))
    }

    pub fn cursor() -> Option<(i32, i32)> {
        let d = display()?;
        let p = d.conn.query_pointer(d.root).ok()?.reply().ok()?;
        Some((i32::from(p.root_x), i32::from(p.root_y)))
    }

    /// A mouse button is held (a drag or a click in progress).
    pub fn mouse_button_down() -> bool {
        let Some(d) = display() else { return false };
        let Ok(Ok(p)) = d.conn.query_pointer(d.root).map(|c| c.reply()) else { return false };
        p.mask.intersects(KeyButMask::BUTTON1 | KeyButMask::BUTTON2 | KeyButMask::BUTTON3)
    }

    /// The window manager draws the corners; nothing to ask for.
    pub fn round_corners(_handle: isize) {}

    /// Left to winit's window level (it would reset the state otherwise).
    pub fn set_topmost(_handle: isize, _on: bool) {}

    /// Move the window's frame to `left`, `top` without raising it.
    ///
    /// Window managers read a move request differently: by ICCCM (no
    /// gravity given means north-west) the position is the frame's, which
    /// KWin follows, while mutter (GNOME, and Pantheon's gala) takes it as
    /// the window's own. So the window is given static gravity — the
    /// position is the window's own, for every window manager — and the
    /// frame the window manager drew is added here.
    pub fn move_window(handle: isize, left: i32, top: i32) {
        let (Some(d), Some(w)) = (display(), window(handle)) else { return };
        static_gravity(d, w);
        // a window just mapped again has no frame yet: the one it had
        static SEEN: std::sync::Mutex<Vec<(Window, (i32, i32))>> = std::sync::Mutex::new(Vec::new());
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        let now = match cardinals(d, w, d.frame_extents, 4).as_deref() {
            Some([l, _, t, _]) if *l != 0 || *t != 0 => Some((*l as i32, *t as i32)),
            _ => None,
        };
        let (l, t) = match now {
            Some(extents) => {
                seen.retain(|(x, _)| *x != w);
                seen.push((w, extents));
                extents
            }
            None => seen.iter().find(|(x, _)| *x == w).map_or((0, 0), |(_, e)| *e),
        };
        drop(seen);
        let _ = d.conn.configure_window(w, &ConfigureWindowAux::new().x(left + l).y(top + t));
        let _ = d.conn.flush();
    }

    /// Give `w` static gravity, once (keeping the rest of its size hints).
    fn static_gravity(d: &Display, w: Window) {
        use x11rb::properties::WmSizeHints;
        use x11rb::protocol::xproto::Gravity;
        static DONE: std::sync::Mutex<Vec<Window>> = std::sync::Mutex::new(Vec::new());
        let mut done = DONE.lock().unwrap_or_else(|e| e.into_inner());
        if done.contains(&w) {
            return;
        }
        let mut hints =
            WmSizeHints::get_normal_hints(&d.conn, w).ok().and_then(|c| c.reply().ok()).flatten().unwrap_or_default();
        hints.win_gravity = Some(Gravity::STATIC);
        if hints.set_normal_hints(&d.conn, w).is_ok() {
            done.push(w);
        }
    }

    /// Give a window that nothing paints one plain colour (the docking
    /// strip): its background, which the server draws.
    pub fn fill(handle: isize, (r, g, b): (u8, u8, u8)) {
        let (Some(d), Some(w)) = (display(), window(handle)) else { return };
        let pixel = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
        let _ = d.conn.change_window_attributes(w, &ChangeWindowAttributesAux::new().background_pixel(pixel));
        let _ = d.conn.clear_area(false, w, 0, 0, 0, 0);
        let _ = d.conn.flush();
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub use x11::{
    cursor, fill, frame_bounds, monitor_bounds, mouse_button_down, move_window, on_a_monitor, round_corners,
    set_topmost, window_bounds, work_area, work_area_at,
};

#[cfg(target_os = "macos")]
mod mac {
    //! AppKit measures in points from the bottom-left of the primary
    //! screen; winit, and so docking, in physical pixels from its top-left.
    //! Points are turned into pixels with the primary screen's scale (on
    //! monitors of mixed scales the others are off by their ratio).

    use super::Bounds;
    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSColor, NSEvent, NSScreen, NSView, NSWindow,
        NSWindowCollectionBehavior,
    };
    use objc2_foundation::{NSPoint, NSRect};

    /// The primary screen's height in points and its scale: what turns
    /// AppKit's coordinates into ours. `None` off the main thread.
    fn space() -> Option<(f64, f64)> {
        let mtm = MainThreadMarker::new()?;
        let primary = NSScreen::screens(mtm).firstObject()?;
        Some((primary.frame().size.height, primary.backingScaleFactor()))
    }

    fn screens() -> Vec<Retained<NSScreen>> {
        MainThreadMarker::new().map(|mtm| NSScreen::screens(mtm).iter().collect()).unwrap_or_default()
    }

    /// `rect` (points, from the bottom) in pixels from the top.
    pub(super) fn to_pixels(rect: NSRect, (height, scale): (f64, f64)) -> Bounds {
        let px = |v: f64| (v * scale).round() as i32;
        Bounds {
            left: px(rect.origin.x),
            top: px(height - rect.origin.y - rect.size.height),
            right: px(rect.origin.x + rect.size.width),
            bottom: px(height - rect.origin.y),
        }
    }

    /// The window of the view winit handed out as the window's handle.
    fn window(handle: isize) -> Option<Retained<NSWindow>> {
        MainThreadMarker::new()?;
        if handle == 0 {
            return None;
        }
        // SAFETY: `handle` is the `NSView` of a window of ours that is
        // still open (winit's AppKit handle), used on the main thread.
        let view: &NSView = unsafe { &*(handle as *const NSView) };
        view.window()
    }

    /// The window's frame, title bar included: what `move_window` places.
    pub fn window_bounds(handle: isize) -> Option<Bounds> {
        Some(to_pixels(window(handle)?.frame(), space()?))
    }

    pub fn frame_bounds(handle: isize) -> Option<Bounds> {
        window_bounds(handle)
    }

    /// The screen the window is mostly on, its menu bar and Dock left out.
    pub fn work_area(handle: isize) -> Option<Bounds> {
        Some(to_pixels(window(handle)?.screen()?.visibleFrame(), space()?))
    }

    /// The screen showing a point (else the nearest), menu bar and Dock
    /// left out.
    pub fn work_area_at(x: i32, y: i32) -> Option<Bounds> {
        let space = space()?;
        let distance = |b: &Bounds| {
            let dx = i64::from((b.left - x).max(0).max(x - (b.right - 1)));
            let dy = i64::from((b.top - y).max(0).max(y - (b.bottom - 1)));
            dx * dx + dy * dy
        };
        let screen = screens().into_iter().min_by_key(|s| distance(&to_pixels(s.frame(), space)))?;
        Some(to_pixels(screen.visibleFrame(), space))
    }

    pub fn monitor_bounds(handle: isize) -> Option<Bounds> {
        Some(to_pixels(window(handle)?.screen()?.frame(), space()?))
    }

    pub fn on_a_monitor(x: i32, y: i32) -> bool {
        let Some(space) = space() else { return true };
        screens().iter().any(|s| to_pixels(s.frame(), space).contains(x, y))
    }

    /// The pointer's pixel. AppKit's pointer reaches a screen's far edges
    /// themselves (x = its width at the right edge), one past its last
    /// pixel: it is kept on the screen it is at.
    pub fn cursor() -> Option<(i32, i32)> {
        let space = space()?;
        let at: NSPoint = NSEvent::mouseLocation();
        let (x, y) = ((at.x * space.1).floor() as i32, ((space.0 - at.y) * space.1).floor() as i32);
        let screens: Vec<Bounds> = screens().iter().map(|s| to_pixels(s.frame(), space)).collect();
        if screens.iter().any(|s| s.contains(x, y)) {
            return Some((x, y));
        }
        Some(screens.iter().find_map(|s| on_edge(*s, x, y)).unwrap_or((x, y)))
    }

    /// `(x, y)` moved onto `screen` when it is on its right or bottom edge.
    pub(super) fn on_edge(screen: Bounds, x: i32, y: i32) -> Option<(i32, i32)> {
        let (cx, cy) = (x.min(screen.right - 1), y.min(screen.bottom - 1));
        (screen.contains(cx, cy) && x <= screen.right && y <= screen.bottom).then_some((cx, cy))
    }

    /// A mouse button is held (a drag or a click in progress).
    pub fn mouse_button_down() -> bool {
        NSEvent::pressedMouseButtons() != 0
    }

    /// AppKit draws the corners.
    pub fn round_corners(_handle: isize) {}

    /// Left to winit's window level (it would reset the state otherwise).
    pub fn set_topmost(_handle: isize, _on: bool) {}

    /// See the crate-level `regular_application`.
    pub fn regular_application() {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let app = NSApplication::sharedApplication(mtm);
        if app.activationPolicy() != NSApplicationActivationPolicy::Regular {
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        }
    }

    /// See the crate-level `over_fullscreen`: the window may join every
    /// Space and stand beside a full-screen window, or no longer may.
    pub fn over_fullscreen(handle: isize, on: bool) {
        let Some(w) = window(handle) else { return };
        let asked = NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::FullScreenAuxiliary;
        let was = w.collectionBehavior();
        let now = if on { was | asked } else { was & !asked };
        if now != was {
            w.setCollectionBehavior(now);
        }
    }

    /// Move the window's frame (title bar included) to `left`, `top`.
    pub fn move_window(handle: isize, left: i32, top: i32) {
        let (Some(w), Some((height, scale))) = (window(handle), space()) else { return };
        w.setFrameTopLeftPoint(NSPoint::new(f64::from(left) / scale, height - f64::from(top) / scale));
    }

    /// Give a window that nothing paints one plain colour (the docking
    /// strip): its background.
    pub fn fill(handle: isize, (r, g, b): (u8, u8, u8)) {
        let Some(w) = window(handle) else { return };
        let c = |v: u8| f64::from(v) / 255.0;
        w.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(c(r), c(g), c(b), 1.0)));
    }

    #[cfg(test)]
    mod tests {
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        #[test]
        fn points_from_the_bottom_become_pixels_from_the_top() {
            // a 1920×1200-point Retina screen, a window 100 points from
            // its left and 200 from its top
            let window = NSRect::new(NSPoint::new(100.0, 1200.0 - 200.0 - 300.0), NSSize::new(400.0, 300.0));
            let b = super::to_pixels(window, (1200.0, 2.0));
            assert_eq!((b.left, b.top, b.right, b.bottom), (200, 400, 1000, 1000));
        }

        #[test]
        fn the_far_edges_are_the_last_pixels() {
            let screen = super::Bounds { left: 0, top: 0, right: 3840, bottom: 2400 };
            assert_eq!(super::on_edge(screen, 3840, 381), Some((3839, 381)));
            assert_eq!(super::on_edge(screen, 500, 2400), Some((500, 2399)));
            assert_eq!(super::on_edge(screen, 3841, 381), None);
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::{
    cursor, fill, frame_bounds, monitor_bounds, mouse_button_down, move_window, on_a_monitor, over_fullscreen,
    regular_application, round_corners, set_topmost, window_bounds, work_area, work_area_at,
};
