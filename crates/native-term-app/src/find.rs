//! "Find", as SecureCRT's dialog: what to find, whole words only, the
//! case, around the ends or not, up or down, and "Find Next". The terminal
//! does the finding (it selects the match and brings it into view) and
//! asks here what to find; NativeTerm shows the dialog, a small window of
//! its own that stays while the person goes from match to match. Each
//! "Find Next" answers one question; the terminal finds, and asks again,
//! saying what the find came to.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;

use native_term_session::protocol::FindResult;

use crate::tab_menu::MenuRequest;
use crate::Core;

/// `on`: the case matters.
pub const CASE_SETTING: &str = "find.match_case";
/// `on`: whole words only.
pub const WORD_SETTING: &str = "find.whole_word";
/// `off`: not around the ends.
pub const WRAP_SETTING: &str = "find.wrap";
/// `down`: towards the end; up otherwise, as SecureCRT starts.
pub const DIRECTION_SETTING: &str = "find.direction";

/// Where the person left the dialog: its top left corner, `x,y` in the
/// screen's pixels (this machine's).
pub const PLACE_SETTING: &str = "find.window";

/// The place the setting names.
#[must_use]
pub fn place_from_setting(text: &str) -> Option<(i32, i32)> {
    let (x, y) = text.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// What to find, and how.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Find {
    pub text: String,
    pub match_case: bool,
    pub whole_word: bool,
    pub wrap: bool,
    pub up: bool,
}

impl Find {
    /// How the person found last, for `text`.
    #[must_use]
    pub fn saved(core: &Core, text: String) -> Find {
        let is = |key: &str, value: &str| core.setting(key).as_deref() == Some(value);
        Find {
            text,
            match_case: is(CASE_SETTING, "on"),
            whole_word: is(WORD_SETTING, "on"),
            wrap: !is(WRAP_SETTING, "off"),
            up: !is(DIRECTION_SETTING, "down"),
        }
    }

    pub fn save(&self, core: &Core) {
        let on = |on: bool| if on { "on" } else { "off" };
        core.set_setting(CASE_SETTING, on(self.match_case));
        core.set_setting(WORD_SETTING, on(self.whole_word));
        core.set_setting(WRAP_SETTING, on(self.wrap));
        core.set_setting(DIRECTION_SETTING, if self.up { "up" } else { "down" });
    }
}

/// What the terminal asks: what to find, the first time (with what the
/// field may start with: the pane's selection), or what to find next,
/// after a find that came to `result`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub ticket: u64,
    pub initial: Option<String>,
    pub result: Option<FindResult>,
}

/// What the dialog says of a find's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The match shown, of how many.
    Shown {
        position: u32,
        count: u32,
    },
    /// There are matches, but no more that way (not around the ends).
    NoMore,
    NotFound,
}

impl Outcome {
    #[must_use]
    pub fn of(result: FindResult) -> Outcome {
        match result {
            FindResult { count: 0, .. } => Outcome::NotFound,
            FindResult { position: 0, .. } => Outcome::NoMore,
            FindResult { position, count } => Outcome::Shown { position, count },
        }
    }
}

static NEXT: AtomicU64 = AtomicU64::new(1);
static WAITING: Mutex<Option<HashMap<u64, Sender<Option<Find>>>>> = Mutex::new(None);

fn waiting<R>(with: impl FnOnce(&mut HashMap<u64, Sender<Option<Find>>>) -> R) -> R {
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    with(waiting.get_or_insert_with(HashMap::new))
}

/// The dialog's answer to the question `ticket` names: what to find, or
/// `None` when the person is done finding.
pub fn answer(ticket: u64, answer: Option<Find>) {
    if let Some(tx) = waiting(|w| w.remove(&ticket)) {
        let _ = tx.send(answer);
    }
}

/// What to find next, from the dialog `ask` brings up or has up; `None`:
/// nothing more (the dialog was closed, or there is no window to show it
/// in). Waits for the person, as long as they take: not for the GUI's
/// thread.
pub(crate) fn ask(
    ask: Option<&(dyn Fn(MenuRequest) + Send + Sync)>,
    initial: Option<String>,
    result: Option<FindResult>,
) -> Option<Find> {
    let ask = ask?;
    let (tx, rx) = mpsc::channel();
    let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
    waiting(|w| w.insert(ticket, tx));
    ask(MenuRequest::Find(Question { ticket, initial, result }));
    let answer = rx.recv().ok().flatten();
    waiting(|w| w.remove(&ticket));
    answer.filter(|find| !find.text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn where_the_dialog_was_left() {
        assert_eq!(place_from_setting("1006,285"), Some((1006, 285)));
        assert_eq!(place_from_setting(" -1200, 40 "), Some((-1200, 40)), "a screen to the left");
        assert_eq!(place_from_setting(""), None);
        assert_eq!(place_from_setting("12"), None);
        assert_eq!(place_from_setting("a,b"), None);
    }

    #[test]
    fn what_a_find_came_to() {
        assert_eq!(Outcome::of(FindResult { position: 3, count: 17 }), Outcome::Shown { position: 3, count: 17 });
        assert_eq!(Outcome::of(FindResult { position: 0, count: 17 }), Outcome::NoMore);
        assert_eq!(Outcome::of(FindResult { position: 0, count: 0 }), Outcome::NotFound);
    }

    #[test]
    fn the_dialog_answers_the_terminal() {
        let asked = std::sync::Arc::new(Mutex::new(Vec::new()));
        let seen = std::sync::Arc::clone(&asked);
        let ask = move |request: MenuRequest| {
            let MenuRequest::Find(question) = request else { panic!("a find") };
            seen.lock().unwrap().push(question.clone());
            // the dialog, on another thread: "Find Next"
            std::thread::spawn(move || {
                let find = Find { text: "eth0".into(), up: true, wrap: true, ..Find::default() };
                answer(question.ticket, Some(find));
            });
        };
        let found = ask_with(&ask, Some("eth0".into()), None);
        assert_eq!(found.map(|f| f.text), Some("eth0".to_string()));
        let asked = asked.lock().unwrap();
        assert_eq!(asked[0].initial.as_deref(), Some("eth0"));
        assert_eq!(asked[0].result, None);
        // closed: nothing more
        let closed = |request: MenuRequest| {
            let MenuRequest::Find(question) = request else { panic!("a find") };
            answer(question.ticket, None);
        };
        assert_eq!(ask_with(&closed, None, Some(FindResult { position: 1, count: 2 })), None);
        // no window to ask in
        assert_eq!(super::ask(None, None, None), None);
    }

    fn ask_with(
        ask: &(dyn Fn(MenuRequest) + Send + Sync),
        initial: Option<String>,
        result: Option<FindResult>,
    ) -> Option<Find> {
        super::ask(Some(ask), initial, result)
    }
}
