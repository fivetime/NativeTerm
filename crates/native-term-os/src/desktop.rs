//! The desktop's look and other programs' windows.

#[cfg(windows)]
pub use native_term_win::desktop::{
    accent, animations, bring_to_front, close_window, message_box, user_registry_dword, user_registry_string,
    windows_of_other_instances,
};

#[cfg(unix)]
mod unix {
    use std::path::Path;

    /// The desktop's accent colour, if it publishes one (as the last
    /// `appearance::refresh` read it).
    #[must_use]
    pub fn accent() -> Option<(u8, u8, u8)> {
        crate::appearance::cached().accent
    }

    /// Whether things may move and fade. No desktop-wide switch is read
    /// here yet, so they may.
    #[must_use]
    pub fn animations() -> bool {
        true
    }

    /// Windows of other processes running `image`: not known here.
    pub fn windows_of_other_instances(_image: &Path) -> Vec<isize> {
        Vec::new()
    }

    pub fn bring_to_front(_handle: isize) -> bool {
        false
    }

    pub fn close_window(_handle: isize) -> bool {
        false
    }

    /// An error for a program without a window: on the terminal it was
    /// started from.
    pub fn message_box(title: &str, text: &str) {
        eprintln!("{title}: {text}");
    }
}

#[cfg(unix)]
pub use unix::{accent, animations, bring_to_front, close_window, message_box, windows_of_other_instances};
