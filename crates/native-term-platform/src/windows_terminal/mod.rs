//! Windows Terminal backend: tabs are opened with `wt` and found, selected
//! and closed through UI Automation, for one chosen Terminal install.

pub mod command;
pub mod events;
pub mod install;
pub mod jsonc;
pub mod launch;
pub mod menu;
mod menu_draw;
pub mod profile;
pub mod sources;
pub mod theme;
pub mod uia;
pub mod window;
pub mod worker;

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::claim::{strip_admin_prefix, Claimer};
use crate::{Snapshot, TabSpec, TabView, Target, WindowView};
use install::Install;
use window::TerminalWindow;
use worker::Worker;

/// Deadline for reading or acting on one window through UIA.
pub const UIA_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a new window may take to appear.
pub const NEW_WINDOW_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a batch of tabs may take to appear before the next is sent.
pub const BATCH_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(100);

/// The optional dedicated window's name.
pub const DEDICATED_WINDOW_NAME: &str = "NativeTerm";

/// What `open` did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenReport {
    /// Tabs handed to `wt` (still to be confirmed).
    pub launched: usize,
    /// Tabs not sent: the user left the new window, it never appeared, or
    /// an earlier batch didn't show up in time.
    pub pending: Vec<TabSpec>,
    /// The window created for `Target::NewWindow` (or a restored workspace).
    pub window: Option<isize>,
}

pub struct WindowsTerminal {
    install: Install,
    shim: PathBuf,
    /// Arguments for every session tab's shim (`--ssh-dir`).
    shim_args: Vec<std::ffi::OsString>,
    worker: Mutex<Worker>,
    claimer: Mutex<Claimer>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl WindowsTerminal {
    pub fn new(install: Install, shim: &Path) -> WindowsTerminal {
        WindowsTerminal {
            install,
            shim: shim.to_path_buf(),
            shim_args: Vec::new(),
            worker: Mutex::new(Worker::new()),
            claimer: Mutex::new(Claimer::new()),
        }
    }

    /// Session tabs look sessions up in `ssh_dir` instead of `~/.ssh`
    /// (NativeTerm started with `--ssh-dir`).
    pub fn with_ssh_dir(mut self, ssh_dir: &Path) -> WindowsTerminal {
        self.shim_args = vec!["--ssh-dir".into(), ssh_dir.into()];
        self
    }

    pub fn install(&self) -> &Install {
        &self.install
    }

    pub fn windows(&self) -> Vec<TerminalWindow> {
        window::terminal_windows(&self.install)
    }

    /// Stuck UIA workers left behind (a Terminal hung mid-call).
    pub fn abandoned_workers(&self) -> usize {
        lock(&self.worker).abandoned()
    }

    /// All windows and tabs, claimed for the open sessions' `labels`.
    pub fn snapshot(&self, labels: &HashSet<String>) -> Snapshot {
        let foreground = window::foreground();
        let windows = self.windows();
        let mut complete = true;
        let mut views = Vec::new();
        for w in &windows {
            let handle = w.handle;
            let read = if w.responsive {
                lock(&self.worker).run(UIA_TIMEOUT, move |a| uia::read_window(a, handle).ok())
            } else {
                None
            };
            let mut view = WindowView {
                handle,
                pid: w.pid,
                foreground: handle == foreground,
                unresponsive: read.is_none(),
                tabs: Vec::new(),
            };
            match read.flatten() {
                Some(tabs) => view.tabs = lock(&self.claimer).claim(handle, &tabs, labels),
                None => complete = false,
            }
            views.push(view);
        }
        if complete {
            lock(&self.claimer).retain(&windows.iter().map(|w| w.handle).collect::<Vec<_>>());
        }
        Snapshot { windows: views, complete }
    }

    /// Open `tabs` with `wt`. Confirm them afterwards with `wait_for`.
    pub fn open(&self, target: &Target, tabs: &[TabSpec]) -> io::Result<OpenReport> {
        let mut report = OpenReport::default();
        if let Target::Named(name) = target {
            // a saved workspace would swallow the command: restore it first
            if self.install.has_saved_workspace(name, false) {
                let before = self.handles();
                launch::run(&self.install.launcher, &["-w".into(), name.into()])?;
                report.window = self.wait_for_new_window(&before, NEW_WINDOW_TIMEOUT);
            }
        }
        let mut previous: &[TabSpec] = &[];
        for (i, (chunk, args)) in command::batches(target, tabs, &self.shim, &self.shim_args).into_iter().enumerate() {
            let new_window = *target == Target::NewWindow;
            if i > 0 {
                // Terminal builds tabs asynchronously: a batch sent while the
                // previous one is still being built gets interleaved with it
                let titles: Vec<&str> = previous.iter().map(|t| t.label.as_str()).collect();
                if !self.wait_for_titles(&titles, BATCH_TIMEOUT) || (new_window && self.user_left(report.window)) {
                    // or the rest would land in the Terminal window the user went to
                    report.pending = tabs[report.launched..].to_vec();
                    return Ok(report);
                }
            }
            previous = chunk;
            let before = if new_window && i == 0 { self.handles() } else { Vec::new() };
            launch::run(&self.install.launcher, &args)?;
            report.launched += chunk.len();
            if new_window && i == 0 {
                report.window = self.wait_for_new_window(&before, NEW_WINDOW_TIMEOUT);
                if report.window.is_none() {
                    report.pending = tabs[report.launched..].to_vec();
                    return Ok(report);
                }
            }
        }
        Ok(report)
    }

    /// A tab in the most recent window running the shim with `shim_args`
    /// (tools like installing a key), titled `title`.
    pub fn open_tool(&self, title: &str, shim_args: &[String]) -> io::Result<()> {
        let mut args: Vec<std::ffi::OsString> = vec![
            "-w".into(),
            "0".into(),
            "new-tab".into(),
            "--profile".into(),
            command::PROFILE_NAME.into(),
            format!("--title={}", command::escape_delimiters(title)).into(),
            "--suppressApplicationTitle".into(),
            self.shim.clone().into(),
        ];
        args.extend(shim_args.iter().map(|a| command::escape_delimiters(a).into()));
        launch::run(&self.install.launcher, &args)
    }

    /// Wait until tabs with all these titles exist, in any window. Reads
    /// names only and leaves the claims alone.
    pub fn wait_for_titles(&self, titles: &[&str], timeout: Duration) -> bool {
        let started = Instant::now();
        loop {
            let mut seen = HashSet::new();
            for w in self.windows().into_iter().filter(|w| w.responsive) {
                let handle = w.handle;
                if let Some(Ok(tabs)) = lock(&self.worker).run(UIA_TIMEOUT, move |a| uia::read_window(a, handle)) {
                    seen.extend(tabs.names.into_iter().map(|n| strip_admin_prefix(&n).to_string()));
                }
            }
            if titles.iter().all(|t| seen.contains(*t)) {
                return true;
            }
            if started.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Whether another window of this Terminal was activated since `window`
    /// was created. Terminal counts a new window as the most recent one from
    /// its creation, so other apps in the foreground don't matter.
    fn user_left(&self, window: Option<isize>) -> bool {
        let foreground = window::foreground();
        Some(foreground) != window && self.handles().contains(&foreground)
    }

    fn handles(&self) -> Vec<isize> {
        self.windows().iter().map(|w| w.handle).collect()
    }

    pub fn wait_for_new_window(&self, before: &[isize], timeout: Duration) -> Option<isize> {
        let started = Instant::now();
        loop {
            if let Some(w) = self.windows().into_iter().find(|w| !before.contains(&w.handle)) {
                return Some(w.handle);
            }
            if started.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(POLL);
        }
    }

    /// Wait until every label in `expected` is claimed; returns the last
    /// snapshot and the labels still missing.
    pub fn wait_for(&self, labels: &HashSet<String>, expected: &[String], timeout: Duration) -> (Snapshot, Vec<String>) {
        let started = Instant::now();
        loop {
            let snapshot = self.snapshot(labels);
            let missing: Vec<String> = expected.iter().filter(|l| snapshot.find(l).is_none()).cloned().collect();
            if missing.is_empty() || started.elapsed() >= timeout {
                return (snapshot, missing);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Bring the window forward and select the tab. `Ok(false)` if the tab
    /// changed since the snapshot.
    pub fn select(&self, window: isize, tab: &TabView) -> io::Result<bool> {
        self.act(window, tab, uia::select_tab).inspect(|&done| {
            if done {
                window::activate(window);
            }
        })
    }

    /// Close a tab through its close button (fallback only).
    pub fn close(&self, window: isize, tab: &TabView) -> io::Result<bool> {
        self.act(window, tab, uia::close_tab)
    }

    fn act(
        &self,
        window: isize,
        tab: &TabView,
        action: fn(&uiautomation::UIAutomation, isize, usize, &str) -> uia::Result<bool>,
    ) -> io::Result<bool> {
        if !window::responds(window::hwnd(window)) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "Windows Terminal is not responding"));
        }
        let (index, name) = (tab.index, tab.name.clone());
        match lock(&self.worker).run(UIA_TIMEOUT, move |a| action(a, window, index, &name)) {
            None => Err(io::Error::new(io::ErrorKind::TimedOut, "Windows Terminal is not responding")),
            Some(result) => result.map_err(|e| io::Error::other(e.to_string())),
        }
    }
}
