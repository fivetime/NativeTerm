//! Other windows' places on the screen, for docking NativeTerm's window
//! to the terminal's: only Windows has this; elsewhere every question
//! answers "not known", so nothing docks.

#[cfg(windows)]
pub use native_term_win::dock::{
    cursor, frame_bounds, mouse_button_down, move_window, on_a_monitor, round_corners, set_topmost, window_bounds,
    work_area, work_area_at, Bounds,
};

#[cfg(unix)]
mod unix {
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
}

#[cfg(unix)]
pub use unix::{
    cursor, frame_bounds, mouse_button_down, move_window, on_a_monitor, round_corners, set_topmost, window_bounds,
    work_area, work_area_at, Bounds,
};
