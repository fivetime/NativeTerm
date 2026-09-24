//! WezTerm driven from outside through `wezterm cli`: tabs are spawned
//! with the shim as their program and titled with their label (which the
//! claimer's first rule finds), `list --format json` and `list-clients`
//! (the focused pane) are the whole state, and there are no events, so a
//! subscription polls them. Which tab a window shows is known for the
//! focused window from the focused pane, and for the others from what
//! this backend last selected or saw focused. Text is typed
//! with `send-text` and screens read with `get-text`; nothing is
//! pictured, no menu is drawn over the tab strip, and a window can't be
//! brought forward by the CLI (a pane can be focused, which is what
//! `activate` does).
//!
//! WezTerm runs on Linux, macOS and Windows alike; this crate builds on
//! all three.

pub mod cli;
pub use cli::Look;

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
/// How long a GUI just started on Wayland gets to die before its window
/// counts (see `gui_died_on_wayland`).
const GUI_GRACE: Duration = Duration::from_millis(1500);
/// How often a subscription looks for changes.
const POLL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct State {
    /// What `Target::Recent` means: the window last activated or made.
    recent: Option<u64>,
    /// `Target::Named` windows, for as long as this process runs.
    named: HashMap<String, u64>,
    /// The tab each window shows, as last seen focused or selected here
    /// (and when it was selected here).
    selected: HashMap<u64, Chosen>,
    /// The GUI died on Wayland before it had a window (a compositor this
    /// WezTerm can't talk to): it runs through X11 from now on.
    x11_only: bool,
}

/// A tab NativeTerm selected in a window, and when.
#[derive(Clone, Copy, Debug)]
struct Chosen {
    tab_id: u64,
    at: Instant,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The tab a window shows: the one known, else the only one there is.
fn shown_or_only(known: Option<usize>, tabs: usize) -> Option<usize> {
    known.or_else(|| (tabs == 1).then_some(0))
}

/// `path` holding `wanted`: written when it doesn't yet. Whether it
/// does now.
fn write_if_changed(path: &Path, wanted: &str) -> bool {
    std::fs::read_to_string(path).ok().as_deref() == Some(wanted) || std::fs::write(path, wanted).is_ok()
}

/// WezTerm, through its command line.
pub struct WezTerm {
    /// `wezterm` (the CLI).
    exe: PathBuf,
    /// `wezterm-gui`, started when no WezTerm is running.
    gui: PathBuf,
    shim: PathBuf,
    shim_args: Vec<OsString>,
    /// NativeTerm's own configuration for the windows it opens, when the
    /// person has none.
    config: Option<PathBuf>,
    claimer: Mutex<Claimer>,
    state: Mutex<State>,
}

/// Where a WezTerm of one's own is, looked at before the one on `PATH`
/// (a distribution's package): `~/.local/bin`, where a hand-installed
/// one goes; on macOS the application bundle in `/Applications` or
/// `~/Applications` too (an app started from the Finder gets no shell
/// `PATH` either). Folders to give `WezTerm::new`.
#[must_use]
pub fn app_dirs() -> Vec<PathBuf> {
    if cfg!(windows) {
        return Vec::new();
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = home.iter().map(|h| h.join(".local").join("bin")).collect();
    if !cfg!(target_os = "macos") {
        return dirs;
    }
    let bundle = Path::new("WezTerm.app/Contents/MacOS");
    dirs.push(Path::new("/Applications").join(bundle));
    if let Some(home) = &home {
        dirs.push(home.join("Applications").join(bundle));
    }
    dirs
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
            config: None,
            claimer: Mutex::new(Claimer::new()),
            state: Mutex::new(State::default()),
        }
    }

    /// Windows NativeTerm opens use `<dir>/wezterm.lua` (written here,
    /// for `look`) when the person has no WezTerm configuration of their
    /// own.
    pub fn with_config_dir(mut self, dir: &Path, look: &Look) -> WezTerm {
        if cli::user_config_exists() {
            return self;
        }
        let path = dir.join("wezterm.lua");
        if std::fs::create_dir_all(dir).is_err() || !write_if_changed(&path, &cli::default_config(look, &self.shim)) {
            return self;
        }
        self.config = Some(path);
        self
    }

    /// The look changed (the desktop's, or NativeTerm's theme setting):
    /// rewrite the configuration, which running WezTerm windows pick up
    /// on their own. Whether there is one to rewrite.
    pub fn set_look(&self, look: &Look) -> bool {
        self.config.as_deref().is_some_and(|path| write_if_changed(path, &cli::default_config(look, &self.shim)))
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

    /// `wezterm` or `wezterm-gui` with NativeTerm's configuration, when
    /// it has one, in the environment (see `cli::start_args`).
    fn command(&self, exe: &Path) -> Command {
        let mut command = Command::new(exe);
        if let Some(config) = &self.config {
            command.env("WEZTERM_CONFIG_FILE", config);
        }
        if lock(&self.state).x11_only {
            command.env_remove("WAYLAND_DISPLAY");
        }
        command
    }

    /// A GUI this started, gone while a Wayland display was set: a
    /// compositor this WezTerm can't talk to (Pantheon's gala with the
    /// 2024 release dies at once, after listing its window). X11 through
    /// Xwayland works there, so from now on the GUI runs without the
    /// Wayland display; the dead GUI's discovery socket, which the cli
    /// would try first, is removed. Whether that is what happened.
    fn gui_died_on_wayland(&self, child: &mut std::process::Child) -> bool {
        let died = child.try_wait().ok().flatten().is_some();
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty());
        if !(died && wayland) || lock(&self.state).x11_only {
            return false;
        }
        cli::forget_gui_socket(child.id());
        lock(&self.state).x11_only = true;
        true
    }

    /// Start the GUI with `program` in its first window.
    fn start_gui(&self, program: &[OsString]) -> io::Result<std::process::Child> {
        self.command(&self.gui)
            .args(cli::start_args(program))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }

    /// Run `wezterm <args>`; its stdout, or what it said on stderr.
    fn run(&self, args: &[OsString]) -> io::Result<String> {
        let output = self.command(&self.exe).args(args).stdin(Stdio::null()).output()?;
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

    /// The panes the GUI has the focus on, with how long ago it last had
    /// input.
    fn focused_panes(&self) -> Vec<cli::Focus> {
        self.run(&cli::list_clients_args()).ok().and_then(|j| cli::parse_focus(&j).ok()).unwrap_or_default()
    }

    /// Note which tab each window shows, from the focused panes, and give
    /// the index of the tab shown in each window (as far as it is known).
    fn selected_tabs(&self, windows: &[ListedWindow], focused: &[cli::Focus]) -> HashMap<u64, usize> {
        let now = Instant::now();
        let mut state = lock(&self.state);
        for w in windows {
            let Some((index, focus)) = focused.iter().find_map(|f| w.tab_of_pane(f.pane).map(|i| (i, f))) else {
                continue;
            };
            // the GUI's focus is what the person last made of the window;
            // a tab selected here since then is what the window shows
            // (the GUI reports it only once the window itself had the focus)
            let ours_since = state.selected.get(&w.window_id).map(|c| c.at);
            let input_at = now.checked_sub(focus.since_input);
            let ours_newer = match (ours_since, input_at) {
                (Some(ours), Some(input)) => ours > input,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if !ours_newer {
                state
                    .selected
                    .insert(w.window_id, Chosen { tab_id: w.tabs[index].tab_id, at: input_at.unwrap_or(now) });
            }
        }
        state.selected.retain(|id, _| windows.iter().any(|w| w.window_id == *id));
        windows
            .iter()
            .filter_map(|w| {
                state.selected.get(&w.window_id).and_then(|c| w.tab_index(c.tab_id)).map(|i| (w.window_id, i))
            })
            .collect()
    }

    /// The tab `window` shows, as far as it is known.
    fn shown_tab(&self, window: &ListedWindow) -> Option<usize> {
        let focused = self.focused_panes();
        let known = self.selected_tabs(std::slice::from_ref(window), &focused).get(&window.window_id).copied();
        shown_or_only(known, window.tabs.len())
    }

    /// A tab this made is the one its window shows (WezTerm switches to
    /// what it spawns), whether or not the GUI ever reports a focus — on
    /// a desktop that keeps a new window from taking the focus (GNOME
    /// with a window opened from elsewhere) it never does.
    fn note_shown(&self, window: u64, pane: u64) {
        if let Some(tab) = self.window(window).and_then(|w| w.tab_of_pane(pane).map(|i| w.tabs[i].tab_id)) {
            lock(&self.state).selected.insert(window, Chosen { tab_id: tab, at: Instant::now() });
        }
    }

    /// The window `Target::Recent` means: the one last activated or made,
    /// else the one with the focus, else any.
    fn recent(&self, windows: &[ListedWindow]) -> Option<u64> {
        let recent = lock(&self.state).recent;
        recent.filter(|id| windows.iter().any(|w| w.window_id == *id)).or_else(|| {
            let focused = self.focused_panes();
            windows
                .iter()
                .find(|w| focused.iter().any(|f| w.tab_of_pane(f.pane).is_some()))
                .or(windows.first())
                .map(|w| w.window_id)
        })
    }

    /// A new window running `program`: `spawn --new-window` when WezTerm
    /// runs, else the GUI started with it; the window's id once it lists.
    fn new_window(&self, program: &[OsString], before: &[ListedWindow]) -> io::Result<u64> {
        let known: HashSet<u64> = before.iter().map(|w| w.window_id).collect();
        let (pane, timeout, mut gui) = if before.is_empty() && self.run(&cli::list_args()).is_err() {
            (None, START_TIMEOUT, Some(self.start_gui(program)?))
        } else {
            let out = self.run(&cli::spawn_args(Into::NewWindow, program))?;
            (cli::parse_spawned(&out), NEW_WINDOW_TIMEOUT, None)
        };
        let mut started = Instant::now();
        loop {
            // the GUI gone before its window came: on a Wayland compositor
            // this WezTerm can't talk to (Pantheon's gala with the 2024
            // release) it dies at once; X11 through Xwayland works there,
            // so it is started once more without Wayland, for good
            if let Some(child) = gui.as_mut() {
                if self.gui_died_on_wayland(child) {
                    gui = Some(self.start_gui(program)?);
                    started = Instant::now();
                    continue;
                }
            }
            let windows = self.list();
            let found = windows.iter().find(|w| match pane {
                Some(pane) => w.tabs.iter().any(|t| t.panes.iter().any(|p| p.pane_id == pane)),
                None => !known.contains(&w.window_id),
            });
            if let Some(w) = found {
                // a GUI just started on Wayland lists its window and only
                // then dies on a compositor it can't talk to: give it a
                // moment before its window counts
                if let Some(child) = gui.as_mut().filter(|_| !lock(&self.state).x11_only) {
                    let grace = Instant::now() + GUI_GRACE;
                    while Instant::now() < grace {
                        if self.gui_died_on_wayland(child) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    if lock(&self.state).x11_only {
                        gui = Some(self.start_gui(program)?);
                        started = Instant::now();
                        continue;
                    }
                }
                if let Some(pane) = w.tabs.first().and_then(|t| t.active_pane()) {
                    self.note_shown(w.window_id, pane);
                }
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
        self.note_shown(window, pane);
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

/// One tab as a poll sees it: window, tab, name.
type TabShape = (u64, u64, String);

/// What a poll compares: the windows, each tab's name, and the focus.
fn shape(windows: &[ListedWindow], focused: &[u64]) -> (Vec<u64>, Vec<TabShape>, Vec<u64>) {
    let ids = windows.iter().map(|w| w.window_id).collect();
    let tabs = windows.iter().flat_map(|w| w.tabs.iter().map(move |t| (w.window_id, t.tab_id, t.name()))).collect();
    (ids, tabs, focused.to_vec())
}

impl TerminalBackend for WezTerm {
    fn name(&self) -> &'static str {
        "WezTerm"
    }

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
        let shown = self.shown_tab(&w).unwrap_or(0);
        lock(&self.state).recent = Some(window.0);
        let pane = w.tabs.get(shown).or(w.tabs.first()).and_then(|t| t.active_pane());
        pane.is_some_and(|p| self.run(&cli::activate_pane_args(p)).is_ok())
    }

    fn snapshot(&self, labels: &HashSet<String>) -> Snapshot {
        let listed = self.list();
        let focused = self.focused_panes();
        let selected = self.selected_tabs(&listed, &focused);
        let mut claimer = lock(&self.claimer);
        let windows = listed
            .iter()
            .map(|w| WindowView {
                handle: w.id(),
                pid: 0,
                foreground: focused.iter().any(|f| w.tab_of_pane(f.pane).is_some()),
                unresponsive: false,
                tabs: claimer.claim(w.id(), &w.as_window_tabs(selected.get(&w.window_id).copied()), labels),
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
        let mut state = lock(&self.state);
        state.recent = Some(window.0);
        state.selected.insert(window.0, Chosen { tab_id: found.tab_id, at: Instant::now() });
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
                let output = |args: Vec<OsString>| {
                    Command::new(&exe)
                        .args(args)
                        .stdin(Stdio::null())
                        .output()
                        .ok()
                        .filter(|o| o.status.success())
                        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                };
                let look = || {
                    let windows = output(cli::list_args()).and_then(|j| cli::parse_list(&j).ok()).unwrap_or_default();
                    let focused: Vec<u64> = output(cli::list_clients_args())
                        .and_then(|j| cli::parse_focus(&j).ok())
                        .unwrap_or_default()
                        .into_iter()
                        .map(|f| f.pane)
                        .collect();
                    shape(&windows, &focused)
                };
                let mut last = look();
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(POLL);
                    let now = look();
                    if now.0 != last.0 {
                        windows.fetch_add(1, Ordering::Relaxed);
                        notify(Change::Windows);
                    } else if now.1 != last.1 {
                        tabs.fetch_add(1, Ordering::Relaxed);
                        notify(Change::Tabs);
                    } else if now.2 != last.2 {
                        // the focus moved: another tab or window is shown
                        tabs.fetch_add(1, Ordering::Relaxed);
                        notify(Change::Content);
                    }
                    last = now;
                }
            })
            .expect("a thread");
        Box::new(poller)
    }

    fn screen_text(&self, window: WindowId, max_lines: usize) -> Option<Vec<String>> {
        let w = self.window(window.0)?;
        let pane = w.tabs.get(self.shown_tab(&w)?)?.active_pane()?;
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

    /// The shim built next to this test binary (`<target>/debug`), wherever
    /// the target folder is.
    fn built_shim() -> PathBuf {
        let exe = std::env::current_exe().expect("this test binary");
        let debug = exe.parent().and_then(Path::parent).expect("target/debug/deps/<test>");
        debug.join(format!("nativeterm-shim{}", std::env::consts::EXE_SUFFIX))
    }

    /// Against a running WezTerm: `NATIVETERM_TEST_WEZTERM_DIR` names its
    /// folder (or it is on `PATH`), and the shim is built.
    #[test]
    #[ignore = "needs WezTerm and a built shim"]
    fn meets_the_contract() {
        let dir = std::env::var_os("NATIVETERM_TEST_WEZTERM_DIR").map(PathBuf::from);
        let shim = built_shim();
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
        assert_ne!(shape(&one, &[3]), shape(&renamed, &[3]));
        assert_eq!(shape(&one, &[3]), shape(&one.clone(), &[3]));
        assert_ne!(shape(&one, &[3]).2, shape(&one, &[]).2, "the focus moved");
        assert_ne!(shape(&one, &[]).0, shape(&[], &[]).0);
    }

    /// Which tab a window shows comes from the focused pane, and stays
    /// known for a window that lost the focus.
    #[test]
    fn the_shown_tab_is_remembered() {
        let w = WezTerm::new(None, Path::new("shim"));
        let windows = cli::parse_list(
            r#"[{"window_id":1,"tab_id":2,"pane_id":3,"tab_title":"a","is_active":true},
                {"window_id":1,"tab_id":4,"pane_id":5,"tab_title":"b","is_active":true},
                {"window_id":7,"tab_id":8,"pane_id":9,"tab_title":"c","is_active":true}]"#,
        )
        .unwrap();
        let focus = |pane: u64, idle_secs: u64| cli::Focus { pane, since_input: Duration::from_secs(idle_secs) };
        let selected = w.selected_tabs(&windows, &[focus(5, 0)]);
        assert_eq!(selected.get(&1), Some(&1));
        assert_eq!(selected.get(&7), None, "never seen focused");
        let selected = w.selected_tabs(&windows, &[focus(9, 0)]);
        assert_eq!(selected.get(&1), Some(&1), "still what it showed");
        assert_eq!(selected.get(&7), Some(&0));
        let only_one = &windows[1..];
        let selected = w.selected_tabs(only_one, &[]);
        assert_eq!(selected.get(&1), None, "window 1 is gone");
        assert_eq!(selected.get(&7), Some(&0));
    }

    #[test]
    fn a_window_with_one_tab_shows_it() {
        assert_eq!(shown_or_only(None, 1), Some(0));
        assert_eq!(shown_or_only(None, 2), None, "two tabs: not known");
        assert_eq!(shown_or_only(Some(1), 2), Some(1));
        assert_eq!(shown_or_only(None, 0), None);
    }

    /// A tab selected here is what the window shows until the person acts
    /// on the window: the GUI reports its focus only from its own events,
    /// which a window without the focus never gets.
    #[test]
    fn a_tab_selected_here_beats_a_stale_focus() {
        let w = WezTerm::new(None, Path::new("shim"));
        let windows = cli::parse_list(
            r#"[{"window_id":1,"tab_id":2,"pane_id":3,"tab_title":"a","is_active":true},
                {"window_id":1,"tab_id":4,"pane_id":5,"tab_title":"b","is_active":true}]"#,
        )
        .unwrap();
        let focus = |pane: u64, idle_secs: u64| cli::Focus { pane, since_input: Duration::from_secs(idle_secs) };
        // the person last touched the window 10 s ago, on tab "a"
        assert_eq!(w.selected_tabs(&windows, &[focus(3, 10)]).get(&1), Some(&0));
        // NativeTerm selected "b" since: that is what the window shows
        lock(&w.state).selected.insert(1, Chosen { tab_id: 4, at: Instant::now() });
        assert_eq!(w.selected_tabs(&windows, &[focus(3, 10)]).get(&1), Some(&1), "ours is newer than the input");
        // the person clicks tab "a" now: the focus is what counts again
        assert_eq!(w.selected_tabs(&windows, &[focus(3, 0)]).get(&1), Some(&0));
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
