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
    /// items.
    TabMenu,
    /// From a `Request` helper: the item chosen from that menu.
    TabAction { id: u32 },
}

/// One line of the tab menu, as `AppMessage::TabMenu` lists it: an
/// item to choose (`id` as `ShimMessage::TabAction` sends it back), or a
/// heading (`id` 0) naming what the menu is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuItem {
    pub id: u32,
    pub text: String,
}

/// NativeTerm → shim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppMessage {
    Welcome {
        protocol: u32,
    },
    /// Start (or restart) the client.
    Connect,
    /// End the client, keep the tab.
    Disconnect,
    /// End everything and exit with 0, which closes the tab.
    Close,
    /// Type text into the tab's console, then Enter if `enter`.
    SendText {
        text: String,
        enter: bool,
    },
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
    Special {
        name: String,
    },
    /// Asks for the tab's console screen (`ShimMessage::Screen`).
    Screen,
    /// The tab menu `ShimMessage::TabMenu` asked for: what applies now,
    /// in order, headings included; empty when the tab has no session.
    TabMenu {
        items: Vec<MenuItem>,
    },
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
            ShimMessage::Exited { code: -1 },
            ShimMessage::Closing,
            ShimMessage::Specials { names: vec!["brk".into(), "ayt".into()] },
            ShimMessage::Unreachable,
            ShimMessage::PasswordRefused,
            ShimMessage::TabMenu,
            ShimMessage::TabAction { id: 4 },
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
            items: vec![MenuItem { id: 0, text: "web01".into() }, MenuItem { id: 1, text: "Reconnect".into() }],
        };
        assert_eq!(decode::<AppMessage>(&encode(&menu)).unwrap(), menu);
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
