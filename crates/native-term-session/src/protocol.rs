//! Messages between a shim and NativeTerm: one JSON object per line.

use std::io::{self, Write};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// What a connecting process is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The tab's first process; stays connected.
    Shim,
    /// The `LocalCommand` helper: sends `Authenticated` and leaves.
    AuthSignal,
    /// A helper in a tab (e.g. rz / sz) asking for something once
    /// (`OpenFiles`), then leaving.
    Request,
}

/// Shim → NativeTerm.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShimMessage {
    /// First message on every connection.
    Hello {
        protocol: u32,
        role: Role,
        pid: u32,
        /// `WT_SESSION`: the Windows Terminal session GUID.
        wt_session: Option<String>,
        /// NativeTerm's own session id (`--session`), kept by "Restart
        /// connection"; absent in restored or duplicated panes.
        session: Option<String>,
        /// Host alias; absent when started without a host.
        alias: Option<String>,
        /// The Windows Terminal window hosting the tab (its HWND), if the
        /// shim could tell.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal_window: Option<i64>,
    },
    /// Started with `--wait`: waiting for `Connect` (or the user) before
    /// the first connection.
    Waiting,
    /// The client (ssh) was started; `attempt` counts from 1 per shim.
    Connecting { attempt: u32 },
    /// Logged in (`LocalCommand` fired).
    Authenticated,
    /// From the `LocalCommand` helper, before `Authenticated`: what the
    /// server said it is when the connection began (its identification
    /// string, `SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13`), where the
    /// client tells (NativeTerm's own ssh).
    Server { version: String },
    /// The client exited with this code.
    Exited { code: i32 },
    /// The tab is being closed (`CTRL_CLOSE_EVENT`).
    Closing,
    /// Nothing arrived since `since` (seconds since the Unix epoch): a
    /// serial line has no connection to lose, a dead device only goes
    /// quiet.
    Quiet { since: u64 },
    /// Output again after `Quiet`.
    Heard,
    /// Sent just before `Exited`: the client never reached the server
    /// (a direct ssh connection that was never established), so the end
    /// is "couldn't connect", not a failed login.
    Unreachable,
    /// Sent just before `Exited`: the account's saved password was given
    /// and the login failed; it is marked refused and no longer used.
    PasswordRefused,
    /// A logon action left something out (a hidden Send not stored on this
    /// computer, no password saved for `\w`), said in the shim's words.
    LogonNote { text: String },
    /// Logged in as another user than the host's (given in the password
    /// window, "Save password" ticked): the host's `User` is to be this.
    UserChanged { user: String },
    /// The client takes these commands (`AppMessage::Special`) for this
    /// connection, e.g. "brk" (a serial line's Break, Telnet's Break);
    /// sent after `Connecting`, none until then.
    Specials { names: Vec<String> },
    /// From a `Request` helper: open the files window (SFTP) of the tab's
    /// session (found by `wt_session`), at its tmux pane's folder.
    OpenFiles,
    /// The tab's console screen, as `AppMessage::Screen` asked: the
    /// lines as they are on it (trailing spaces gone, empty lines at the
    /// end left out). Terminal renders only the tab it shows, so this is
    /// how a tab nobody has looked at can still be shown.
    Screen { columns: u16, lines: Vec<String> },
    /// From a `Request` helper: files were dropped into the tab (Terminal
    /// pastes their names, which the client held back). NativeTerm asks
    /// what to do with them: upload them, or send the text after all.
    Dropped { paths: Vec<String>, text: String },
    /// From a `Request` helper: what NativeTerm's tab menu offers the
    /// tab's session (found by `wt_session`), answered by
    /// `AppMessage::TabMenu`. For terminals whose tab strip NativeTerm
    /// can't draw over (WezTerm): the terminal's own picker shows the
    /// items. With the tab's `place` among its window's tabs (newer), a
    /// tab without a session has a menu too: what is done with any tab.
    TabMenu {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        place: Option<TabPlace>,
    },
    /// From a `Request` helper: the item chosen from that menu.
    TabAction { id: u32 },
    /// From a `Request` helper (the shim serving ssh's askpass): ssh asks
    /// for `user@host`'s password and none is saved; answered by
    /// `AppMessage::Password` once the person has said. `retry`: the one
    /// given before was wrong; `refused`: the saved one was refused;
    /// `can_save`: there is a store to keep it in.
    AskPassword { user: String, host: String, retry: bool, refused: bool, can_save: bool },
    /// From a `Request` helper: the terminal gives a tab another title
    /// and asks which (`current`: the one it has), answered by
    /// `AppMessage::TabTitle` once the person has said.
    TabTitle { current: String },
    /// From a `Request` helper: what the tab's hover card says (the
    /// session's name and state), answered by `AppMessage::TabCard`. For
    /// terminals that draw the card themselves (WezTerm).
    TabCard,
    /// From a `Request` helper: the terminal is about to paste what the
    /// clipboard holds as a quotation and asks with which characters,
    /// answered by `AppMessage::Quotation` (once the person has said, where
    /// they are asked).
    PasteQuotation,
    /// From a `Request` helper: the terminal finds text in a pane for the
    /// person and asks what, answered by `AppMessage::Find` when they say
    /// "Find Next" in NativeTerm's dialog (or close it). `initial`: what
    /// the dialog's field may start with, the first time; `result`: what
    /// the last find came to, from then on.
    Find {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        initial: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<FindResult>,
    },
    /// The tab's session log: the file being written, or none (stopped,
    /// or never started). Sent when it changes, replayed to a NativeTerm
    /// that starts later.
    Logging {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
    },
    /// From a `Request` helper: a session log starts and its settings say
    /// to ask for the file; `suggested` is the one they name. Answered by
    /// `AppMessage::LogFile`.
    AskLogFile { suggested: String },
    /// From a `Request` helper (the shim serving ssh's askpass): the host
    /// key of `host` isn't known (`old` none), or differs from the one
    /// known (`old`: its type and fingerprint, where it is kept).
    /// Answered by `AppMessage::HostKey`.
    AskHostKey {
        host: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        ip: String,
        key_type: String,
        fingerprint: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old: Option<OldHostKey>,
    },
}

/// The host key known before, where a server now shows another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OldHostKey {
    pub key_type: String,
    pub fingerprint: String,
    /// The `known_hosts` file it is in, and its line.
    pub file: String,
    pub line: u64,
}

/// What the person said about a host key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostKeyAnswer {
    /// Trust it and keep it (a changed one: the old one taken out).
    Save,
    /// Trust it for this connection only (a new host).
    Once,
    /// Don't connect.
    Cancel,
}

/// What a find came to: the match shown, counted from the first (0: none,
/// there is none or no more that way), and how many there are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindResult {
    pub position: u32,
    pub count: u32,
}

/// Where a tab is among its window's tabs: the first is 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabPlace {
    pub index: u32,
    pub count: u32,
}

/// One line of the tab menu, as `AppMessage::TabMenu` lists it: an
/// item to choose (`id` as `ShimMessage::TabAction` sends it back), a
/// heading (`id` 0) naming what the menu is about, or a separator. The
/// fields after `text` are newer; missing, an item is enabled and plain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuItem {
    pub id: u32,
    pub text: String,
    /// Shown, dimmed, but not chosen when false.
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub separator: bool,
    /// Its icon, as a nerdfont name (what WezTerm draws icons with).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The terminal does it itself (a tab of its own closed, its screen
    /// cleared): chosen, it is not sent back as a `TabAction`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub here: bool,
}

fn enabled() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl MenuItem {
    /// An enabled item (or, with `id` 0, the heading) without an icon.
    pub fn new(id: u32, text: impl Into<String>) -> MenuItem {
        MenuItem { id, text: text.into(), enabled: true, separator: false, icon: None, here: false }
    }

    pub fn separator() -> MenuItem {
        MenuItem { separator: true, ..MenuItem::new(0, "") }
    }
}

/// What the person said when asked for a password.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PasswordAnswer {
    /// This one; kept after the login where `save`. `user`: another
    /// user name than the one asked for (the connection is made again as
    /// that user).
    Given {
        secret: String,
        save: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
    },
    /// Asked in the tab after all.
    Skip,
    /// The connection is given up.
    Cancel,
}

impl std::fmt::Debug for PasswordAnswer {
    // (a password is never written out: not in a log, not in a panic)
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasswordAnswer::Given { save, user, .. } => {
                write!(f, "Given {{ secret: <hidden>, save: {save}, user: {user:?} }}")
            }
            PasswordAnswer::Skip => f.write_str("Skip"),
            PasswordAnswer::Cancel => f.write_str("Cancel"),
        }
    }
}

/// NativeTerm → shim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppMessage {
    Welcome {
        protocol: u32,
        /// NativeTerm's data folder: where session logs go when no other
        /// file is named.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data_dir: Option<String>,
    },
    /// Start (or restart) the client.
    Connect,
    /// End the client, keep the tab.
    Disconnect,
    /// End everything and exit with 0, which closes the tab.
    Close,
    /// Type text into the tab's console, then Enter if `enter`.
    SendText { text: String, enter: bool },
    /// Clear the tab's scrollback, and its screen: after login by typing
    /// Ctrl+L for the remote side, otherwise directly.
    ClearScreen,
    /// For a shim started without a host: be a local shell.
    LocalShell,
    /// For a shim started without a host: stay, NativeTerm is replacing
    /// this tab and will close it.
    Hold,
    /// Send one of the client's special commands (see
    /// `ShimMessage::Specials`) over the connection.
    Special { name: String },
    /// Asks for the tab's console screen (`ShimMessage::Screen`).
    Screen,
    /// The tab menu `ShimMessage::TabMenu` asked for: what applies now,
    /// in order, headings included; empty when the tab has no session.
    TabMenu { items: Vec<MenuItem> },
    /// The hover card `ShimMessage::TabCard` asked for: the session's
    /// name, and under it its state; `show` false when cards are off in
    /// the settings. Empty `title` when the tab has no session.
    TabCard { title: String, note: String, show: bool },
    /// The answer to `ShimMessage::TabTitle`: the tab's new title (empty:
    /// the terminal's own again); `rename` false: the person said no.
    TabTitle { title: String, rename: bool },
    /// The answer to `ShimMessage::AskPassword`.
    Password { answer: PasswordAnswer },
    /// The answer to `ShimMessage::PasteQuotation`: every line goes
    /// between `chars` (`between`) or after them; `paste` false: the
    /// person said no, nothing is pasted.
    Quotation { chars: String, between: bool, paste: bool },
    /// The answer to `ShimMessage::AskHostKey`.
    HostKey { answer: HostKeyAnswer },
    /// Start (`on`) or stop the tab's session log, from the session's menu.
    Log { on: bool },
    /// The answer to `ShimMessage::AskLogFile`: the file, or none (the
    /// person cancelled: no log).
    LogFile {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// The answer to `ShimMessage::Find`: what to find and how; `find`
    /// false: the person is done, the dialog is closed.
    Find { find: bool, text: String, match_case: bool, whole_word: bool, wrap: bool, up: bool },
}

pub fn encode<T: Serialize>(message: &T) -> String {
    let mut line = serde_json::to_string(message).expect("messages always serialize");
    line.push('\n');
    line
}

pub fn decode<T: DeserializeOwned>(line: &str) -> io::Result<T> {
    serde_json::from_str(line.trim_end()).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn write_message<T: Serialize>(out: &mut impl Write, message: &T) -> io::Result<()> {
    out.write_all(encode(message).as_bytes())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let messages = vec![
            ShimMessage::Hello {
                protocol: 1,
                role: Role::Shim,
                pid: 42,
                wt_session: Some("6e7a0000-0000-4000-8000-0000000000b2".into()),
                session: Some("s-1".into()),
                alias: Some("ceph-cluster.osd1".into()),
                terminal_window: Some(0x5a0b2c),
            },
            ShimMessage::Waiting,
            ShimMessage::Connecting { attempt: 2 },
            ShimMessage::Authenticated,
            ShimMessage::Server { version: "SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.19".into() },
            ShimMessage::Exited { code: -1 },
            ShimMessage::Closing,
            ShimMessage::Specials { names: vec!["brk".into(), "ayt".into()] },
            ShimMessage::Unreachable,
            ShimMessage::PasswordRefused,
            ShimMessage::UserChanged { user: "admin".into() },
            ShimMessage::TabMenu { place: None },
            ShimMessage::TabMenu { place: Some(TabPlace { index: 1, count: 3 }) },
            ShimMessage::TabTitle { current: "bash".into() },
            ShimMessage::AskPassword {
                user: "root".into(),
                host: "192.0.2.1".into(),
                retry: true,
                refused: false,
                can_save: true,
            },
            ShimMessage::TabAction { id: 4 },
            ShimMessage::PasteQuotation,
            ShimMessage::Find { initial: Some("eth0".into()), result: None },
            ShimMessage::Find { initial: None, result: Some(FindResult { position: 3, count: 17 }) },
            ShimMessage::TabCard,
        ];
        for m in messages {
            let line = encode(&m);
            assert!(line.ends_with('\n') && !line[..line.len() - 1].contains('\n'), "{line}");
            assert_eq!(decode::<ShimMessage>(&line).unwrap(), m);
        }
        // older shims don't send the window
        let old =
            r#"{"type":"hello","protocol":1,"role":"shim","pid":1,"wt_session":null,"session":null,"alias":null}"#;
        assert!(matches!(decode::<ShimMessage>(old).unwrap(), ShimMessage::Hello { terminal_window: None, .. }));
        let text = AppMessage::SendText { text: "echo 你好\n😀".into(), enter: true };
        assert_eq!(decode::<AppMessage>(&encode(&text)).unwrap(), text);
        let menu = AppMessage::TabMenu {
            items: vec![
                MenuItem::new(0, "web01"),
                MenuItem { icon: Some("cod_refresh".into()), ..MenuItem::new(1, "Reconnect") },
                MenuItem::separator(),
                MenuItem { enabled: false, ..MenuItem::new(2, "Disconnect") },
            ],
        };
        assert_eq!(decode::<AppMessage>(&encode(&menu)).unwrap(), menu);
        let card = AppMessage::TabCard { title: "web01".into(), note: "Connected · 5 min".into(), show: true };
        assert_eq!(decode::<AppMessage>(&encode(&card)).unwrap(), card);
        let title = AppMessage::TabTitle { title: "构建".into(), rename: true };
        for answer in [
            PasswordAnswer::Given { secret: "p\"a ss".into(), save: true, user: None },
            PasswordAnswer::Given { secret: "x".into(), save: false, user: Some("admin".into()) },
            PasswordAnswer::Skip,
            PasswordAnswer::Cancel,
        ] {
            let message = AppMessage::Password { answer };
            assert_eq!(decode::<AppMessage>(&encode(&message)).unwrap(), message);
        }
        let shown = format!("{:?}", PasswordAnswer::Given { secret: "hunter2".into(), save: false, user: None });
        assert!(!shown.contains("hunter2"), "{shown}");
        assert_eq!(decode::<AppMessage>(&encode(&title)).unwrap(), title);
        // an older shim asks without the tab's place
        let old = r#"{"type":"tab_menu"}"#;
        assert_eq!(decode::<ShimMessage>(old).unwrap(), ShimMessage::TabMenu { place: None });
        assert_eq!(encode(&ShimMessage::TabMenu { place: None }), "{\"type\":\"tab_menu\"}\n");
        // what the terminal does itself is said only where it is so
        let here = MenuItem { here: true, ..MenuItem::new(4, "Close") };
        assert!(encode(&here).contains("\"here\":true"));
        assert!(!encode(&MenuItem::new(4, "Close")).contains("here"));
        // an older NativeTerm sends id and text only
        let old = r#"{"type":"tab_menu","items":[{"id":1,"text":"Reconnect"}]}"#;
        assert_eq!(
            decode::<AppMessage>(old).unwrap(),
            AppMessage::TabMenu { items: vec![MenuItem::new(1, "Reconnect")] }
        );
    }

    #[test]
    fn wire_format_is_readable() {
        assert_eq!(encode(&AppMessage::Close), "{\"type\":\"close\"}\n");
        assert_eq!(encode(&ShimMessage::Exited { code: 255 }), "{\"type\":\"exited\",\"code\":255}\n");
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode::<AppMessage>("{\"type\":\"format_disk\"}").is_err());
        assert!(decode::<AppMessage>("not json").is_err());
    }
}
