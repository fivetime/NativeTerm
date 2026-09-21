//! The session commands every UI surface offers — the session card, the
//! tab menu, the floating button — decided and carried out in one place:
//! when each one applies, what it does, and which sessions a batch close
//! takes. Surfaces only draw them and say which one was chosen.

use crate::{Core, SessionView, State};

/// What can be done to one session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionCommand {
    /// Connect a waiting session, or reconnect an ended one.
    Connect,
    Disconnect,
    /// The same host again, without port forwards.
    Clone,
    Close,
    /// Lock or unlock (kept out of batch closes and group sends).
    ToggleLock,
    ClearScreen,
    /// Select its tab and bring its window to the front.
    Focus,
    /// Type commands into it (the surface opens its dialog).
    Send,
    /// A serial line's Break, or Telnet's (non-SSH sessions run by ntplink).
    SendBreak,
}

impl SessionCommand {
    /// Whether it applies to `s` now (the surface greys it out otherwise).
    pub fn applies(self, s: &SessionView) -> bool {
        let open = s.state.is_open();
        match self {
            SessionCommand::Connect => open && s.linked && s.state.can_connect(),
            SessionCommand::Disconnect => open && s.linked && matches!(s.state, State::Connecting | State::Connected),
            SessionCommand::Clone => true,
            SessionCommand::Close => open && !s.locked,
            SessionCommand::ToggleLock => open,
            SessionCommand::ClearScreen => open && s.linked,
            SessionCommand::Focus => s.location.is_some(),
            SessionCommand::Send => s.linked && s.state == State::Connected,
            SessionCommand::SendBreak => s.linked && s.state == State::Connected && s.can_break(),
        }
    }

    /// Surfaces leave it out entirely where it can never apply.
    pub fn offered(self, s: &SessionView) -> bool {
        match self {
            SessionCommand::SendBreak => s.can_break(),
            _ => true,
        }
    }
}

impl SessionView {
    /// Its connection takes a Break now.
    pub fn can_break(&self) -> bool {
        self.specials.iter().any(|n| n == BREAK)
    }
}

/// ntplink's name for Break.
pub const BREAK: &str = "brk";

/// The sessions a batch close takes. Locked sessions never, and only
/// NativeTerm's own: the user's tabs between or beside them aren't
/// sessions and are never touched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseSet {
    /// The other sessions in this session's window.
    Others(String),
    /// Sessions to the right of this one in its window.
    RightOf(String),
    /// Every session whose connection ended or failed, in all windows.
    Ended,
    /// Restored sessions still waiting to connect.
    Waiting,
}

pub fn close_set(sessions: &[SessionView], set: &CloseSet) -> Vec<String> {
    let place = |id: &str| {
        sessions.iter().find(|s| s.id == id).and_then(|s| s.location.as_ref()).map(|l| (l.window, l.tab_index))
    };
    sessions
        .iter()
        .filter(|s| s.state.is_open() && !s.locked)
        .filter(|s| {
            let here = s.location.as_ref().map(|l| (l.window, l.tab_index));
            match set {
                CloseSet::Others(id) => {
                    s.id != *id && place(id).zip(here).is_some_and(|((window, _), (w, _))| w == window)
                }
                CloseSet::RightOf(id) => {
                    place(id).zip(here).is_some_and(|((window, index), (w, i))| w == window && i > index)
                }
                CloseSet::Ended => matches!(
                    s.state,
                    State::LoginFailed(_) | State::Unreachable(_) | State::Disconnected(_) | State::Ended(_)
                ),
                CloseSet::Waiting => s.state == State::Waiting && s.linked,
            }
        })
        .map(|s| s.id.clone())
        .collect()
}

/// How a batch close went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Closing {
    /// This many closed.
    Done(usize),
    /// Nothing closed yet: one of these tabs holds other panes as well,
    /// so the user confirms first (then `Core::close_ids`).
    Confirm(Vec<String>),
}

impl Core {
    /// Carry out `command` on session `id`, if it applies. `Send` is the
    /// surface's own (a dialog or a line), so nothing happens here.
    pub fn run(&self, id: &str, command: SessionCommand) {
        let Some(s) = self.sessions().into_iter().find(|s| s.id == id) else { return };
        if !command.applies(&s) {
            return;
        }
        match command {
            SessionCommand::Connect => self.connect(id),
            SessionCommand::Disconnect => self.disconnect(id),
            SessionCommand::Clone => self.clone_session(id),
            SessionCommand::Close => self.close(id),
            SessionCommand::ToggleLock => self.set_locked(id, !s.locked),
            SessionCommand::ClearScreen => self.clear_screen(id),
            SessionCommand::Focus => self.focus(id),
            SessionCommand::SendBreak => self.send_special(id, BREAK),
            SessionCommand::Send => {}
        }
    }

    /// Close a set of sessions, unless it takes a tab that holds other
    /// panes too: then nothing is closed and the ids are returned to
    /// confirm.
    pub fn close_sessions(&self, set: &CloseSet) -> Closing {
        let sessions = self.sessions();
        let ids = close_set(&sessions, set);
        let mixed = sessions.iter().any(|s| ids.contains(&s.id) && s.location.as_ref().is_some_and(|l| l.mixed));
        if mixed {
            return Closing::Confirm(ids);
        }
        self.close_ids(&ids);
        Closing::Done(ids.len())
    }

    /// Close these sessions (after a confirmation); locked ones stay.
    pub fn close_ids(&self, ids: &[String]) {
        let locked: Vec<String> = self.sessions().into_iter().filter(|s| s.locked).map(|s| s.id).collect();
        let closing: Vec<&String> = ids.iter().filter(|id| !locked.contains(id)).collect();
        // closing several tabs at once leaves nothing on screen to say so
        if closing.len() > 1 {
            crate::toast::done(crate::t!("toast-closed", count = closing.len()));
        }
        for id in closing {
            self.close(id);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::Location;

    pub(crate) fn session(id: &str, window: isize, tab_index: usize, state: State) -> SessionView {
        SessionView {
            id: id.into(),
            label: id.into(),
            alias: "h".into(),
            state,
            shim_pid: None,
            attempt: 1,
            linked: true,
            location: Some(Location {
                window_number: window as usize,
                window,
                tab_index,
                title: id.into(),
                selected: false,
                mixed: false,
            }),
            auto_retry: None,
            locked: false,
            renamed_to: None,
            last_position: None,
            quiet_since: None,
            specials: Vec::new(),
        }
    }

    #[test]
    fn close_sets_stay_within_nativeterm_tabs() {
        let all = vec![
            session("a", 1, 0, State::Connected),
            // the user's own tabs sit at 1 and 3, they aren't sessions
            session("b", 1, 2, State::Disconnected(255)),
            session("c", 1, 4, State::Connected),
            session("d", 2, 0, State::LoginFailed(255)),
            session("w", 2, 1, State::Waiting),
        ];
        let others = CloseSet::Others("b".into());
        let right = CloseSet::RightOf("b".into());
        assert_eq!(close_set(&all, &others), ["a", "c"]);
        assert_eq!(close_set(&all, &right), ["c"]);
        assert_eq!(close_set(&all, &CloseSet::Ended), ["b", "d"], "all windows");
        assert_eq!(close_set(&all, &CloseSet::Waiting), ["w"]);
        assert!(close_set(&all, &CloseSet::RightOf("c".into())).is_empty());

        let mut all = all;
        all[2].locked = true;
        all[3].locked = true;
        assert_eq!(close_set(&all, &others), ["a"], "locked sessions stay");
        assert!(close_set(&all, &right).is_empty());
        assert_eq!(close_set(&all, &CloseSet::Ended), ["b"]);
    }

    /// Break only where the connection takes one (ntplink told the shim),
    /// and only while connected.
    #[test]
    fn break_is_offered_where_the_connection_takes_it() {
        let mut s = session("a", 1, 0, State::Connected);
        assert!(!SessionCommand::SendBreak.offered(&s) && !SessionCommand::SendBreak.applies(&s), "ssh: never");
        s.specials = vec!["brk".into()];
        assert!(SessionCommand::SendBreak.offered(&s) && SessionCommand::SendBreak.applies(&s));
        s.state = State::Connecting;
        assert!(SessionCommand::SendBreak.offered(&s) && !SessionCommand::SendBreak.applies(&s));
    }

    #[test]
    fn when_commands_apply() {
        let mut s = session("a", 1, 0, State::Connected);
        assert!(SessionCommand::Disconnect.applies(&s) && SessionCommand::Send.applies(&s));
        assert!(!SessionCommand::Connect.applies(&s));
        s.state = State::Disconnected(255);
        assert!(SessionCommand::Connect.applies(&s) && !SessionCommand::Send.applies(&s));
        s.linked = false;
        assert!(!SessionCommand::Connect.applies(&s), "no shim to tell");
        assert!(SessionCommand::Close.applies(&s), "closing works without a shim (the tab's button)");
        s.locked = true;
        assert!(!SessionCommand::Close.applies(&s) && SessionCommand::ToggleLock.applies(&s));
        s.state = State::Closed;
        assert!(!SessionCommand::ToggleLock.applies(&s) && !SessionCommand::Close.applies(&s));
        s.location = None;
        assert!(!SessionCommand::Focus.applies(&s) && SessionCommand::Clone.applies(&s));
    }
}
