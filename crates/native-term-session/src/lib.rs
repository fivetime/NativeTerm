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

/// The `ssh` NativeTerm runs and checks configs with: `NATIVETERM_SSH`, else
/// the first `ssh.exe` on `PATH` that is a native Windows build, else the
/// one that comes with Windows. MSYS/Cygwin builds (Git for Windows puts
/// one on some `PATH`s) are skipped: they read `C:/...` paths in `Include`
/// differently, so NativeTerm's folders would be invisible to them.
pub fn ssh_program() -> std::path::PathBuf {
    use std::path::PathBuf;
    if let Some(p) = std::env::var_os("NATIVETERM_SSH") {
        return PathBuf::from(p);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    ssh_in(std::env::split_paths(&path)).unwrap_or_else(|| {
        let windir = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let system = windir.join(r"System32\OpenSSH\ssh.exe");
        if system.is_file() {
            system
        } else {
            PathBuf::from("ssh")
        }
    })
}

fn ssh_in(dirs: impl Iterator<Item = std::path::PathBuf>) -> Option<std::path::PathBuf> {
    dirs.map(|dir| (dir.join("ssh.exe"), dir))
        .find(|(exe, dir)| exe.is_file() && !dir.join("msys-2.0.dll").exists() && !dir.join("cygwin1.dll").exists())
        .map(|(exe, _)| exe)
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

#[cfg(test)]
mod ssh_tests {
    use super::*;

    #[test]
    fn msys_builds_are_skipped() {
        let root = tempfile::tempdir().unwrap();
        let git = root.path().join("git-usr-bin");
        let native = root.path().join("openssh");
        for d in [&git, &native] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("ssh.exe"), b"").unwrap();
        }
        std::fs::write(git.join("msys-2.0.dll"), b"").unwrap();
        let empty = root.path().join("empty");
        let found = ssh_in([empty, git.clone(), native.clone()].into_iter());
        assert_eq!(found, Some(native.join("ssh.exe")));
        assert_eq!(ssh_in([git].into_iter()), None);
    }
}
