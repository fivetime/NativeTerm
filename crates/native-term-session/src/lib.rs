//! The NativeTerm ↔ shim protocol. Each tab runs `nativeterm-shim`, which
//! talks to NativeTerm over a per-user named pipe: it announces itself,
//! reports the client's lifecycle, and executes commands (reconnect,
//! close, type text). See `docs/ARCHITECTURE.md`, "Session lifecycle".
//!
//! - [`protocol`]: messages, one JSON object per line.
//! - [`pipe`]: the named pipe (Windows).

#[cfg(windows)]
pub mod pipe;
pub mod protocol;

/// Full name: `\\.\pipe\nativeterm-<user SID>-<logon session id>`.
pub const PIPE_NAME_PREFIX: &str = r"\\.\pipe\nativeterm-";
/// Sent in `Hello`/`Welcome`; a newer NativeTerm keeps talking to older
/// shims where it can.
pub const PROTOCOL_VERSION: u32 = 1;

/// How a session ended, from the client's exit code and whether the
/// login signal arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEnd {
    /// Connection-level failure before login (refused, auth failed,
    /// cancelled at a prompt). Never retried automatically.
    LoginFailed,
    /// Connection-level failure after login (dropped, keepalive timeout).
    Disconnected,
    /// The remote session ended with this status.
    Closed(i32),
}

/// ssh exits 255 on connection-level failures, and Windows OpenSSH exits
/// -1 when the connection closes without a remote exit status.
pub fn is_connection_level(code: i32) -> bool {
    code == 255 || code == -1
}

pub fn classify_exit(code: i32, authenticated: bool) -> SessionEnd {
    match (is_connection_level(code), authenticated) {
        (true, false) => SessionEnd::LoginFailed,
        (true, true) => SessionEnd::Disconnected,
        (false, _) => SessionEnd::Closed(code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_classification() {
        assert_eq!(classify_exit(255, false), SessionEnd::LoginFailed);
        assert_eq!(classify_exit(-1, true), SessionEnd::Disconnected);
        assert_eq!(classify_exit(255, true), SessionEnd::Disconnected);
        assert_eq!(classify_exit(0, true), SessionEnd::Closed(0));
        assert_eq!(classify_exit(130, false), SessionEnd::Closed(130));
    }
}
