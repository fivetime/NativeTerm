//! What NativeTerm's windows ask of each other: the floating button asks
//! for the main window (and its tab list), and reads the host list the
//! main window loaded.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use native_term_app::tab_menu::MenuRequest;
use native_term_app::HostRequest;

static SHOW_MAIN: AtomicBool = AtomicBool::new(false);
static SHOW_TABS: AtomicBool = AtomicBool::new(false);
static HOSTS: Mutex<Vec<HostEntry>> = Mutex::new(Vec::new());
static REQUESTS: Mutex<Vec<MenuRequest>> = Mutex::new(Vec::new());

/// What the tab menu asked for (its dialogs live in the main window).
pub fn ask(request: MenuRequest) {
    REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).push(request);
    show_main();
}

pub fn take_requests() -> Vec<MenuRequest> {
    std::mem::take(&mut *REQUESTS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// A saved host, for the floating button's search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEntry {
    pub alias: String,
    pub label: String,
    pub hostname: String,
    pub folder: String,
    pub on_login: Option<String>,
}

impl HostEntry {
    pub fn request(&self) -> HostRequest {
        HostRequest { on_login: self.on_login.clone(), ..HostRequest::new(&self.alias, &self.label) }
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
