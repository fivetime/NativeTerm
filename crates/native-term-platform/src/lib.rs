//! OS-specific control of the native terminal's tabs: NativeTerm's SSH
//! sessions are opened as tabs in the user's own terminal window. See
//! `docs/ARCHITECTURE.md`.

pub mod backend;
pub mod claim;
pub mod contract;
pub mod fake;
pub mod overlay;
#[cfg(windows)]
pub mod windows_terminal;

/// A terminal window, as its backend names it: the `HWND` on Windows,
/// the window id iTerm2 or WezTerm gives out elsewhere. The program only
/// ever compares it and hands it back; `0` is never a window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

impl WindowId {
    /// As the shim reports it (`Hello.terminal_window`): the same number,
    /// signed on the wire.
    #[must_use]
    pub fn from_wire(id: i64) -> WindowId {
        WindowId(id as u64)
    }

    #[must_use]
    pub fn to_wire(self) -> i64 {
        self.0 as i64
    }

    #[cfg(windows)]
    #[must_use]
    pub fn from_hwnd(handle: isize) -> WindowId {
        WindowId(handle as u64)
    }

    #[cfg(windows)]
    #[must_use]
    pub fn hwnd(self) -> isize {
        self.0 as isize
    }
}

impl std::fmt::Display for WindowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

pub use backend::{Capabilities, Change, ChangeCounts, Image, Notify, OpenReport, Subscription, TerminalBackend};
pub use fake::FakeBackend;
pub use overlay::{Entry, HoverCard, MenuProvider, MenuTab, OverlayMenu, SwitcherTab};

/// One tab to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabSpec {
    /// The terminal session GUID NativeTerm assigns (without braces).
    pub terminal_session: String,
    /// The unique tab title the tab is claimed by.
    pub label: String,
    /// NativeTerm's own session id (`--session`), kept by "Restart connection".
    pub session: String,
    /// Host alias for the shim (sanitized: no spaces, quotes or `;`).
    pub alias: String,
    /// Don't connect until NativeTerm (or the user) says so.
    pub wait: bool,
    /// A clone: no port forwards (they would clash with the original's).
    pub no_forwards: bool,
    /// `--tabColor` (`#RRGGBB`), from the host or its folder.
    pub tab_color: Option<String>,
}

/// Which window new tabs go to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The most recently used window (`-w 0`).
    Recent,
    /// One new unnamed window for all of them (`-w new`, then `-w 0`).
    NewWindow,
    /// A named window (`-w <name>`).
    Named(String),
}

/// A screen rectangle in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// A NativeTerm session a tab was claimed for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    pub label: String,
    /// The tab also holds panes that aren't this session (the user split it).
    /// Only known once the tab has been selected.
    pub mixed: bool,
}

/// A tab as NativeTerm sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabView {
    /// Position in the window's tab strip.
    pub index: usize,
    /// Current title (the focused pane's title for a split tab).
    pub name: String,
    pub selected: bool,
    /// Only for tabs currently realized in the strip (not scrolled away).
    pub rect: Option<Rect>,
    pub claim: Option<Claim>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowView {
    /// Native window handle.
    pub handle: WindowId,
    pub pid: u32,
    pub foreground: bool,
    /// The window didn't answer and was skipped; its tabs are unknown.
    pub unresponsive: bool,
    pub tabs: Vec<TabView>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub windows: Vec<WindowView>,
    /// False if a window couldn't be read; claims of missing windows are kept.
    pub complete: bool,
}

impl Snapshot {
    pub fn claimed(&self) -> impl Iterator<Item = (&WindowView, &TabView, &Claim)> {
        self.windows.iter().flat_map(|w| w.tabs.iter().filter_map(move |t| t.claim.as_ref().map(|c| (w, t, c))))
    }

    pub fn find(&self, label: &str) -> Option<(&WindowView, &TabView)> {
        self.claimed().find(|(_, _, c)| c.label == label).map(|(w, t, _)| (w, t))
    }
}
