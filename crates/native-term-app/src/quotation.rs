//! "Paste as Quotation", as SecureCRT's: what is in the clipboard is
//! pasted with every line between quotation characters (`"<text>"`), or
//! after them (`> <text>`). The terminal does the pasting; NativeTerm
//! keeps the characters and asks for them in a small window of its own,
//! until the person says not to ask again.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::time::Duration;

use crate::tab_menu::MenuRequest;
use crate::Core;

/// The quotation characters.
pub const CHARS_SETTING: &str = "paste.quotation";
/// `off`: the text goes after them, not between them.
pub const BETWEEN_SETTING: &str = "paste.quotation_between";
/// `off`: not asked, what was chosen last is used.
pub const PROMPT_SETTING: &str = "paste.quotation_prompt";

/// SecureCRT's own ("Quotation Characters").
const DEFAULT_CHARS: &str = "\"";
/// A window nobody answers is given up.
const PATIENCE: Duration = Duration::from_secs(600);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quotation {
    pub chars: String,
    /// The text between the characters, or after them.
    pub between: bool,
}

impl Default for Quotation {
    fn default() -> Quotation {
        Quotation { chars: DEFAULT_CHARS.to_string(), between: true }
    }
}

impl Quotation {
    /// What was chosen last.
    #[must_use]
    pub fn saved(core: &Core) -> Quotation {
        Quotation {
            chars: core.setting(CHARS_SETTING).unwrap_or_else(|| DEFAULT_CHARS.to_string()),
            between: core.setting(BETWEEN_SETTING).as_deref() != Some("off"),
        }
    }

    pub fn save(&self, core: &Core) {
        core.set_setting(CHARS_SETTING, &self.chars);
        core.set_setting(BETWEEN_SETTING, if self.between { "on" } else { "off" });
    }

    /// What a pasted line looks like, `text` standing for it.
    #[must_use]
    pub fn sample(&self, text: &str) -> String {
        if self.between {
            format!("{0}{text}{0}", self.chars)
        } else {
            format!("{}{text}", self.chars)
        }
    }

    /// The characters as they can be used: on one line (a line's end in
    /// them would make two lines of every line).
    #[must_use]
    pub fn cleaned(mut self) -> Quotation {
        self.chars.retain(|c| c != '\n' && c != '\r');
        self
    }
}

/// Whether the person is asked.
#[must_use]
pub fn prompts(core: &Core) -> bool {
    core.setting(PROMPT_SETTING).as_deref() != Some("off")
}

pub fn set_prompts(core: &Core, on: bool) {
    core.set_setting(PROMPT_SETTING, if on { "on" } else { "off" });
}

static NEXT: AtomicU64 = AtomicU64::new(1);
static WAITING: Mutex<Option<HashMap<u64, Sender<Option<Quotation>>>>> = Mutex::new(None);

fn waiting<R>(with: impl FnOnce(&mut HashMap<u64, Sender<Option<Quotation>>>) -> R) -> R {
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    with(waiting.get_or_insert_with(HashMap::new))
}

/// The window's answer to the question `ticket` names: the characters to
/// paste with, or `None` for no paste.
pub fn answer(ticket: u64, answer: Option<Quotation>) {
    if let Some(tx) = waiting(|w| w.remove(&ticket)) {
        let _ = tx.send(answer);
    }
}

/// The characters for a paste asked for now: what the person says in the
/// window `ask` brings up (`None`: no paste), or what was chosen last
/// when they are not to be asked or there is no window to ask in. Waits
/// for the answer: not for the GUI's thread.
pub(crate) fn ask(core: &Core, ask: Option<&(dyn Fn(MenuRequest) + Send + Sync)>) -> Option<Quotation> {
    let saved = Quotation::saved(core).cleaned();
    let Some(ask) = ask.filter(|_| prompts(core)) else { return Some(saved) };
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
    ask(MenuRequest::PasteQuotation(ticket));
    let answer = rx.recv_timeout(PATIENCE).ok().flatten();
    waiting(|w| w.remove(&ticket));
    answer.map(Quotation::cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_between_the_characters_or_after_them() {
        let quotation = Quotation::default();
        assert_eq!(quotation.sample("<text>"), "\"<text>\"");
        let after = Quotation { chars: "> ".into(), between: false };
        assert_eq!(after.sample("<text>"), "> <text>");
        assert_eq!(Quotation { chars: "'\r\n".into(), between: true }.cleaned().chars, "'");
    }

    #[test]
    fn an_answer_reaches_who_waits_for_it() {
        let (tx, rx) = mpsc::channel();
        waiting(|w| w.insert(u64::MAX, tx));
        answer(u64::MAX - 1, None);
        assert!(rx.try_recv().is_err(), "another question's answer");
        answer(u64::MAX, Some(Quotation::default()));
        assert_eq!(rx.try_recv().unwrap(), Some(Quotation::default()));
        answer(u64::MAX, None);
        assert!(rx.try_recv().is_err(), "answered once");
    }
}
