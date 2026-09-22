//! Other windows' places on the screen, for docking NativeTerm's window
//! to the screen's edge: Windows has it all; on X11 the same questions
//! go to the X server (the frame the window manager put around the
//! window, the work area it publishes, the pointer); on Wayland and
//! macOS every question answers "not known", so nothing docks.
//!
//! Coordinates are physical pixels of the whole screen. On X11
//! `window_bounds` and `frame_bounds` are the same rectangle — the frame
//! the window manager drew, which is also what `move_window` places (a
//! window's requested position is its frame's corner, ICCCM's north-west
//! gravity) — so the docking geometry's inset for Windows' invisible
//! borders is zero.

#[cfg(windows)]
pub use native_term_win::dock::{
    cursor, frame_bounds, mouse_button_down, move_window, on_a_monitor, round_corners, set_topmost, window_bounds,
    work_area, work_area_at, Bounds,
};

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

    /// The work area, or the whole screen where none is published.
    pub fn work_area(_handle: isize) -> Option<Bounds> {
        published_work_area().or_else(|| {
            let d = display()?;
            Some(Bounds { left: 0, top: 0, right: d.width, bottom: d.height })
        })
    }

    pub fn work_area_at(_x: i32, _y: i32) -> Option<Bounds> {
        work_area(0)
    }

    /// Whether the screen shows this point (one X screen spans every
    /// monitor, so an edge with another monitor behind it is not told
    /// apart here).
    pub fn on_a_monitor(x: i32, y: i32) -> bool {
        display().is_some_and(|d| (0..d.width).contains(&x) && (0..d.height).contains(&y))
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
    pub fn move_window(handle: isize, left: i32, top: i32) {
        let (Some(d), Some(w)) = (display(), window(handle)) else { return };
        let _ = d.conn.configure_window(w, &ConfigureWindowAux::new().x(left).y(top));
        let _ = d.conn.flush();
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
    cursor, fill, frame_bounds, mouse_button_down, move_window, on_a_monitor, round_corners, set_topmost,
    window_bounds, work_area, work_area_at,
};

#[cfg(target_os = "macos")]
mod none {
    use super::Bounds;

    pub fn window_bounds(_handle: isize) -> Option<Bounds> {
        None
    }

    pub fn frame_bounds(_handle: isize) -> Option<Bounds> {
        None
    }

    pub fn work_area(_handle: isize) -> Option<Bounds> {
        None
    }

    pub fn work_area_at(_x: i32, _y: i32) -> Option<Bounds> {
        None
    }

    pub fn on_a_monitor(_x: i32, _y: i32) -> bool {
        true
    }

    pub fn cursor() -> Option<(i32, i32)> {
        None
    }

    pub fn mouse_button_down() -> bool {
        false
    }

    pub fn round_corners(_handle: isize) {}

    pub fn set_topmost(_handle: isize, _on: bool) {}

    pub fn move_window(_handle: isize, _left: i32, _top: i32) {}

    pub fn fill(_handle: isize, _rgb: (u8, u8, u8)) {}
}

#[cfg(target_os = "macos")]
pub use none::{
    cursor, fill, frame_bounds, mouse_button_down, move_window, on_a_monitor, round_corners, set_topmost,
    window_bounds, work_area, work_area_at,
};
