//! A password ssh asks for where none is saved: asked in a window of
//! NativeTerm's own, as SecureCRT asks it ("Enter Secure Shell
//! Password"). The shim that serves ssh's askpass asks here and waits;
//! the window answers with the password (and whether to keep it once the
//! login worked), or to ask in the tab after all, or to give up.
//!
//! The password goes from the window to the shim that asked, over the
//! user's own pipe, and is kept nowhere here.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::time::Duration;

pub use native_term_session::protocol::PasswordAnswer;

use crate::tab_menu::MenuRequest;

/// A window nobody answers is given up: the tab asks then.
const PATIENCE: Duration = Duration::from_secs(600);

static NEXT: AtomicU64 = AtomicU64::new(1);
/// The questions asked and not answered yet: where the answer goes, and
/// what was asked.
type Waiting = HashMap<u64, (Sender<PasswordAnswer>, Question)>;

static WAITING: Mutex<Option<Waiting>> = Mutex::new(None);

fn waiting<R>(with: impl FnOnce(&mut Waiting) -> R) -> R {
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    with(waiting.get_or_insert_with(HashMap::new))
}

/// What is asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    /// The session's name, where the tab is one of NativeTerm's.
    pub label: Option<String>,
    pub user: String,
    pub host: String,
    /// The password given before was wrong.
    pub retry: bool,
    /// The saved one was refused by the server.
    pub refused: bool,
    /// There is a store to keep it in.
    pub can_save: bool,
}

/// The window's answer to the question `ticket` names.
pub fn answer(ticket: u64, answer: PasswordAnswer) {
    if let Some((tx, _)) = waiting(|w| w.remove(&ticket)) {
        let _ = tx.send(answer);
    }
}

/// What the person says in the window `ask` brings up; `Skip` where
/// there is no window to ask in, or nobody answers. Waits for the
/// answer: not for the GUI's thread.
pub(crate) fn ask(ask: Option<&(dyn Fn(MenuRequest) + Send + Sync)>, question: Question) -> PasswordAnswer {
    let Some(ask) = ask else { return PasswordAnswer::Skip };
    let (tx, rx) = mpsc::channel();
    let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
    waiting(|w| w.insert(ticket, (tx, question.clone())));
    ask(MenuRequest::Password { ticket, question });
    let answer = rx.recv_timeout(PATIENCE).unwrap_or(PasswordAnswer::Skip);
    waiting(|w| w.remove(&ticket));
    answer
}

/// The windows asking still, in front again: the terminal was just
/// brought forward over them (its tab opened a moment before ssh asked).
/// Asked for again, a window that is open is only brought forward.
pub(crate) fn raise(ask: &(dyn Fn(MenuRequest) + Send + Sync)) {
    let open: Vec<(u64, Question)> = waiting(|w| w.iter().map(|(t, (_, q))| (*t, q.clone())).collect());
    for (ticket, question) in open {
        ask(MenuRequest::Password { ticket, question });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_reaches_the_one_who_asked() {
        let question = Question {
            label: Some("web01".into()),
            user: "root".into(),
            host: "192.0.2.1".into(),
            retry: false,
            refused: false,
            can_save: true,
        };
        let asked = std::sync::Arc::new(Mutex::new(Vec::new()));
        let seen = std::sync::Arc::clone(&asked);
        let window = move |request: MenuRequest| {
            let MenuRequest::Password { ticket, question } = request else { panic!("{request:?}") };
            seen.lock().unwrap().push(question);
            // the person types, a moment later
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                answer(ticket, PasswordAnswer::Given { secret: "s3cret".into(), save: true, user: None });
            });
        };
        let got = ask(Some(&window), question.clone());
        assert_eq!(got, PasswordAnswer::Given { secret: "s3cret".into(), save: true, user: None });
        assert_eq!(asked.lock().unwrap().as_slice(), std::slice::from_ref(&question));
        // no window to ask in: the tab asks
        assert_eq!(ask(None, question), PasswordAnswer::Skip);
        // an answer nobody waits for is dropped
        answer(999_999, PasswordAnswer::Cancel);
    }
}
