//! iTerm2 driven from outside through its scripting (JavaScript for
//! Automation, run with `osascript`): a session tab is a tab created with
//! the shim as its command and its session named with the label (which
//! the claimer's first rule finds); one script lists every window, tab
//! and session; there are no events, so a subscription polls. Text is
//! typed with `write`, screens read with `contents`, a window brought
//! forward with `select` and `activate`. Nothing is pictured and no menu
//! is drawn over the tab strip.
//!
//! The first script run asks the person whether NativeTerm may control
//! iTerm2 (Automation permission), which macOS remembers per app bundle.
//! The scripts are pure and tested anywhere; running them needs macOS.

pub mod jxa;

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use native_term_platform::claim::Claimer;
use native_term_platform::{
    Capabilities, Change, ChangeCounts, Notify, OpenReport, Snapshot, Subscription, TabSpec, TabView, Target,
    TerminalBackend, WindowId, WindowView,
};

use jxa::{Listing, Window};

/// How often a subscription looks for changes.
const POLL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct State {
    /// What `Target::Recent` means: the window last activated or made.
    recent: Option<u64>,
    /// `Target::Named` windows, for as long as this process runs.
    named: HashMap<String, u64>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run a JXA script with `osascript`; what it returned, or what it said.
fn osascript(script: &str) -> io::Result<String> {
    let mut child = Command::new("osascript")
        .args(["-l", "JavaScript"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said.lines().last().unwrap_or("").trim().to_string();
        Err(io::Error::other(format!("osascript {}: {said}", output.status)))
    }
}

/// iTerm2, through its scripting.
pub struct ITerm2 {
    shim: PathBuf,
    shim_args: Vec<OsString>,
    claimer: Mutex<Claimer>,
    state: Mutex<State>,
}

impl ITerm2 {
    /// Tabs run `shim`.
    pub fn new(shim: &Path) -> ITerm2 {
        ITerm2 {
            shim: shim.to_path_buf(),
            shim_args: Vec::new(),
            claimer: Mutex::new(Claimer::new()),
            state: Mutex::default(),
        }
    }

    /// Session tabs look sessions up in `ssh_dir` instead of `~/.ssh`.
    pub fn with_ssh_dir(mut self, ssh_dir: &Path) -> ITerm2 {
        self.shim_args = vec!["--ssh-dir".into(), ssh_dir.into()];
        self
    }

    /// Put NativeTerm's profile in iTerm2's dynamic profiles folder
    /// (`~/Library/Application Support/iTerm2/DynamicProfiles`), which
    /// iTerm2 watches; unchanged, the file is left alone. Tabs open with
    /// the default profile while it is missing.
    pub fn install_profile() -> io::Result<()> {
        let home = std::env::var_os("HOME").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no $HOME"))?;
        let dir = Path::new(&home).join("Library/Application Support/iTerm2/DynamicProfiles");
        let file = dir.join("nativeterm.json");
        let profile = jxa::dynamic_profile();
        if std::fs::read_to_string(&file).is_ok_and(|now| now == profile) {
            return Ok(());
        }
        std::fs::create_dir_all(&dir)?;
        std::fs::write(file, profile)
    }

    /// Whether iTerm2 is installed here (its application bundle).
    pub fn available() -> bool {
        Path::new("/Applications/iTerm.app").is_dir()
            || std::env::var_os("HOME").is_some_and(|h| Path::new(&h).join("Applications/iTerm.app").is_dir())
    }

    fn listing(&self) -> Listing {
        osascript(&jxa::list_script()).ok().and_then(|json| jxa::parse_listing(&json).ok()).unwrap_or_default()
    }

    fn window(&self, id: u64) -> Option<Window> {
        self.listing().windows.into_iter().find(|w| w.id == id)
    }

    fn recent(&self, listing: &Listing) -> Option<u64> {
        let recent = lock(&self.state).recent;
        recent
            .filter(|id| listing.windows.iter().any(|w| w.id == *id))
            .or(listing.current_window)
            .or_else(|| listing.windows.first().map(|w| w.id))
    }

    fn new_window(&self, command: &str, name: &str) -> io::Result<u64> {
        let out = osascript(&jxa::new_window_script(command, name))?;
        out.trim().parse().map_err(|_| io::Error::other(format!("iTerm2 said {:?}, not a window id", out.trim())))
    }

    fn new_tab(&self, window: u64, command: &str, name: &str) -> io::Result<()> {
        osascript(&jxa::new_tab_script(window, command, name)).map(|_| ())
    }

    /// The tab at `tab`'s place, if it is still what the snapshot saw.
    fn find_tab(&self, window: WindowId, tab: &TabView) -> io::Result<Option<usize>> {
        let w = self.window(window.0).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such window"))?;
        Ok(w.tab_named(tab.index, &tab.name).map(|_| tab.index))
    }
}

struct Poller {
    stop: Arc<AtomicBool>,
    windows: Arc<AtomicU64>,
    tabs: Arc<AtomicU64>,
}

impl Subscription for Poller {
    fn counts(&self) -> ChangeCounts {
        ChangeCounts { windows: self.windows.load(Ordering::Relaxed), tabs: self.tabs.load(Ordering::Relaxed) }
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// One tab as a poll sees it: window, name, current.
type TabShape = (u64, String, bool);

/// What a poll compares: the windows, the tabs, and which window is in front.
fn shape(listing: &Listing) -> (Vec<u64>, Vec<TabShape>, Option<u64>) {
    let ids = listing.windows.iter().map(|w| w.id).collect();
    let tabs = listing.windows.iter().flat_map(|w| w.tabs.iter().map(move |t| (w.id, t.name(), t.current))).collect();
    (ids, tabs, listing.frontmost.then_some(listing.current_window).flatten())
}

impl TerminalBackend for ITerm2 {
    fn name(&self) -> &'static str {
        "iTerm2"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { screen_text: true, type_text: true, ..Capabilities::default() }
    }

    fn shim_path(&self) -> &Path {
        &self.shim
    }

    fn window_ids(&self) -> Vec<WindowId> {
        self.listing().windows.iter().map(Window::window_id).collect()
    }

    fn foreground(&self) -> Option<WindowId> {
        let listing = self.listing();
        listing.frontmost.then_some(listing.current_window).flatten().map(WindowId)
    }

    fn activate(&self, window: WindowId) -> bool {
        if self.window(window.0).is_none() {
            return false;
        }
        lock(&self.state).recent = Some(window.0);
        osascript(&jxa::activate_script(window.0)).is_ok()
    }

    fn snapshot(&self, labels: &HashSet<String>) -> Snapshot {
        let listing = self.listing();
        let front = listing.frontmost.then_some(listing.current_window).flatten();
        let mut claimer = lock(&self.claimer);
        let windows = listing
            .windows
            .iter()
            .map(|w| WindowView {
                handle: w.window_id(),
                pid: 0,
                foreground: front == Some(w.id),
                unresponsive: false,
                tabs: claimer.claim(w.window_id(), &w.as_window_tabs(), labels),
            })
            .collect();
        claimer.retain(&listing.windows.iter().map(Window::window_id).collect::<Vec<_>>());
        Snapshot { windows, complete: true }
    }

    fn open(&self, target: &Target, tabs: &[TabSpec]) -> io::Result<OpenReport> {
        let Some(first) = tabs.first() else { return Ok(OpenReport::default()) };
        let shim = self.shim.to_string_lossy();
        let commands: Vec<String> = tabs.iter().map(|t| jxa::shim_command(&shim, &self.shim_args, t)).collect();
        let listing = self.listing();
        let (window, made) = match target {
            Target::NewWindow => (self.new_window(&commands[0], &first.label)?, true),
            Target::Recent => match self.recent(&listing) {
                Some(w) => (w, false),
                None => (self.new_window(&commands[0], &first.label)?, true),
            },
            Target::Named(name) => {
                let known = lock(&self.state).named.get(name).copied();
                match known.filter(|id| listing.windows.iter().any(|w| w.id == *id)) {
                    Some(w) => (w, false),
                    None => {
                        let w = self.new_window(&commands[0], &first.label)?;
                        lock(&self.state).named.insert(name.clone(), w);
                        (w, true)
                    }
                }
            }
        };
        lock(&self.state).recent = Some(window);
        let mut launched = usize::from(made);
        for (tab, command) in tabs.iter().zip(&commands).skip(launched) {
            self.new_tab(window, command, &tab.label)?;
            launched += 1;
        }
        Ok(OpenReport { launched, pending: Vec::new(), window: made.then_some(WindowId(window)) })
    }

    fn open_tool(&self, title: &str, shim_args: &[String]) -> io::Result<()> {
        let command = jxa::tool_command(&self.shim.to_string_lossy(), shim_args);
        let listing = self.listing();
        match self.recent(&listing) {
            Some(w) => self.new_tab(w, &command, title),
            None => self.new_window(&command, title).map(|_| ()),
        }
    }

    fn select(&self, window: WindowId, tab: &TabView) -> io::Result<bool> {
        let Some(index) = self.find_tab(window, tab)? else { return Ok(false) };
        osascript(&jxa::select_tab_script(window.0, index))?;
        lock(&self.state).recent = Some(window.0);
        Ok(true)
    }

    fn close(&self, window: WindowId, tab: &TabView) -> io::Result<bool> {
        let Some(index) = self.find_tab(window, tab)? else { return Ok(false) };
        osascript(&jxa::close_tab_script(window.0, index)).map(|_| true)
    }

    fn subscribe(&self, notify: Notify) -> Box<dyn Subscription> {
        let poller = Poller { stop: Arc::default(), windows: Arc::default(), tabs: Arc::default() };
        let (stop, windows, tabs) = (Arc::clone(&poller.stop), Arc::clone(&poller.windows), Arc::clone(&poller.tabs));
        std::thread::Builder::new()
            .name("iterm2-poll".into())
            .spawn(move || {
                let list = || {
                    osascript(&jxa::list_script()).ok().and_then(|j| jxa::parse_listing(&j).ok()).unwrap_or_default()
                };
                let mut last = shape(&list());
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(POLL);
                    let now = shape(&list());
                    if now.0 != last.0 {
                        windows.fetch_add(1, Ordering::Relaxed);
                        notify(Change::Windows);
                    } else if now.1 != last.1 {
                        tabs.fetch_add(1, Ordering::Relaxed);
                        notify(Change::Tabs);
                    } else if now.2 != last.2 {
                        notify(Change::Foreground);
                    }
                    last = now;
                }
            })
            .expect("a thread");
        Box::new(poller)
    }

    fn screen_text(&self, window: WindowId, max_lines: usize) -> Option<Vec<String>> {
        let text = osascript(&jxa::contents_script(window.0)).ok()?;
        Some(jxa::screen_lines(&text, max_lines))
    }

    fn type_text(&self, window: WindowId, tab: &TabView, text: &str) -> io::Result<()> {
        let Some(index) = self.find_tab(window, tab)? else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "the tab moved"));
        };
        osascript(&jxa::write_script(window.0, index, text)).map(|_| ())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shim built next to this test binary (`<target>/debug`), wherever
    /// the target folder is.
    fn built_shim() -> PathBuf {
        let exe = std::env::current_exe().expect("this test binary");
        let debug = exe.parent().and_then(Path::parent).expect("target/debug/deps/<test>");
        debug.join(format!("nativeterm-shim{}", std::env::consts::EXE_SUFFIX))
    }

    /// Against a running iTerm2 on a Mac, with the shim built. The first
    /// run asks for Automation permission.
    #[test]
    #[ignore = "needs macOS with iTerm2 and a built shim"]
    fn meets_the_contract() {
        let shim = built_shim();
        assert!(shim.exists(), "build the shim first");
        assert!(ITerm2::available(), "no iTerm2 here");
        ITerm2::install_profile().unwrap();
        native_term_platform::contract::exercise(&ITerm2::new(&shim), ["nt-iterm a", "nt-iterm b"]);
    }

    #[test]
    fn a_change_of_shape_is_a_change() {
        let one = jxa::parse_listing(
            r#"{"windows":[{"id":1,"tabs":[{"current":true,"sessions":[{"id":"a","name":"x","current":true}]}]}],"frontmost":true,"current_window":1}"#,
        )
        .unwrap();
        let behind = Listing { frontmost: false, ..one.clone() };
        assert_eq!(shape(&one).0, shape(&behind).0);
        assert_eq!(shape(&one).1, shape(&behind).1);
        assert_ne!(shape(&one).2, shape(&behind).2, "the front window changed");
        let w = ITerm2::new(Path::new("/opt/nt/shim"));
        assert!(w.capabilities().type_text && !w.capabilities().capture);
    }
}
