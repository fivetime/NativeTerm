//! OS-specific control of the native terminal's tabs: NativeTerm's SSH
//! sessions are opened as tabs in the user's own terminal window. See
//! `docs/ARCHITECTURE.md`.

#[cfg(windows)]
mod windows_backend;
#[cfg(windows)]
pub use windows_backend::{WindowsTerminalBackend as DefaultBackend, DEDICATED_WINDOW_NAME};

/// A tab as seen from outside the terminal. NativeTerm's own tabs are
/// identified by their fixed, unique title; `runtime_id` is only valid right
/// after the lookup (Windows Terminal recycles tab elements when scrolling).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabInfo {
    pub runtime_id: Vec<i32>,
    pub title: String,
    pub index: usize,
    pub selected: bool,
}

pub trait TerminalBackend {
    /// Open a new tab in the user's current terminal window, titled `title`,
    /// with the given terminal session GUID, running `nativeterm-shim <host_alias>`.
    fn open_tab(&self, session_id: &str, title: &str, host_alias: &str) -> std::io::Result<()>;

    /// Current tabs of all terminal windows, in on-screen order.
    fn list_tabs(&self) -> std::io::Result<Vec<TabInfo>>;

    fn focus_tab(&self, title: &str) -> std::io::Result<()>;

    fn close_tab(&self, title: &str) -> std::io::Result<()>;
}
