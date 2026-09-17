//! What NativeTerm's tab menu offers and does. Only NativeTerm's own tabs
//! are ever touched; the user's other tabs never are.

use std::sync::{Arc, Weak};

use native_term_platform::windows_terminal::menu::{Entry, MenuTab, Provider};
use native_term_platform::Target;

use crate::{lock, t, Core, HostRequest, Shared, State};

pub const CONNECT: u32 = 1;
pub const DISCONNECT: u32 = 2;
pub const CLONE: u32 = 3;
pub const CLOSE: u32 = 4;
pub const CLOSE_OTHERS: u32 = 5;
pub const CLOSE_ENDED: u32 = 6;
pub const CLOSE_RIGHT: u32 = 7;
pub const SEND: u32 = 8;
pub const LOCK: u32 = 9;

pub(crate) struct Actions {
    pub(crate) core: Weak<Shared>,
    /// Opens the send dialog (the window lives in the binary).
    pub(crate) send: Arc<dyn Fn(&str) + Send + Sync>,
}

/// A session as the menu sees it.
struct MenuSession {
    id: String,
    alias: String,
    label: String,
    state: State,
    linked: bool,
    window: Option<isize>,
    index: Option<usize>,
    locked: bool,
}

fn sessions(shared: &Shared) -> Vec<MenuSession> {
    lock(&shared.sessions)
        .iter()
        .filter(|s| s.state.is_open())
        .map(|s| MenuSession {
            id: s.id.clone(),
            alias: s.alias.clone(),
            label: s.label.clone(),
            state: s.state.clone(),
            linked: s.link.is_some(),
            window: s.location.as_ref().map(|l| l.window),
            index: s.location.as_ref().map(|l| l.tab_index),
            locked: s.locked,
        })
        .collect()
}

fn ended(state: &State) -> bool {
    matches!(state, State::LoginFailed(_) | State::Disconnected(_) | State::Ended(_))
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

/// Sessions the "close …" items would close, for the tab `tab`; locked
/// ones never.
fn to_close(all: &[MenuSession], tab: &MenuTab, id: u32) -> Vec<String> {
    all.iter()
        .filter(|s| !s.locked)
        .filter(|s| match id {
            CLOSE_OTHERS => s.window == Some(tab.window) && s.label != tab.label,
            CLOSE_RIGHT => s.window == Some(tab.window) && s.index.is_some_and(|i| i > tab.index),
            CLOSE_ENDED => ended(&s.state),
            _ => false,
        })
        .map(|s| s.id.clone())
        .collect()
}

fn action(id: u32, glyph: char, text: &str, enabled: bool) -> Entry {
    Entry::Action { id, glyph, text: text.to_string(), enabled }
}

impl Provider for Actions {
    fn entries(&self, tab: &MenuTab) -> Vec<Entry> {
        let Some(shared) = self.core.upgrade() else { return Vec::new() };
        let all = sessions(&shared);
        let Some(this) = all.iter().find(|s| s.label == tab.label) else { return Vec::new() };
        let mut entries = Vec::new();
        if tab.mixed {
            entries.push(Entry::Header(t!("tabmenu-header-mixed", label = tab.label.as_str())));
            entries.push(Entry::Separator);
        } else if tab.title != tab.label {
            entries.push(Entry::Header(t!("tabmenu-header-titled", label = tab.label.as_str(), title = tab.title.as_str())));
            entries.push(Entry::Separator);
        }
        let connect = if this.state == State::Waiting { t!("tabmenu-connect") } else { t!("tabmenu-reconnect") };
        entries.push(action(CONNECT, '\u{E72C}', &connect, this.linked && this.state.can_connect()));
        let live = matches!(this.state, State::Connecting | State::Connected);
        entries.push(action(DISCONNECT, '\u{E8CD}', &t!("tabmenu-disconnect"), this.linked && live));
        entries.push(action(CLONE, '\u{E8C8}', &t!("tabmenu-clone"), true));
        entries.push(action(SEND, '\u{E724}', &t!("tabmenu-send"), this.linked && this.state == State::Connected));
        if this.locked {
            entries.push(action(LOCK, '\u{E785}', &t!("tabmenu-unlock"), true));
        } else {
            entries.push(action(LOCK, '\u{E72E}', &t!("tabmenu-lock"), true));
        }
        entries.push(Entry::Separator);
        let close = if tab.mixed { t!("tabmenu-close-mixed") } else { t!("tabmenu-close") };
        entries.push(action(CLOSE, '\u{E711}', &close, !this.locked));
        let others = !to_close(&all, tab, CLOSE_OTHERS).is_empty();
        entries.push(action(CLOSE_OTHERS, '\u{E8BB}', &t!("tabmenu-close-others"), others));
        let ended = !to_close(&all, tab, CLOSE_ENDED).is_empty();
        entries.push(action(CLOSE_ENDED, '\u{E894}', &t!("tabmenu-close-disconnected"), ended));
        let right = !to_close(&all, tab, CLOSE_RIGHT).is_empty();
        entries.push(action(CLOSE_RIGHT, '\u{E72A}', &t!("tabmenu-close-right"), right));
        entries
    }

    fn chosen(&self, tab: &MenuTab, id: u32) {
        let Some(shared) = self.core.upgrade() else { return };
        let core = Core { shared: Arc::clone(&shared) };
        let all = sessions(&shared);
        let Some(this) = all.iter().find(|s| s.label == tab.label) else { return };
        match id {
            CONNECT => core.connect(&this.id),
            DISCONNECT => core.disconnect(&this.id),
            CLONE => {
                let on_login = lock(&shared.sessions).iter().find(|s| s.id == this.id).and_then(|s| s.on_login.clone());
                let host = HostRequest { no_forwards: true, on_login, ..HostRequest::new(&this.alias, base_label(&this.label)) };
                core.open(&[host], Target::Recent);
            }
            CLOSE if !this.locked => core.close(&this.id),
            LOCK => core.set_locked(&this.id, !this.locked),
            SEND => (self.send)(&this.id),
            CLOSE_OTHERS | CLOSE_ENDED | CLOSE_RIGHT => {
                for id in to_close(&all, tab, id) {
                    core.close(&id);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_term_platform::Rect;

    fn session(id: &str, window: isize, index: usize, state: State) -> MenuSession {
        MenuSession {
            id: id.into(),
            alias: "h".into(),
            label: id.into(),
            state,
            linked: true,
            window: Some(window),
            index: Some(index),
            locked: false,
        }
    }

    fn tab(label: &str, window: isize, index: usize) -> MenuTab {
        MenuTab { window, rect: Rect::default(), label: label.into(), title: label.into(), mixed: false, index }
    }

    #[test]
    fn close_sets_stay_within_nativeterm_tabs() {
        let all = vec![
            session("a", 1, 0, State::Connected),
            // the user's own tabs sit at 1 and 3, they aren't sessions
            session("b", 1, 2, State::Disconnected(255)),
            session("c", 1, 4, State::Connected),
            session("d", 2, 0, State::LoginFailed(255)),
        ];
        let t = tab("b", 1, 2);
        assert_eq!(to_close(&all, &t, CLOSE_OTHERS), ["a", "c"]);
        assert_eq!(to_close(&all, &t, CLOSE_RIGHT), ["c"]);
        assert_eq!(to_close(&all, &t, CLOSE_ENDED), ["b", "d"], "all windows");
        assert!(to_close(&all, &tab("c", 1, 4), CLOSE_RIGHT).is_empty());

        let mut all = all;
        all[2].locked = true;
        all[3].locked = true;
        assert_eq!(to_close(&all, &t, CLOSE_OTHERS), ["a"], "locked sessions stay");
        assert!(to_close(&all, &t, CLOSE_RIGHT).is_empty());
        assert_eq!(to_close(&all, &t, CLOSE_ENDED), ["b"]);
    }

    #[test]
    fn clone_label() {
        assert_eq!(base_label("web01 (2)"), "web01");
        assert_eq!(base_label("web01 (prod)"), "web01 (prod)");
        assert_eq!(base_label("节点 (12)"), "节点");
        assert_eq!(base_label("x ()"), "x ()");
    }
}
