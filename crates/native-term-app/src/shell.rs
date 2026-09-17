//! What NativeTerm's windows ask of each other: the floating button asks
//! for the main window (and its tab list), and reads the host list the
//! main window loaded.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use native_term_app::HostRequest;

static SHOW_MAIN: AtomicBool = AtomicBool::new(false);
static SHOW_TABS: AtomicBool = AtomicBool::new(false);
static HOSTS: Mutex<Vec<HostEntry>> = Mutex::new(Vec::new());
static SEND_TO: Mutex<Option<String>> = Mutex::new(None);

/// Open the send dialog for a session (from the tab menu).
pub fn send_to(session: &str) {
    *SEND_TO.lock().unwrap_or_else(|e| e.into_inner()) = Some(session.to_string());
    show_main();
}

pub fn take_send_to() -> Option<String> {
    SEND_TO.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// A saved host, for the floating button's search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEntry {
    pub alias: String,
    pub label: String,
    pub hostname: String,
    pub folder: String,
}

impl HostEntry {
    pub fn request(&self) -> HostRequest {
        HostRequest::new(&self.alias, &self.label)
    }
}

/// Bring the main window out (it may be docked and hidden).
pub fn show_main() {
    SHOW_MAIN.store(true, Ordering::Relaxed);
}

pub fn take_show_main() -> bool {
    SHOW_MAIN.swap(false, Ordering::Relaxed)
}

/// Show the tab list in the main window.
pub fn show_tabs() {
    SHOW_TABS.store(true, Ordering::Relaxed);
    show_main();
}

pub fn take_show_tabs() -> bool {
    SHOW_TABS.swap(false, Ordering::Relaxed)
}

pub fn set_hosts(hosts: Vec<HostEntry>) {
    *HOSTS.lock().unwrap_or_else(|e| e.into_inner()) = hosts;
}

pub fn hosts() -> Vec<HostEntry> {
    HOSTS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
