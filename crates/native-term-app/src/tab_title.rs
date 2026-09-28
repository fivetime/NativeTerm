//! A tab's title for as long as the tab lives ("Rename Tab…" in the menu
//! of a tab without a session): the terminal sets it; NativeTerm asks
//! for it in a small window of its own. Nothing is kept: a tab that has
//! no session is nothing NativeTerm knows again.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::time::Duration;

use crate::tab_menu::MenuRequest;

/// A window nobody answers is given up.
const PATIENCE: Duration = Duration::from_secs(600);
/// A title longer than this is cut: a tab has no room for it.
const LONGEST: usize = 120;

static NEXT: AtomicU64 = AtomicU64::new(1);
static WAITING: Mutex<Option<HashMap<u64, Sender<Option<String>>>>> = Mutex::new(None);

fn waiting<R>(with: impl FnOnce(&mut HashMap<u64, Sender<Option<String>>>) -> R) -> R {
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    with(waiting.get_or_insert_with(HashMap::new))
}

/// A title as a tab can have it: on one line, without blanks around it,
/// not longer than a tab shows. An empty one is the terminal's own again.
#[must_use]
pub fn cleaned(title: &str) -> String {
    let line: String = title.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    line.trim().chars().take(LONGEST).collect::<String>().trim_end().to_string()
}

/// The window's answer to the question `ticket` names: the title, or
/// `None` for the tab to keep its own.
pub fn answer(ticket: u64, answer: Option<String>) {
    if let Some(tx) = waiting(|w| w.remove(&ticket)) {
        let _ = tx.send(answer);
    }
}

/// The title the person gives the tab called `current` in the window
/// `ask` brings up; `None`: they said no, or there is no window to ask
/// in. Waits for the answer: not for the GUI's thread.
pub(crate) fn ask(ask: Option<&(dyn Fn(MenuRequest) + Send + Sync)>, current: String) -> Option<String> {
    let ask = ask?;
    let (tx, rx) = mpsc::channel();
    let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
    // one question at a time: a second one while the window is up is
    // dropped, as a second click on the menu's item would be
    let first = waiting(|w| {
        let first = w.is_empty();
        if first {
            w.insert(ticket, tx);
        }
        first
    });
    if !first {
        return None;
    }
    ask(MenuRequest::TabTitle { ticket, current });
    let answer = rx.recv_timeout(PATIENCE).ok().flatten();
    waiting(|w| w.remove(&ticket));
    answer.map(|title| cleaned(&title))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_one_short_line() {
        assert_eq!(cleaned("  build\tlogs\n"), "build logs");
        assert_eq!(cleaned(" \r\n"), "", "the terminal's own again");
        assert_eq!(cleaned(&"长".repeat(300)).chars().count(), LONGEST);
    }

    #[test]
    fn an_answer_reaches_who_waits_for_it() {
        let (tx, rx) = mpsc::channel();
        waiting(|w| w.insert(u64::MAX, tx));
        answer(u64::MAX - 1, None);
        assert!(rx.try_recv().is_err(), "another question's answer");
        answer(u64::MAX, Some("build".into()));
        assert_eq!(rx.try_recv().unwrap(), Some("build".to_string()));
        waiting(|w| w.remove(&u64::MAX));
    }
}
