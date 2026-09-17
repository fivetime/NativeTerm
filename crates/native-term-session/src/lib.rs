//! Session records and the NativeTerm <-> shim protocol. Each tab runs
//! `nativeterm-shim`, which reports ssh's exit code over a named pipe.
//! Sessions are identified by the Windows Terminal session GUID
//! (`wt new-tab --sessionId`, seen by the shim as `WT_SESSION`).
//! See `docs/ARCHITECTURE.md`.

/// Full name: `\\.\pipe\nativeterm-<user SID>-<logon session id>`.
pub const PIPE_NAME_PREFIX: &str = r"\\.\pipe\nativeterm-";
pub const PROTOCOL_VERSION: u32 = 1;

/// Windows Terminal session GUID, as text.
pub type SessionId = String;

pub enum SessionEnd {
    /// Connection-level failure (dropped/refused/auth/keepalive timeout):
    /// ssh exits 255, or -1 on Windows OpenSSH when the connection closes
    /// without a remote exit status.
    Disconnected,
    /// Any other exit code — the remote session ended normally.
    ClosedNormally(i32),
}

pub enum SessionState {
    Connected,
    Ended(SessionEnd),
}

pub struct Session {
    pub id: SessionId,
    /// Also the tab title, which identifies the tab in the terminal.
    pub label: String,
    pub host_alias: String,
    pub state: SessionState,
    pub locked: bool,
}

/// Messages from a shim to NativeTerm.
pub enum ShimEvent {
    Registered { protocol_version: u32, session_id: SessionId, host_alias: String },
    Authenticated { session_id: SessionId },
    SshExited { session_id: SessionId, code: i32 },
    TabClosing { session_id: SessionId },
}

/// Messages from NativeTerm to a shim.
pub enum ShimCommand {
    Connect,
    Reconnect,
    Disconnect,
    /// End ssh and exit with code 0 so Windows Terminal closes the tab.
    Close,
    SendText(String),
}

pub fn classify_exit(code: i32) -> SessionEnd {
    if code == 255 || code == -1 {
        SessionEnd::Disconnected
    } else {
        SessionEnd::ClosedNormally(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_level_exit_codes() {
        assert!(matches!(classify_exit(255), SessionEnd::Disconnected));
        assert!(matches!(classify_exit(-1), SessionEnd::Disconnected));
        assert!(matches!(classify_exit(0), SessionEnd::ClosedNormally(0)));
        assert!(matches!(classify_exit(130), SessionEnd::ClosedNormally(130)));
    }
}
