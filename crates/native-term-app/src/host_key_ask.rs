//! A host key ssh asks about, decided in a window of NativeTerm's own, as
//! SecureCRT asks ("New Host Key", "Host Key Changed"): a host not known
//! yet (keep its key, trust it this once, or don't connect), or a host
//! whose key differs from the known one (take the old one out and keep
//! the new one, or don't connect). The shim that serves ssh's askpass asks
//! here and waits (see the shim's `hostkey.rs`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::time::Duration;

pub use native_term_session::protocol::{HostKeyAnswer, OldHostKey};

use crate::tab_menu::MenuRequest;

/// A window nobody answers is given up: the connection is refused.
const PATIENCE: Duration = Duration::from_secs(600);

static NEXT: AtomicU64 = AtomicU64::new(1);
type Waiting = HashMap<u64, (Sender<HostKeyAnswer>, Question)>;
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
    pub host: String,
    pub ip: String,
    pub key_type: String,
    pub fingerprint: String,
    /// The key known before: the key changed.
    pub old: Option<OldHostKey>,
}

/// The window's answer to the question `ticket` names.
pub fn answer(ticket: u64, answer: HostKeyAnswer) {
    if let Some((tx, _)) = waiting(|w| w.remove(&ticket)) {
        let _ = tx.send(answer);
    }
}

/// What the person says in the window `ask` brings up; `None` where
/// there is no window to ask in. Waits: not for the GUI's thread.
pub(crate) fn ask(ask: Option<&(dyn Fn(MenuRequest) + Send + Sync)>, question: Question) -> Option<HostKeyAnswer> {
    let ask = ask?;
    let (tx, rx) = mpsc::channel();
    let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
    waiting(|w| w.insert(ticket, (tx, question.clone())));
    ask(MenuRequest::HostKey { ticket, question });
    // nobody answered: not trusted
    let answer = rx.recv_timeout(PATIENCE).unwrap_or(HostKeyAnswer::Cancel);
    waiting(|w| w.remove(&ticket));
    Some(answer)
}

/// The windows asking still, in front again (the terminal was brought
/// forward over them).
pub(crate) fn raise(ask: &(dyn Fn(MenuRequest) + Send + Sync)) {
    let open: Vec<(u64, Question)> = waiting(|w| w.iter().map(|(t, (_, q))| (*t, q.clone())).collect());
    for (ticket, question) in open {
        ask(MenuRequest::HostKey { ticket, question });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_reaches_the_one_who_asked() {
        let question = Question {
            label: Some("web01".into()),
            host: "web01.lan".into(),
            ip: "192.0.2.1".into(),
            key_type: "ED25519".into(),
            fingerprint: "SHA256:new".into(),
            old: None,
        };
        let window = move |request: MenuRequest| {
            let MenuRequest::HostKey { ticket, .. } = request else { panic!("{request:?}") };
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                answer(ticket, HostKeyAnswer::Once);
            });
        };
        assert_eq!(ask(Some(&window), question.clone()), Some(HostKeyAnswer::Once));
        // no window: the shim decides (the tab asks, or it is refused)
        assert_eq!(ask(None, question), None);
    }
}
