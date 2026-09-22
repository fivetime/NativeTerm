//! NativeTerm's own menu over the terminal's tab strip, as the program
//! sees it: what a tab is, what the menu shows for it, what the hover
//! card and the Ctrl+Tab grid hold, and what the program can ask of the
//! menu once it is up. A backend that can draw such a menu (Windows
//! Terminal, with hooks and a popup of its own) returns one from
//! `TerminalBackend::start_overlay_menu`; the drawing is the backend's.

use std::time::Duration;

use crate::{Rect, WindowId};

/// A NativeTerm tab as the menu knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuTab {
    pub window: WindowId,
    pub rect: Rect,
    /// The session label the tab was claimed for.
    pub label: String,
    /// The tab's current title.
    pub title: String,
    pub mixed: bool,
    pub index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A glyph from Segoe Fluent Icons (Segoe MDL2 Assets on Windows 10).
    Action {
        id: u32,
        glyph: char,
        text: String,
        enabled: bool,
    },
    Header(String),
    Separator,
}

/// What a tab's card shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HoverCard {
    /// The session's name (the tab's title when it has no session).
    pub title: String,
    /// Under it: where the tab is, the state, when the picture was taken.
    pub note: String,
    /// The tab's picture: width, height, RGBA.
    pub image: Option<(u32, u32, Vec<u8>)>,
    /// What its console holds, when there is no picture of it.
    pub lines: Vec<String>,
    /// How wide that console is, for sizing the text.
    pub columns: u16,
}

impl HoverCard {
    /// Nothing to show: no picture and no text.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.image.is_none() && self.lines.iter().all(String::is_empty)
    }
}

/// One tab in the Ctrl+Tab grid.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SwitcherTab {
    pub window: WindowId,
    pub index: usize,
    /// What the tile says: the session's name, or the tab's title.
    pub title: String,
    /// What Terminal calls the tab, for finding it again when the strip
    /// has moved under the grid.
    pub name: String,
    /// The tab's last picture: width, height, RGBA.
    pub image: Option<(u32, u32, Vec<u8>)>,
    /// What was on its screen, when there is no picture.
    pub lines: Vec<String>,
    /// How wide that screen is, for sizing the text.
    pub columns: u16,
    /// Whether it is the window's selected tab.
    pub selected: bool,
}

/// What the menu shows for a tab, and what happens when an item is chosen.
/// Called on the menu thread: don't block.
pub trait MenuProvider: Send + Sync {
    fn entries(&self, tab: &MenuTab) -> Vec<Entry>;
    fn chosen(&self, tab: &MenuTab, id: u32);
    /// What to show when the mouse rests on the tab, and how long to wait
    /// first (`None`: no card for this tab, or the person turned them
    /// off). Called on the menu thread: don't block.
    fn hover(&self, _tab: &MenuTab) -> Option<(HoverCard, Duration)> {
        None
    }

    /// Every tab of `window` in strip order, for the Ctrl+Tab grid, with
    /// the selected one marked. Fewer than two: no grid. Called on the
    /// menu thread: don't block.
    fn tiles(&self, _window: WindowId) -> Vec<SwitcherTab> {
        Vec::new()
    }

    /// Switch to the tab the grid picked (by index, or by title if the
    /// strip moved under it). Called on the menu thread: don't block.
    fn switch(&self, _window: WindowId, _index: usize, _title: &str) {}
}

/// The menu once it is up: what the program tells it and asks of it.
pub trait OverlayMenu: Send + Sync {
    /// The current NativeTerm tabs, fresh from a scan.
    fn set_tabs(&self, tabs: Vec<MenuTab>);
    /// Tabs may have moved: pass right-clicks through until the next scan.
    fn invalidate(&self);
    fn is_open(&self) -> bool;
    /// How many menus were opened (diagnostics, tests).
    fn opened(&self) -> u32;
    /// Choose an item of the open menu by id, as a click would (automation).
    fn choose(&self, id: u32);
    /// Id of the highlighted item of the open menu, if any.
    fn hovered(&self) -> Option<u32>;
    /// Whether Ctrl+Tab over a terminal window with NativeTerm tabs shows
    /// NativeTerm's grid.
    fn set_ctrl_tab(&self, on: bool);
    fn ctrl_tab(&self) -> bool;
    /// Whether the grid is on screen.
    fn switcher_open(&self) -> bool;
    /// The tab the grid would switch to: window and index.
    fn switcher_pick(&self) -> Option<(WindowId, usize)>;
    /// Grids shown, and tabs switched by one (diagnostics, tests).
    fn switcher_counts(&self) -> (u32, u32);
    /// Known tabs, stale flag, hooks installed, right-clicks seen.
    fn debug_state(&self) -> String;
    /// How long ago a drag from another window ended over one of the
    /// terminal's windows, if one has since the menu was put up: text that
    /// arrives right after came from a drop, not from typing or a paste.
    fn since_drag_release(&self) -> Option<Duration>;
}
