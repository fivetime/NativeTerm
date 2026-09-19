//! What NativeTerm's tab menu offers and does. Only NativeTerm's own tabs
//! are ever touched; the user's other tabs never are.

use std::sync::{Arc, Weak};

use native_term_platform::windows_terminal::menu::{Entry, MenuTab, Provider};

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
}

pub(crate) struct Actions {
    pub(crate) core: Weak<Shared>,
    /// Asks the main window; its dialogs live in the binary.
    pub(crate) ask: Arc<dyn Fn(MenuRequest) + Send + Sync>,
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

fn action(id: u32, glyph: char, text: &str, enabled: bool) -> Entry {
    Entry::Action { id, glyph, text: text.to_string(), enabled }
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
            entries.push(Entry::Header(t!("tabmenu-header-titled", label = tab.label.as_str(), title = tab.title.as_str())));
            entries.push(Entry::Separator);
        }
        let applies = |id: u32| command(id).is_some_and(|c| c.applies(this));
        let connect = if this.state == State::Waiting { t!("tabmenu-connect") } else { t!("tabmenu-reconnect") };
        entries.push(action(CONNECT, '\u{E72C}', &connect, applies(CONNECT)));
        entries.push(action(DISCONNECT, '\u{E8CD}', &t!("tabmenu-disconnect"), applies(DISCONNECT)));
        entries.push(action(CLONE, '\u{E8C8}', &t!("tabmenu-clone"), applies(CLONE)));
        entries.push(action(SEND, '\u{E724}', &t!("tabmenu-send"), applies(SEND)));
        // the main window knows the host: a non-SSH session gets a note there
        entries.push(action(FILES, '\u{E8B7}', &t!("tabmenu-files"), true));
        if SessionCommand::SendBreak.offered(this) {
            entries.push(action(BREAK, '\u{E7BA}', &t!("tabmenu-break"), applies(BREAK)));
        }
        entries.push(action(CLEAR, '\u{E75C}', &t!("tabmenu-clear"), applies(CLEAR)));
        entries.push(action(RENAME, '\u{E8AC}', &t!("tabmenu-rename"), true));
        if this.locked {
            entries.push(action(LOCK, '\u{E785}', &t!("tabmenu-unlock"), applies(LOCK)));
        } else {
            entries.push(action(LOCK, '\u{E72E}', &t!("tabmenu-lock"), applies(LOCK)));
        }
        entries.push(Entry::Separator);
        let close = if tab.mixed { t!("tabmenu-close-mixed") } else { t!("tabmenu-close") };
        entries.push(action(CLOSE, '\u{E711}', &close, applies(CLOSE)));
        let some = |id: u32| close_item(id, &this.id).is_some_and(|set| !close_set(&all, &set).is_empty());
        entries.push(action(CLOSE_OTHERS, '\u{E8BB}', &t!("tabmenu-close-others"), some(CLOSE_OTHERS)));
        entries.push(action(CLOSE_ENDED, '\u{E894}', &t!("tabmenu-close-disconnected"), some(CLOSE_ENDED)));
        entries.push(action(CLOSE_RIGHT, '\u{E72A}', &t!("tabmenu-close-right"), some(CLOSE_RIGHT)));
        entries
    }

    fn chosen(&self, tab: &MenuTab, id: u32) {
        let Some(shared) = self.core.upgrade() else { return };
        let core = Core { shared };
        let Some(this) = core.sessions().into_iter().find(|s| s.state.is_open() && s.label == tab.label) else { return };
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
