//! What NativeTerm's tab menu offers and does. Only NativeTerm's own tabs
//! are ever touched; the user's other tabs never are.

use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use native_term_platform::{Entry, HoverCard, Icon, MenuProvider as Provider, MenuTab, SwitcherTab, WindowId};
use native_term_session::protocol::MenuItem;

use crate::actions::{close_set, CloseSet, Closing, SessionCommand};
use crate::{t, Core, SessionView, Shared, State};

pub const CONNECT: u32 = 1;
pub const DISCONNECT: u32 = 2;
pub const CLONE: u32 = 3;
pub const CLOSE: u32 = 4;
pub const CLOSE_OTHERS: u32 = 5;
pub const CLOSE_ENDED: u32 = 6;
pub const CLOSE_RIGHT: u32 = 7;
pub const SEND: u32 = 8;
pub const LOCK: u32 = 9;
pub const CLEAR: u32 = 10;
pub const RENAME: u32 = 11;
pub const BREAK: u32 = 12;
pub const FILES: u32 = 13;

/// What the menu asks the main window to do (its dialogs live there).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuRequest {
    /// Send commands to this session.
    Send(String),
    /// Rename this host.
    Rename(String),
    /// Closing these sessions would close tabs that hold other panes too.
    ConfirmClose(Vec<String>),
    /// This session's files (SFTP): alias, session id.
    Files { alias: String, session: String },
    /// Files dropped into this session's tab: the client held back the
    /// text Terminal pasted, so either they are uploaded or the text is
    /// sent after all.
    Dropped { alias: String, session: String, paths: Vec<PathBuf>, text: String },
}

/// `off` keeps the cards away (`state.db`).
pub const HOVER_SETTING: &str = "tabs.hover";
/// `on` gives Ctrl+Tab to NativeTerm's thumbnail grid (`state.db`). Off
/// until the person asks for it: taking a key from Terminal is not
/// something to do behind their back.
pub const SWITCHER_SETTING: &str = "tabs.switcher";
/// How long the mouse rests on a tab before its card appears.
const DELAY: Duration = Duration::from_millis(500);
/// A tab without a picture is asked what is on its screen at most this
/// often (the tab list asks on the same terms).
const SCREEN_EVERY: Duration = Duration::from_secs(5);

pub(crate) struct Actions {
    pub(crate) core: Weak<Shared>,
    /// Asks the main window; its dialogs live in the binary.
    pub(crate) ask: Arc<dyn Fn(MenuRequest) + Send + Sync>,
}

/// The menu's tab for `session`, when it is somewhere in a window.
fn tab_of(session: &SessionView) -> Option<MenuTab> {
    let at = session.location.as_ref()?;
    Some(MenuTab {
        window: at.window,
        rect: native_term_platform::Rect::default(),
        label: session.label.clone(),
        title: at.title.clone(),
        mixed: at.mixed,
        index: at.tab_index,
    })
}

/// The menu as a terminal that shows it itself gets it (see the shim's
/// `--tab-menu`): the items that apply, and a heading (id 0) — the
/// menu's own header where it has one, else the session's label.
pub(crate) fn items_for(shared: &Arc<Shared>, session: &str) -> Vec<MenuItem> {
    let core = Core { shared: Arc::clone(shared) };
    let Some(view) = core.sessions().into_iter().find(|s| s.id == session) else { return Vec::new() };
    let Some(tab) = tab_of(&view) else { return Vec::new() };
    let actions = Actions { core: Arc::downgrade(shared), ask: Arc::new(|_| {}) };
    let mut items = Vec::new();
    for entry in actions.entries(&tab) {
        match entry {
            Entry::Header(text) => items.push(MenuItem { id: 0, text }),
            Entry::Action { id, text, enabled: true, .. } => items.push(MenuItem { id, text }),
            Entry::Action { .. } | Entry::Separator => {}
        }
    }
    if !items.is_empty() && items[0].id != 0 {
        items.insert(0, MenuItem { id: 0, text: view.label.clone() });
    }
    items
}

/// An item chosen from that menu, for `session`.
pub(crate) fn choose_for(actions: &Actions, shared: &Arc<Shared>, session: &str, id: u32) {
    let core = Core { shared: Arc::clone(shared) };
    let Some(view) = core.sessions().into_iter().find(|s| s.id == session) else { return };
    if let Some(tab) = tab_of(&view) {
        actions.chosen(&tab, id);
    }
}

/// `web01 (2)` → `web01`, for cloning.
pub(crate) fn base_label(label: &str) -> &str {
    if let Some(open) = label.rfind(" (") {
        let inner = &label[open + 2..];
        if inner.strip_suffix(')').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
            return &label[..open];
        }
    }
    label
}

fn action(id: u32, icon: Icon, text: &str, enabled: bool) -> Entry {
    Entry::Action { id, icon, text: text.to_string(), enabled }
}

/// The "close …" items and the set each one closes.
fn close_item(id: u32, this: &str) -> Option<CloseSet> {
    match id {
        CLOSE_OTHERS => Some(CloseSet::Others(this.to_string())),
        CLOSE_RIGHT => Some(CloseSet::RightOf(this.to_string())),
        CLOSE_ENDED => Some(CloseSet::Ended),
        _ => None,
    }
}

/// The session commands behind the menu's items.
fn command(id: u32) -> Option<SessionCommand> {
    match id {
        CONNECT => Some(SessionCommand::Connect),
        DISCONNECT => Some(SessionCommand::Disconnect),
        CLONE => Some(SessionCommand::Clone),
        CLOSE => Some(SessionCommand::Close),
        LOCK => Some(SessionCommand::ToggleLock),
        CLEAR => Some(SessionCommand::ClearScreen),
        SEND => Some(SessionCommand::Send),
        BREAK => Some(SessionCommand::SendBreak),
        _ => None,
    }
}

impl Provider for Actions {
    fn entries(&self, tab: &MenuTab) -> Vec<Entry> {
        let Some(shared) = self.core.upgrade() else { return Vec::new() };
        let core = Core { shared };
        let all: Vec<SessionView> = core.sessions().into_iter().filter(|s| s.state.is_open()).collect();
        let Some(this) = all.iter().find(|s| s.label == tab.label) else { return Vec::new() };
        let mut entries = Vec::new();
        if tab.mixed {
            entries.push(Entry::Header(t!("tabmenu-header-mixed", label = tab.label.as_str())));
            entries.push(Entry::Separator);
        } else if tab.title != tab.label {
            entries.push(Entry::Header(t!(
                "tabmenu-header-titled",
                label = tab.label.as_str(),
                title = tab.title.as_str()
            )));
            entries.push(Entry::Separator);
        }
        let applies = |id: u32| command(id).is_some_and(|c| c.applies(this));
        let connect = if this.state == State::Waiting { t!("tabmenu-connect") } else { t!("tabmenu-reconnect") };
        entries.push(action(CONNECT, Icon::Refresh, &connect, applies(CONNECT)));
        entries.push(action(DISCONNECT, Icon::Disconnect, &t!("tabmenu-disconnect"), applies(DISCONNECT)));
        entries.push(action(CLONE, Icon::Clone, &t!("tabmenu-clone"), applies(CLONE)));
        entries.push(action(SEND, Icon::Send, &t!("tabmenu-send"), applies(SEND)));
        // the main window knows the host: a non-SSH session gets a note there
        entries.push(action(FILES, Icon::Folder, &t!("tabmenu-files"), true));
        if SessionCommand::SendBreak.offered(this) {
            entries.push(action(BREAK, Icon::Break, &t!("tabmenu-break"), applies(BREAK)));
        }
        entries.push(action(CLEAR, Icon::ClearScreen, &t!("tabmenu-clear"), applies(CLEAR)));
        entries.push(action(RENAME, Icon::Rename, &t!("tabmenu-rename"), true));
        if this.locked {
            entries.push(action(LOCK, Icon::Unlock, &t!("tabmenu-unlock"), applies(LOCK)));
        } else {
            entries.push(action(LOCK, Icon::Lock, &t!("tabmenu-lock"), applies(LOCK)));
        }
        entries.push(Entry::Separator);
        let close = if tab.mixed { t!("tabmenu-close-mixed") } else { t!("tabmenu-close") };
        entries.push(action(CLOSE, Icon::Clear, &close, applies(CLOSE)));
        let some = |id: u32| close_item(id, &this.id).is_some_and(|set| !close_set(&all, &set).is_empty());
        entries.push(action(CLOSE_OTHERS, Icon::CloseOthers, &t!("tabmenu-close-others"), some(CLOSE_OTHERS)));
        entries.push(action(CLOSE_ENDED, Icon::CloseEnded, &t!("tabmenu-close-disconnected"), some(CLOSE_ENDED)));
        entries.push(action(CLOSE_RIGHT, Icon::CloseRight, &t!("tabmenu-close-right"), some(CLOSE_RIGHT)));
        entries
    }

    /// What the mouse resting on a tab shows: its picture, or the text
    /// its console holds when Terminal has never rendered it. Off when
    /// `HOVER_SETTING` says so.
    fn hover(&self, tab: &MenuTab) -> Option<(HoverCard, Duration)> {
        let core = Core { shared: self.core.upgrade()? };
        if core.setting(HOVER_SETTING).as_deref() == Some("off") {
            return None;
        }
        let session = core.sessions().into_iter().find(|s| s.state.is_open() && s.label == tab.label);
        let mut card = HoverCard {
            title: session.as_ref().map_or_else(|| tab.title.clone(), |s| s.label.clone()),
            ..Default::default()
        };
        let mut note = Vec::new();
        if tab.title != card.title {
            note.push(tab.title.clone());
        }
        if let Some(session) = &session {
            note.push(session.state.describe());
        }
        match core.preview(tab.window, tab.index) {
            Some(preview) => {
                let image = &preview.image;
                card.image = Some((image.width, image.height, image.rgba.clone()));
            }
            None => {
                // no picture of it: what its own console says (asked for
                // here, shown from the next card on)
                if let Some(session) = &session {
                    core.ask_screen(&session.id, SCREEN_EVERY);
                    if let Some(screen) = core.screen(&session.id) {
                        card.columns = screen.columns;
                        card.lines = screen.lines;
                    }
                }
                // a tab NativeTerm doesn't run: the screen read when it
                // was last seen (`previews`)
                if card.lines.is_empty() {
                    if let Some(lines) = core.preview_text(tab.window, tab.index) {
                        card.lines = lines.as_ref().clone();
                    }
                }
                if !card.lines.is_empty() {
                    note.push(t!("tabs-text-preview"));
                }
            }
        }
        card.note = note.join(" · ");
        // an empty card still waits: the screen was just asked for, and
        // the answer is there by the time the card would be shown
        (session.is_some() || !card.is_empty()).then_some((card, DELAY))
    }

    /// Every tab of the window Ctrl+Tab was pressed over, with the
    /// picture each was last seen with. Only asked for when the grid is
    /// turned on (the menu thread holds that flag).
    fn tiles(&self, window: WindowId) -> Vec<SwitcherTab> {
        let Some(shared) = self.core.upgrade() else { return Vec::new() };
        let core = Core { shared };
        let snapshot = core.snapshot();
        let Some(view) = snapshot.windows.iter().find(|w| w.handle == window) else { return Vec::new() };
        let sessions = core.sessions();
        view.tabs
            .iter()
            .map(|tab| {
                let session = tab
                    .claim
                    .as_ref()
                    .and_then(|claim| sessions.iter().find(|s| s.state.is_open() && s.label == claim.label));
                let mut tile = SwitcherTab {
                    window,
                    index: tab.index,
                    // NativeTerm's own tabs are known by their session's
                    // name, whatever the shell has titled them
                    title: session.map_or_else(|| tab.name.clone(), |s| s.label.clone()),
                    name: tab.name.clone(),
                    selected: tab.selected,
                    ..Default::default()
                };
                match core.preview(window, tab.index) {
                    Some(preview) => {
                        let image = &preview.image;
                        tile.image = Some((image.width, image.height, image.rgba.clone()));
                    }
                    None => {
                        // never rendered by Terminal: what its own console
                        // holds, else the screen read when it was last seen
                        if let Some(session) = session {
                            core.ask_screen(&session.id, SCREEN_EVERY);
                            if let Some(screen) = core.screen(&session.id) {
                                tile.columns = screen.columns;
                                tile.lines = screen.lines;
                            }
                        }
                        if tile.lines.is_empty() {
                            if let Some(lines) = core.preview_text(window, tab.index) {
                                tile.lines = lines.as_ref().clone();
                            }
                        }
                    }
                }
                tile
            })
            .collect()
    }

    fn switch(&self, window: WindowId, index: usize, name: &str) {
        let Some(shared) = self.core.upgrade() else { return };
        Core { shared }.select_tab(window, index, name);
    }

    fn chosen(&self, tab: &MenuTab, id: u32) {
        let Some(shared) = self.core.upgrade() else { return };
        let core = Core { shared };
        let Some(this) = core.sessions().into_iter().find(|s| s.state.is_open() && s.label == tab.label) else {
            return;
        };
        match (id, command(id), close_item(id, &this.id)) {
            (SEND, _, _) => (self.ask)(MenuRequest::Send(this.id.clone())),
            (RENAME, _, _) => (self.ask)(MenuRequest::Rename(this.alias.clone())),
            (FILES, _, _) => (self.ask)(MenuRequest::Files { alias: this.alias.clone(), session: this.id.clone() }),
            (_, Some(command), _) => core.run(&this.id, command),
            (_, _, Some(set)) => {
                // a tab that holds other panes is closed only when confirmed
                if let Closing::Confirm(ids) = core.close_sessions(&set) {
                    (self.ask)(MenuRequest::ConfirmClose(ids));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_label() {
        assert_eq!(base_label("web01 (2)"), "web01");
        assert_eq!(base_label("web01 (prod)"), "web01 (prod)");
        assert_eq!(base_label("节点 (12)"), "节点");
        assert_eq!(base_label("x ()"), "x ()");
    }
}
