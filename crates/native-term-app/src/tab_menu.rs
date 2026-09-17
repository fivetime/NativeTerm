//! What NativeTerm's tab menu offers and does. Only NativeTerm's own tabs
//! are ever touched; the user's other tabs never are.

use std::sync::{Arc, Weak};

use native_term_platform::windows_terminal::menu::{Entry, MenuTab, Provider};
use native_term_platform::Target;

use crate::{lock, Core, HostRequest, Shared, State};

pub const CONNECT: u32 = 1;
pub const DISCONNECT: u32 = 2;
pub const CLONE: u32 = 3;
pub const CLOSE: u32 = 4;
pub const CLOSE_OTHERS: u32 = 5;
pub const CLOSE_ENDED: u32 = 6;
pub const CLOSE_RIGHT: u32 = 7;

pub(crate) struct Actions {
    pub(crate) core: Weak<Shared>,
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
        })
        .collect()
}

fn ended(state: &State) -> bool {
    matches!(state, State::LoginFailed(_) | State::Disconnected(_) | State::Ended(_))
}

/// `web01 (2)` → `web01`, for cloning.
fn base_label(label: &str) -> &str {
    if let Some(open) = label.rfind(" (") {
        let inner = &label[open + 2..];
        if inner.strip_suffix(')').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
            return &label[..open];
        }
    }
    label
}

/// Sessions the "close …" items would close, for the tab `tab`.
fn to_close(all: &[MenuSession], tab: &MenuTab, id: u32) -> Vec<String> {
    all.iter()
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
            entries.push(Entry::Header(format!("{} · this tab has other panes too", tab.label)));
            entries.push(Entry::Separator);
        } else if tab.title != tab.label {
            entries.push(Entry::Header(format!("{} · titled \u{201c}{}\u{201d}", tab.label, tab.title)));
            entries.push(Entry::Separator);
        }
        let connect = if this.state == State::Waiting { "Connect" } else { "Reconnect" };
        entries.push(action(CONNECT, '\u{E72C}', connect, this.linked && this.state.can_connect()));
        let live = matches!(this.state, State::Connecting | State::Connected);
        entries.push(action(DISCONNECT, '\u{E8CD}', "Disconnect", this.linked && live));
        entries.push(action(CLONE, '\u{E8C8}', "Clone Session", true));
        entries.push(Entry::Separator);
        let close = if tab.mixed { "Close This Session (keeps the other panes)" } else { "Close" };
        entries.push(action(CLOSE, '\u{E711}', close, true));
        entries.push(action(CLOSE_OTHERS, '\u{E8BB}', "Close Other NativeTerm Tabs", !to_close(&all, tab, CLOSE_OTHERS).is_empty()));
        entries.push(action(CLOSE_ENDED, '\u{E894}', "Close Disconnected Tabs", !to_close(&all, tab, CLOSE_ENDED).is_empty()));
        entries.push(action(CLOSE_RIGHT, '\u{E72A}', "Close Tabs to the Right", !to_close(&all, tab, CLOSE_RIGHT).is_empty()));
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
                let host = HostRequest { alias: this.alias.clone(), label: base_label(&this.label).to_string() };
                core.open(&[host], Target::Recent);
            }
            CLOSE => core.close(&this.id),
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
    }

    #[test]
    fn clone_label() {
        assert_eq!(base_label("web01 (2)"), "web01");
        assert_eq!(base_label("web01 (prod)"), "web01 (prod)");
        assert_eq!(base_label("节点 (12)"), "节点");
        assert_eq!(base_label("x ()"), "x ()");
    }
}
