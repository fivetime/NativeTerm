//! What NativeTerm asks of a terminal program: open tabs running its
//! helper, read which tabs exist and which are selected, select and
//! close them, and hear when something changed. NativeTerm neither
//! embeds nor renders a terminal; it drives the user's own terminal
//! window from outside (see "Platform sequencing" in
//! `docs/ARCHITECTURE.md`), and this trait is the whole of what it needs
//! for that. Windows Terminal implements it with UI Automation and
//! `wt.exe`; other terminals with their own scripting.
//!
//! Every method may take seconds (a process launch, an automation call)
//! and is called on background threads, never the GUI thread, often from
//! several threads at once.

use std::any::Any;
use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::overlay::{MenuProvider, OverlayMenu};
use crate::{Snapshot, TabSpec, TabView, Target};

/// What `open` did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenReport {
    /// Tabs handed to the terminal (still to be confirmed).
    pub launched: usize,
    /// Tabs not sent: the user left the new window, it never appeared, or
    /// an earlier batch didn't show up in time.
    pub pending: Vec<TabSpec>,
    /// The window created for `Target::NewWindow` (or a restored workspace).
    pub window: Option<isize>,
}

/// An RGBA picture, rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Something about the terminal's windows or tabs changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Change {
    /// A Terminal window appeared or went away.
    Windows,
    /// A Terminal window (or something else) became the foreground window.
    Foreground,
    /// The tab strip changed (tabs opened, closed, moved): tab rectangles
    /// are stale.
    Tabs,
    /// A tab was selected, or a tab's content changed (panes, focus):
    /// worth a rescan, rectangles unchanged.
    Content,
    /// A flyout menu or popup opened or closed (e.g. Terminal's own tab
    /// menu): nothing NativeTerm tracks changed.
    Popup,
    /// A Terminal window moved or changed size: tab rectangles are stale.
    Moved,
}

/// Where a change notification goes.
pub type Notify = Arc<dyn Fn(Change) + Send + Sync>;

/// How many notifications were delivered so far (diagnostics, tests):
/// about windows, and about tabs (selection and structure).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChangeCounts {
    pub windows: u64,
    pub tabs: u64,
}

/// Change notifications from a backend; dropping it stops them.
pub trait Subscription: Send {
    fn counts(&self) -> ChangeCounts;
}

/// What a backend can do beyond the basics; the program hides what it
/// can't, rather than the backend pretending.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// `capture` gives pictures of a window (tab list thumbnails, hover
    /// cards, the Ctrl+Tab grid).
    pub capture: bool,
    /// `screen_text` reads the selected tab's screen.
    pub screen_text: bool,
    /// `start_overlay_menu` gives a menu drawn over the terminal's tab
    /// strip (right-click menu, hover cards, Ctrl+Tab).
    pub overlay_menu: bool,
    /// `TabView.rect` is filled in (the overlay menu needs it).
    pub tab_rects: bool,
    /// The terminal has a profile NativeTerm installs (Windows Terminal's
    /// fragment).
    pub profile_install: bool,
    /// `Target::Named` is honoured: a window the terminal finds again by
    /// its name.
    pub named_windows: bool,
}

/// One terminal program driven from outside.
pub trait TerminalBackend: Send + Sync + 'static {
    fn capabilities(&self) -> Capabilities;

    /// The helper every tab runs (what a client on the pipe must be).
    fn shim_path(&self) -> &Path;

    /// Every window of this terminal, responsive or not.
    fn window_ids(&self) -> Vec<isize>;

    /// The window in front, if it is one of this terminal's.
    fn foreground(&self) -> Option<isize>;

    /// Bring a window forward; false if it couldn't be. It is then also
    /// the window `Target::Recent` means.
    fn activate(&self, window: isize) -> bool;

    /// All windows and tabs, claimed for the open sessions' `labels`.
    /// Claims are kept only for labels in `labels`; a window that can't
    /// be read comes back with `unresponsive` set and `complete` false,
    /// and its earlier claims are kept for the next scan.
    fn snapshot(&self, labels: &HashSet<String>) -> Snapshot;

    /// Open `tabs`, each running `shim_path()` with the arguments the
    /// backend builds from its spec. `Target::Recent` is the most recently
    /// activated window (by the user, or by `activate`); `Target::NewWindow`
    /// must report the new window in `OpenReport::window`; `Target::Named`
    /// is a window the backend can find again by name, or a new window
    /// where it can't (`Capabilities::named_windows`). Confirm the tabs
    /// afterwards with `wait_for`.
    fn open(&self, target: &Target, tabs: &[TabSpec]) -> io::Result<OpenReport>;

    /// A tab in the most recent window running the shim with `shim_args`
    /// (tools like installing a key), titled `title`.
    fn open_tool(&self, title: &str, shim_args: &[String]) -> io::Result<()>;

    /// Bring the window forward and select the tab. `Ok(false)` if the tab
    /// changed since the snapshot (its index or name no longer match).
    fn select(&self, window: isize, tab: &TabView) -> io::Result<bool>;

    /// Close a tab from outside (a fallback: normally the shim closes it).
    fn close(&self, window: isize, tab: &TabView) -> io::Result<bool>;

    /// Change notifications. A backend without events of its own polls
    /// and reports what it saw change.
    fn subscribe(&self, notify: Notify) -> Box<dyn Subscription>;

    /// What is on the screen of `window`'s selected tab, at most
    /// `max_lines` lines. `None` when the window didn't answer in time,
    /// has no text to give, or the backend can't read screens.
    fn screen_text(&self, _window: isize, _max_lines: usize) -> Option<Vec<String>> {
        None
    }

    /// A picture of the window below `content_top` (a screen y, to leave
    /// the tab strip out), scaled to `width` pixels. `None` when the
    /// backend can't picture windows, or this one is minimized.
    fn capture(&self, _window: isize, _content_top: Option<i32>, _width: i32) -> Option<Image> {
        None
    }

    /// NativeTerm's own menu over this terminal's tab strip, if the
    /// backend can draw one. `Ok(None)`: it can't, and nothing is lost but
    /// the menu.
    fn start_overlay_menu(&self, _provider: Arc<dyn MenuProvider>) -> io::Result<Option<Box<dyn OverlayMenu>>> {
        Ok(None)
    }

    /// Wait until every label in `expected` is claimed; returns the last
    /// snapshot and the labels still missing.
    fn wait_for(&self, labels: &HashSet<String>, expected: &[String], timeout: Duration) -> (Snapshot, Vec<String>) {
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

    /// For what only one terminal has (Windows Terminal's install, its
    /// profile): the program asks for the concrete type.
    fn as_any(&self) -> &dyn Any;
}
