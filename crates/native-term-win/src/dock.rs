//! What a docked window needs from Windows: the monitor's work area, the
//! visible frame (without the invisible resize borders), the cursor, and
//! "always on top". Coordinates are physical pixels (the process is
//! per-monitor DPI aware).

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowRect, SetWindowPos, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

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

impl From<RECT> for Bounds {
    fn from(r: RECT) -> Self {
        Bounds { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
    }
}

pub(crate) fn hwnd(handle: isize) -> HWND {
    HWND(handle as *mut _)
}

/// The window rectangle, invisible resize borders included.
pub fn window_bounds(handle: isize) -> Option<Bounds> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd(handle), &mut r) }.ok().map(|_| r.into())
}

/// What the user sees of the window (DWM's frame bounds).
pub fn frame_bounds(handle: isize) -> Option<Bounds> {
    let mut r = RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            hwnd(handle),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut r as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .ok()
    .map(|_| r.into())
    .or_else(|| window_bounds(handle))
}

fn monitor_info(monitor: windows::Win32::Graphics::Gdi::HMONITOR) -> Option<(Bounds, Bounds)> {
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool().then(|| (info.rcMonitor.into(), info.rcWork.into()))
}

/// The work area (without the taskbar) of the window's monitor.
pub fn work_area(handle: isize) -> Option<Bounds> {
    monitor_info(unsafe { MonitorFromWindow(hwnd(handle), MONITOR_DEFAULTTONEAREST) }).map(|(_, work)| work)
}

/// The work area of the monitor nearest to a point.
pub fn work_area_at(x: i32, y: i32) -> Option<Bounds> {
    monitor_info(unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) }).map(|(_, work)| work)
}

/// Whether any monitor shows this point.
pub fn on_a_monitor(x: i32, y: i32) -> bool {
    !unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONULL) }.is_invalid()
}

pub fn cursor() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p) }.ok().map(|_| (p.x, p.y))
}

/// A mouse button is held (a drag or a click in progress).
pub fn mouse_button_down() -> bool {
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|vk| unsafe { GetAsyncKeyState(i32::from(vk.0)) } < 0)
}

/// Rounded corners for a window without a frame (Windows 11; Windows 10
/// ignores it).
pub fn round_corners(handle: isize) {
    let preference = DWMWCP_ROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd(handle),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&preference as *const windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE).cast(),
            std::mem::size_of_val(&preference) as u32,
        );
    }
}

pub fn set_topmost(handle: isize, on: bool) {
    let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
    unsafe {
        let _ = SetWindowPos(hwnd(handle), Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

/// Move the window (its rectangle, borders included) without activating it.
pub fn move_window(handle: isize, left: i32, top: i32) {
    unsafe {
        let _ = SetWindowPos(hwnd(handle), None, left, top, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds() {
        let b = Bounds { left: 10, top: 20, right: 110, bottom: 220 };
        assert_eq!((b.width(), b.height()), (100, 200));
        assert!(b.contains(10, 20) && !b.contains(110, 20));
    }

    #[test]
    fn the_desktop_has_a_work_area() {
        let work = work_area_at(0, 0).expect("a monitor");
        assert!(work.width() > 0 && work.height() > 0);
    }
}
