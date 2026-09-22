//! WezTerm driven from outside through `wezterm cli`: tabs are spawned
//! with the shim as their program and titled with their label (which the
//! claimer's first rule finds), `list --format json` is the whole state,
//! and there are no events, so a subscription polls it. Text is typed
//! with `send-text` and screens read with `get-text`; nothing is
//! pictured, no menu is drawn over the tab strip, and a window can't be
//! brought forward by the CLI (a pane can be focused, which is what
//! `activate` does).
//!
//! WezTerm runs on Linux, macOS and Windows alike; this crate builds on
//! all three.

pub mod cli;

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use native_term_platform::claim::Claimer;
use native_term_platform::{
    Capabilities, Change, ChangeCounts, Notify, OpenReport, Snapshot, Subscription, TabSpec, TabView, Target,
    TerminalBackend, WindowId, WindowView,
};

use cli::{Into, ListedWindow};

/// How long a new window may take to show up in `list`.
const NEW_WINDOW_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the GUI may take to start and answer the CLI.
const START_TIMEOUT: Duration = Duration::from_secs(20);
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

/// WezTerm, through its command line.
pub struct WezTerm {
    /// `wezterm` (the CLI).
    exe: PathBuf,
    /// `wezterm-gui`, started when no WezTerm is running.
    gui: PathBuf,
    shim: PathBuf,
    shim_args: Vec<OsString>,
    claimer: Mutex<Claimer>,
    state: Mutex<State>,
}

impl WezTerm {
    /// WezTerm from `dir` (its `wezterm` and `wezterm-gui` executables),
    /// or the ones on `PATH` when `dir` is `None`; tabs run `shim`.
    pub fn new(dir: Option<&Path>, shim: &Path) -> WezTerm {
        let exe = |name: &str| {
            let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
            dir.map_or_else(|| PathBuf::from(&file), |d| d.join(&file))
        };
        WezTerm {
            exe: exe("wezterm"),
            gui: exe("wezterm-gui"),
            shim: shim.to_path_buf(),
            shim_args: Vec::new(),
            claimer: Mutex::new(Claimer::new()),
            state: Mutex::new(State::default()),
        }
    }

    /// Session tabs look sessions up in `ssh_dir` instead of `~/.ssh`.
    pub fn with_ssh_dir(mut self, ssh_dir: &Path) -> WezTerm {
        self.shim_args = vec!["--ssh-dir".into(), ssh_dir.into()];
        self
    }

    /// Whether a `wezterm` executable is where this looks for it.
    pub fn available(&self) -> bool {
        Command::new(&self.exe).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
    }

    /// Run `wezterm <args>`; its stdout, or what it said on stderr.
    fn run(&self, args: &[OsString]) -> io::Result<String> {
        let output = Command::new(&self.exe).args(args).stdin(Stdio::null()).output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let said = String::from_utf8_lossy(&output.stderr);
            let said = said.lines().last().unwrap_or("").trim().to_string();
            Err(io::Error::other(format!("wezterm {}: {said}", output.status)))
        }
    }

    /// Every window and tab, or nothing when no WezTerm answers.
    fn list(&self) -> Vec<ListedWindow> {
        self.run(&cli::list_args()).ok().and_then(|json| cli::parse_list(&json).ok()).unwrap_or_default()
    }

    fn window(&self, id: u64) -> Option<ListedWindow> {
        self.list().into_iter().find(|w| w.window_id == id)
    }

    /// The window `Target::Recent` means: the one last activated or made,
    /// else the one with the active tab, else any.
    fn recent(&self, windows: &[ListedWindow]) -> Option<u64> {
        let recent = lock(&self.state).recent;
        recent
            .filter(|id| windows.iter().any(|w| w.window_id == *id))
            .or_else(|| windows.iter().find(|w| w.tabs.iter().any(|t| t.is_active())).map(|w| w.window_id))
            .or_else(|| windows.first().map(|w| w.window_id))
    }

    /// A new window running `program`: `spawn --new-window` when WezTerm
    /// runs, else the GUI started with it; the window's id once it lists.
    fn new_window(&self, program: &[OsString], before: &[ListedWindow]) -> io::Result<u64> {
        let known: HashSet<u64> = before.iter().map(|w| w.window_id).collect();
        let (pane, timeout) = if before.is_empty() && self.run(&cli::list_args()).is_err() {
            Command::new(&self.gui).args(cli::start_args(program)).stdin(Stdio::null()).spawn()?;
            (None, START_TIMEOUT)
        } else {
            let out = self.run(&cli::spawn_args(Into::NewWindow, program))?;
            (cli::parse_spawned(&out), NEW_WINDOW_TIMEOUT)
        };
        let started = Instant::now();
        loop {
            let windows = self.list();
            let found = windows.iter().find(|w| match pane {
                Some(pane) => w.tabs.iter().any(|t| t.panes.iter().any(|p| p.pane_id == pane)),
                None => !known.contains(&w.window_id),
            });
            if let Some(w) = found {
                return Ok(w.window_id);
            }
            if started.elapsed() >= timeout {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the new WezTerm window never appeared"));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// A tab running `program` in `window`, titled `title`.
    fn spawn_into(&self, window: u64, program: &[OsString], title: &str) -> io::Result<u64> {
        let out = self.run(&cli::spawn_args(Into::Window(window), program))?;
        let pane = cli::parse_spawned(&out)
            .ok_or_else(|| io::Error::other(format!("wezterm spawn said {:?}, not a pane id", out.trim())))?;
        self.run(&cli::set_tab_title_args(pane, title))?;
        Ok(pane)
    }

    /// The tab at `tab`'s place, if it is still what the snapshot saw.
    fn find_tab(&self, window: WindowId, tab: &TabView) -> io::Result<Option<cli::ListedTab>> {
        let w = self.window(window.0).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such window"))?;
        Ok(w.tab_named(tab.index, &tab.name).cloned())
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

/// One tab as a poll sees it: window, tab, name, selected.
type TabShape = (u64, u64, String, bool);

/// What a poll compares: the windows, and each tab's name and selection.
fn shape(windows: &[ListedWindow]) -> (Vec<u64>, Vec<TabShape>) {
    let ids = windows.iter().map(|w| w.window_id).collect();
    let tabs = windows
        .iter()
        .flat_map(|w| w.tabs.iter().map(move |t| (w.window_id, t.tab_id, t.name(), t.is_active())))
        .collect();
    (ids, tabs)
}

impl TerminalBackend for WezTerm {
    fn capabilities(&self) -> Capabilities {
        Capabilities { screen_text: true, type_text: true, ..Capabilities::default() }
    }

    fn shim_path(&self) -> &Path {
        &self.shim
    }

    fn window_ids(&self) -> Vec<WindowId> {
        self.list().iter().map(ListedWindow::id).collect()
    }

    /// The window last activated or made, while it exists: WezTerm's CLI
    /// doesn't say which window has the focus.
    fn foreground(&self) -> Option<WindowId> {
        let recent = lock(&self.state).recent?;
        self.window(recent).map(|w| w.id())
    }

    fn activate(&self, window: WindowId) -> bool {
        let Some(w) = self.window(window.0) else { return false };
        lock(&self.state).recent = Some(window.0);
        let pane = w.tabs.iter().find(|t| t.is_active()).or(w.tabs.first()).and_then(|t| t.active_pane());
        pane.is_some_and(|p| self.run(&cli::activate_pane_args(p)).is_ok())
    }

    fn snapshot(&self, labels: &HashSet<String>) -> Snapshot {
        let listed = self.list();
        let mut claimer = lock(&self.claimer);
        let windows = listed
            .iter()
            .map(|w| WindowView {
                handle: w.id(),
                pid: 0,
                foreground: false,
                unresponsive: false,
                tabs: claimer.claim(w.id(), &w.as_window_tabs(), labels),
            })
            .collect();
        claimer.retain(&listed.iter().map(ListedWindow::id).collect::<Vec<_>>());
        Snapshot { windows, complete: true }
    }

    fn open(&self, target: &Target, tabs: &[TabSpec]) -> io::Result<OpenReport> {
        let Some(first) = tabs.first() else { return Ok(OpenReport::default()) };
        let programs: Vec<Vec<OsString>> =
            tabs.iter().map(|t| cli::shim_program(&self.shim, &self.shim_args, t)).collect();
        let before = self.list();
        let (window, made) = match target {
            Target::NewWindow => (self.new_window(&programs[0], &before)?, true),
            Target::Recent => match self.recent(&before) {
                Some(w) => (w, false),
                None => (self.new_window(&programs[0], &before)?, true),
            },
            Target::Named(name) => {
                let known = lock(&self.state).named.get(name).copied();
                match known.filter(|id| before.iter().any(|w| w.window_id == *id)) {
                    Some(w) => (w, false),
                    None => {
                        let w = self.new_window(&programs[0], &before)?;
                        lock(&self.state).named.insert(name.clone(), w);
                        (w, true)
                    }
                }
            }
        };
        lock(&self.state).recent = Some(window);
        // a window made for the first tab already runs it, untitled yet
        let mut launched = 0;
        if made {
            let w = self.window(window).unwrap_or_default();
            if let Some(pane) = w.tabs.first().and_then(|t| t.active_pane()) {
                self.run(&cli::set_tab_title_args(pane, &first.label))?;
            }
            launched = 1;
        }
        for (tab, program) in tabs.iter().zip(&programs).skip(launched) {
            self.spawn_into(window, program, &tab.label)?;
            launched += 1;
        }
        Ok(OpenReport { launched, pending: Vec::new(), window: made.then_some(WindowId(window)) })
    }

    fn open_tool(&self, title: &str, shim_args: &[String]) -> io::Result<()> {
        let mut program: Vec<OsString> = vec![self.shim.clone().into()];
        program.extend(shim_args.iter().map(OsString::from));
        let before = self.list();
        let window = match self.recent(&before) {
            Some(w) => w,
            None => {
                let w = self.new_window(&program, &before)?;
                if let Some(pane) = self.window(w).and_then(|w| w.tabs.first().and_then(|t| t.active_pane())) {
                    self.run(&cli::set_tab_title_args(pane, title))?;
                }
                return Ok(());
            }
        };
        self.spawn_into(window, &program, title).map(|_| ())
    }

    fn select(&self, window: WindowId, tab: &TabView) -> io::Result<bool> {
        let Some(found) = self.find_tab(window, tab)? else { return Ok(false) };
        self.run(&cli::activate_tab_args(found.tab_id))?;
        lock(&self.state).recent = Some(window.0);
        Ok(true)
    }

    fn close(&self, window: WindowId, tab: &TabView) -> io::Result<bool> {
        let Some(found) = self.find_tab(window, tab)? else { return Ok(false) };
        for pane in &found.panes {
            self.run(&cli::kill_pane_args(pane.pane_id))?;
        }
        Ok(true)
    }

    fn subscribe(&self, notify: Notify) -> Box<dyn Subscription> {
        let poller = Poller { stop: Arc::default(), windows: Arc::default(), tabs: Arc::default() };
        let (stop, windows, tabs) = (Arc::clone(&poller.stop), Arc::clone(&poller.windows), Arc::clone(&poller.tabs));
        let exe = self.exe.clone();
        std::thread::Builder::new()
            .name("wezterm-poll".into())
            .spawn(move || {
                let list = || {
                    Command::new(&exe)
                        .args(cli::list_args())
                        .stdin(Stdio::null())
                        .output()
                        .ok()
                        .filter(|o| o.status.success())
                        .and_then(|o| cli::parse_list(&String::from_utf8_lossy(&o.stdout)).ok())
                        .unwrap_or_default()
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
                    }
                    last = now;
                }
            })
            .expect("a thread");
        Box::new(poller)
    }

    fn screen_text(&self, window: WindowId, max_lines: usize) -> Option<Vec<String>> {
        let w = self.window(window.0)?;
        let pane = w.tabs.iter().find(|t| t.is_active())?.active_pane()?;
        let text = self.run(&cli::get_text_args(pane)).ok()?;
        Some(cli::screen_lines(&text, max_lines))
    }

    fn type_text(&self, window: WindowId, tab: &TabView, text: &str) -> io::Result<()> {
        let Some(found) = self.find_tab(window, tab)? else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "the tab moved"));
        };
        let pane = found.active_pane().ok_or_else(|| io::Error::other("a tab without panes"))?;
        self.run(&cli::send_text_args(pane, text)).map(|_| ())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against a running WezTerm: `NATIVETERM_TEST_WEZTERM_DIR` names its
    /// folder (or it is on `PATH`), and the shim is built.
    #[test]
    #[ignore = "needs WezTerm and a built shim"]
    fn meets_the_contract() {
        let dir = std::env::var_os("NATIVETERM_TEST_WEZTERM_DIR").map(PathBuf::from);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf();
        let shim = root.join("target").join("debug").join(format!("nativeterm-shim{}", std::env::consts::EXE_SUFFIX));
        assert!(shim.exists(), "build the shim first");
        let backend = WezTerm::new(dir.as_deref(), &shim);
        assert!(backend.available(), "no wezterm at {}", backend.exe.display());
        native_term_platform::contract::exercise(&backend, ["nt-wez a", "nt-wez b"]);
    }

    #[test]
    fn a_change_of_shape_is_a_change() {
        let one =
            cli::parse_list(r#"[{"window_id":1,"tab_id":2,"pane_id":3,"tab_title":"a","is_active":true}]"#).unwrap();
        let renamed =
            cli::parse_list(r#"[{"window_id":1,"tab_id":2,"pane_id":3,"tab_title":"b","is_active":true}]"#).unwrap();
        assert_ne!(shape(&one), shape(&renamed));
        assert_eq!(shape(&one), shape(&one.clone()));
        assert_ne!(shape(&one).0, shape(&[]).0);
    }

    #[test]
    fn paths_follow_the_folder() {
        let w = WezTerm::new(Some(Path::new("/opt/wezterm")), Path::new("/opt/nt/shim"));
        assert_eq!(w.exe, Path::new("/opt/wezterm").join(format!("wezterm{}", std::env::consts::EXE_SUFFIX)));
        assert!(w.gui.ends_with(format!("wezterm-gui{}", std::env::consts::EXE_SUFFIX)));
        let on_path = WezTerm::new(None, Path::new("shim"));
        assert_eq!(on_path.exe, PathBuf::from(format!("wezterm{}", std::env::consts::EXE_SUFFIX)));
        assert!(w.capabilities().type_text);
        assert!(!w.capabilities().capture);
    }
}
