//! Logon actions, as SecureCRT has them (Session Options → Connection →
//! Logon Actions → Automate logon): a table of what to wait for in the
//! session's output (Expect) and what to type then (Send), worked through
//! in order once per connection. The shim does it (`native-term-shim`,
//! `logon.rs`): it sees the output through the session log's pipe and
//! types as the person would.
//!
//! The settings are `NativeTermLogon*` keys, in a host's block or in the
//! folder's `Host __nativeterm_folder__` block as its sessions' default;
//! a non-SSH session keeps them in its `logon` table. As with the session
//! log, a session with any logon key of its own uses its own set, whole.
//!
//! - `NativeTermLogon yes|no`: "Automate logon".
//! - `NativeTermLogonInitialCR yes|no`: "Send initial carriage return".
//! - `NativeTermLogonExpect<n>`, `NativeTermLogonSend<n>`,
//!   `NativeTermLogonFlags<n>` for the n-th row, from 1: its flags are
//!   `hide` ("Hide"), `nocr` ("Send trailing carriage return" off) and
//!   `cred=<set>` (the credential set `\s` and `\w` take from).
//!
//! A hidden Send is not written into the configuration: it is kept in the
//! system's password store (`NativeTerm/logon/<id>`, the same store as
//! saved passwords) and the configuration says `secret:<id>`. SecureCRT
//! keeps such a Send encrypted; an enable password in plain text in the
//! ssh config would be a step back.

use std::collections::BTreeMap;

use crate::tree::{Folder, HostEntry};

/// A row of the table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// The text to wait for; empty: send at once (after the row before).
    pub expect: String,
    /// What to type, with SecureCRT's escapes (see [`pieces`]); for a
    /// hidden row as kept, `secret:<id>` (see [`secret_id`]).
    pub send: String,
    /// "Hide": the text is not shown and kept in the password store.
    pub hide: bool,
    /// "Send trailing carriage return".
    pub enter: bool,
    /// The credential set `\s` and `\w` take the user and password from;
    /// without one, the session's own.
    pub credential: Option<String>,
}

/// A row's keys as found, before they are read.
#[derive(Default)]
struct RawRow {
    expect: Option<String>,
    send: Option<String>,
    flags: Option<String>,
}

/// The whole page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogonActions {
    /// "Automate logon": the table is worked through.
    pub automate: bool,
    /// "Send initial carriage return": Enter as soon as the session is up.
    pub initial_cr: bool,
    pub steps: Vec<Step>,
}

/// The most rows read (a guard against a key numbered by mistake).
pub const MAX_STEPS: usize = 256;

const AUTOMATE: (&str, &str) = ("logon", "NativeTermLogon");
const INITIAL_CR: (&str, &str) = ("logoninitialcr", "NativeTermLogonInitialCR");
const EXPECT: (&str, &str) = ("logonexpect", "NativeTermLogonExpect");
const SEND: (&str, &str) = ("logonsend", "NativeTermLogonSend");
const FLAGS: (&str, &str) = ("logonflags", "NativeTermLogonFlags");

/// Where a hidden Send is kept in the password store: under this, then its
/// id (tests set `NATIVETERM_CRED_PREFIX` to keep theirs apart).
pub fn secret_prefix() -> String {
    format!("{}/logon/", crate::password::prefix())
}

/// A hidden Send's password store entry.
pub fn secret_entry(id: &str) -> String {
    format!("{}{id}", secret_prefix())
}

/// The id of a hidden Send kept in the store (`secret:<id>`), if `send`
/// is one.
pub fn secret_id(send: &str) -> Option<&str> {
    send.strip_prefix("secret:")
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// A new id for a hidden Send.
pub fn new_secret_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Whether a lowercase `NativeTerm*` key (without the prefix) is a logon
/// key.
pub fn is_logon_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key == AUTOMATE.0 || key == INITIAL_CR.0 || numbered(&key).is_some()
}

/// A numbered key's kind and row (`logonsend3` → (send, 3)).
fn numbered(key: &str) -> Option<(&'static str, usize)> {
    for (kind, _) in [EXPECT, SEND, FLAGS] {
        if let Some(n) = key.strip_prefix(kind).and_then(|n| n.parse::<usize>().ok()) {
            return (1..=MAX_STEPS).contains(&n).then_some((kind, n));
        }
    }
    None
}

fn yes(value: Option<&str>) -> bool {
    value.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "yes" | "true" | "1" | "on"))
}

impl LogonActions {
    /// From a block's keys (lowercase, without the prefix); `None` when
    /// there is no logon key at all. Rows are read in their numbers'
    /// order, gaps closed; a row needs a Send or an Expect.
    pub fn from_keys<'a>(keys: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<LogonActions> {
        let mut any = false;
        let mut actions = LogonActions::default();
        let mut rows: BTreeMap<usize, RawRow> = BTreeMap::new();
        for (key, value) in keys {
            let key = key.to_ascii_lowercase();
            if key == AUTOMATE.0 {
                actions.automate = yes(Some(value));
            } else if key == INITIAL_CR.0 {
                actions.initial_cr = yes(Some(value));
            } else if let Some((kind, n)) = numbered(&key) {
                let row = rows.entry(n).or_default();
                let slot = match kind {
                    k if k == EXPECT.0 => &mut row.expect,
                    k if k == SEND.0 => &mut row.send,
                    _ => &mut row.flags,
                };
                *slot = Some(value.to_string());
            } else {
                continue;
            }
            any = true;
        }
        if !any {
            return None;
        }
        for RawRow { expect, send, flags } in rows.into_values() {
            if expect.is_none() && send.is_none() {
                continue;
            }
            let mut step = Step {
                expect: expect.unwrap_or_default(),
                send: send.unwrap_or_default(),
                enter: true,
                ..Step::default()
            };
            for flag in flags.as_deref().unwrap_or_default().split(',').map(str::trim) {
                match flag.to_ascii_lowercase().as_str() {
                    "hide" => step.hide = true,
                    "nocr" => step.enter = false,
                    _ => {
                        if let Some(set) = flag.strip_prefix("cred=").filter(|s| crate::password::valid_set_name(s)) {
                            step.credential = Some(set.to_string());
                        }
                    }
                }
            }
            actions.steps.push(step);
        }
        Some(actions)
    }

    /// The keys to write, as (written name, value): all of them, so the
    /// set stays whole.
    pub fn directives(&self) -> Vec<(String, String)> {
        let flag = |on: bool| if on { "yes" } else { "no" }.to_string();
        let mut out =
            vec![(AUTOMATE.1.to_string(), flag(self.automate)), (INITIAL_CR.1.to_string(), flag(self.initial_cr))];
        for (i, step) in self.steps.iter().take(MAX_STEPS).enumerate() {
            let n = i + 1;
            out.push((format!("{}{n}", EXPECT.1), step.expect.clone()));
            out.push((format!("{}{n}", SEND.1), step.send.clone()));
            let mut flags = Vec::new();
            if step.hide {
                flags.push("hide".to_string());
            }
            if !step.enter {
                flags.push("nocr".to_string());
            }
            if let Some(set) = &step.credential {
                flags.push(format!("cred={set}"));
            }
            if !flags.is_empty() {
                out.push((format!("{}{n}", FLAGS.1), flags.join(",")));
            }
        }
        out
    }

    /// The same as lowercase keys (a non-SSH session's `logon` table).
    pub fn to_keys(&self) -> BTreeMap<String, String> {
        self.directives()
            .into_iter()
            .map(|(name, value)| (name["NativeTerm".len()..].to_ascii_lowercase(), value))
            .collect()
    }

    /// The session's own set, if it has one.
    pub fn own(host: &HostEntry) -> Option<LogonActions> {
        // (a non-SSH session's `logon` table is among its keys too)
        LogonActions::from_keys(host.nt.0.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    /// The folder's default set, if it has one.
    pub fn of_folder(folder: &Folder) -> Option<LogonActions> {
        LogonActions::from_keys(folder.defaults.0.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    /// What a session uses: its own set, else its folder's, else none.
    pub fn for_host(folder: &Folder, host: &HostEntry) -> LogonActions {
        LogonActions::own(host).or_else(|| LogonActions::of_folder(folder)).unwrap_or_default()
    }

    /// Whether anything is to be done.
    pub fn active(&self) -> bool {
        self.automate && (self.initial_cr || !self.steps.is_empty())
    }
}

/// A part of what a Send types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    /// `\p`: a second's pause.
    Pause,
    /// `\v`: what the clipboard holds.
    Clipboard,
    /// `\s`: the user name of the row's credentials.
    User,
    /// `\w`: the password of the row's credentials.
    Password,
}

/// A Send read with SecureCRT's escapes: `\r` carriage return, `\n` line
/// feed, `\b` backspace, `\e` escape, `\t` tab, `\\` a backslash, `\v`
/// the clipboard, `\p` a second's pause, `\s` the user name and `\w` the
/// password of the credentials. Any other backslash is typed as it is.
/// The trailing carriage return, if the row has one, is not part of it.
pub fn pieces(send: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut chars = send.chars().peekable();
    let special = |out: &mut Vec<Piece>, text: &mut String, piece: Piece| {
        if !text.is_empty() {
            out.push(Piece::Text(std::mem::take(text)));
        }
        out.push(piece);
    };
    while let Some(c) = chars.next() {
        if c != '\\' {
            text.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('r') => text.push('\r'),
            Some('n') => text.push('\n'),
            Some('b') => text.push('\u{8}'),
            Some('e') => text.push('\u{1b}'),
            Some('t') => text.push('\t'),
            Some('\\') => text.push('\\'),
            Some('v') => special(&mut out, &mut text, Piece::Clipboard),
            Some('p') => special(&mut out, &mut text, Piece::Pause),
            Some('s') => special(&mut out, &mut text, Piece::User),
            Some('w') => special(&mut out, &mut text, Piece::Password),
            _ => {
                text.push('\\');
                continue;
            }
        }
        chars.next();
    }
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<(String, String)> {
        text.lines()
            .filter_map(|l| l.split_once(' '))
            .map(|(k, v)| (k.trim_start_matches("NativeTerm").to_ascii_lowercase(), v.to_string()))
            .collect()
    }

    fn read(text: &str) -> Option<LogonActions> {
        let keys = keys(text);
        LogonActions::from_keys(keys.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    #[test]
    fn a_table_read_and_written() {
        let actions = read(
            "NativeTermLogon yes\nNativeTermLogonInitialCR yes\nNativeTermLogonExpect1 login:\nNativeTermLogonSend1 \\s\nNativeTermLogonFlags1 cred=lab\nNativeTermLogonExpect2 Password:\nNativeTermLogonSend2 secret:0f1e-2d\nNativeTermLogonFlags2 hide,cred=lab\nNativeTermLogonExpect3 $\nNativeTermLogonSend3 sudo -i\nNativeTermLogonExpect4 #\nNativeTermLogonSend4 \\e:q\nNativeTermLogonFlags4 nocr",
        )
        .unwrap();
        assert!(actions.automate && actions.initial_cr && actions.active());
        assert_eq!(actions.steps.len(), 4);
        assert_eq!(
            actions.steps[0],
            Step {
                expect: "login:".into(),
                send: "\\s".into(),
                hide: false,
                enter: true,
                credential: Some("lab".into())
            }
        );
        assert!(actions.steps[1].hide);
        assert_eq!(secret_id(&actions.steps[1].send), Some("0f1e-2d"));
        assert!(!actions.steps[3].enter);
        // written as read
        let written: Vec<(String, String)> = actions
            .directives()
            .into_iter()
            .map(|(k, v)| (k.trim_start_matches("NativeTerm").to_ascii_lowercase(), v))
            .collect();
        let again = LogonActions::from_keys(written.iter().map(|(k, v)| (k.as_str(), v.as_str()))).unwrap();
        assert_eq!(again, actions);
        assert_eq!(actions.to_keys().get("logonsend3").map(String::as_str), Some("sudo -i"));
        assert!(!actions.to_keys().contains_key("logonflags3"), "the defaults are not written");
    }

    #[test]
    fn rows_in_their_numbers_order() {
        let actions = read("NativeTermLogonSend10 ten\nNativeTermLogonExpect2 two:\nNativeTermLogonSend2 2\nNativeTermLogonFlags7 hide\nNativeTermLogonSend999 far").unwrap();
        let sends: Vec<&str> = actions.steps.iter().map(|s| s.send.as_str()).collect();
        assert_eq!(sends, ["2", "ten"], "a row of flags alone is none; 999 is past the last");
        assert!(!actions.automate, "not asked for");
        assert!(!actions.active());
    }

    #[test]
    fn no_keys_no_set() {
        assert_eq!(read("NativeTermLogFile x.log\nNativeTermLabel web"), None);
        assert!(is_logon_key("LogonExpect3") && is_logon_key("logon") && is_logon_key("logoninitialcr"));
        assert!(!is_logon_key("logfile") && !is_logon_key("logonexpect") && !is_logon_key("logonexpect0"));
        let off = read("NativeTermLogon no").unwrap();
        assert_eq!(off, LogonActions::default());
    }

    #[test]
    fn a_credential_that_cannot_be_one_is_none() {
        let actions = read("NativeTermLogon yes\nNativeTermLogonSend1 x\nNativeTermLogonFlags1 cred=a/b").unwrap();
        assert_eq!(actions.steps[0].credential, None);
    }

    #[test]
    fn securecrts_escapes() {
        use Piece::*;
        assert_eq!(pieces("root"), [Text("root".into())]);
        assert_eq!(pieces("a\\rb\\n\\t\\\\c"), [Text("a\rb\n\t\\c".into())]);
        assert_eq!(pieces("\\b\\e"), [Text("\u{8}\u{1b}".into())]);
        assert_eq!(pieces("\\s\\r\\p\\w"), [User, Text("\r".into()), Pause, Password]);
        assert_eq!(pieces("en\\pable\\v"), [Text("en".into()), Pause, Text("able".into()), Clipboard]);
        assert_eq!(pieces("C:\\x\\"), [Text("C:\\x\\".into())], "others as they are");
        assert_eq!(pieces(""), []);
    }

    #[test]
    fn hidden_sends_are_kept_apart() {
        assert_eq!(secret_id("secret:abc-123"), Some("abc-123"));
        assert_eq!(secret_id("secret:"), None);
        assert_eq!(secret_id("secret:a b"), None);
        assert_eq!(secret_id("hunter2"), None);
        assert!(secret_entry("x").ends_with("/logon/x"));
        let id = new_secret_id();
        assert_eq!(secret_id(&format!("secret:{id}")), Some(id.as_str()));
    }
}
