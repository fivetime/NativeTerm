//! Sending a command to a persistent session through tmux on the server
//! (`NativeTermPersistent tmux`): the shell runs in tmux there whether or
//! not its tab is connected, so a command can reach it over a connection of
//! its own, `ssh <alias> tmux send-keys`, when the tab can't take it (tab
//! disconnected, reconnecting, or closed). One extra ssh handshake per
//! target (Windows OpenSSH can't share connections); key or agent login
//! only (`BatchMode`): there is no console to ask a password in.

use std::path::Path;
use std::process::{Command, Stdio};

/// What the server runs: the session must exist (`=name`: that exact
/// name); each line is typed literally (`-l`, so key names like `Enter`
/// in the text stay text), then Enter where asked. One tmux invocation
/// (`\;` between commands), so the lines arrive in order.
pub fn remote_command(name: &str, lines: &[(String, bool)]) -> String {
    let target = quote(&format!("={name}:"));
    let mut keys = Vec::new();
    for (line, enter) in lines {
        if !line.is_empty() {
            keys.push(format!("send-keys -t {target} -l -- {}", quote(line)));
        }
        if *enter {
            keys.push(format!("send-keys -t {target} Enter"));
        }
    }
    format!(
        "tmux has-session -t {} 2>/dev/null || {{ echo {MISSING} >&2; exit 3; }}; tmux {}",
        quote(&format!("={name}")),
        keys.join(" \\; ")
    )
}

/// What the server says when the session isn't there.
const MISSING: &str = "nativeterm-no-such-session";

/// Why a send didn't happen.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// The tmux session isn't on the server (ended, or never started).
    NoSession,
    /// ssh failed (login, network): its last message.
    Ssh(String),
}

/// Sends `lines` to the tmux session `name` on `alias`.
pub fn send(
    ssh: &Path,
    config: Option<&Path>,
    alias: &str,
    name: &str,
    lines: &[(String, bool)],
) -> Result<(), Failure> {
    let mut command = Command::new(ssh);
    if let Some(config) = config {
        command.arg("-F").arg(config);
    }
    // nothing of the host's session setup: no terminal, no forwards, no
    // LocalCommand, and the host's RemoteCommand replaced by ours
    for option in [
        "BatchMode=yes",
        "ConnectTimeout=15",
        "RequestTTY=no",
        "ClearAllForwardings=yes",
        "PermitLocalCommand=no",
        "RemoteCommand=none",
    ] {
        command.arg("-o").arg(option);
    }
    command.arg("--").arg(alias).arg(remote_command(name, lines));
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // no console window of its own
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().map_err(|e| Failure::Ssh(e.to_string()))?;
    if output.status.success() {
        return Ok(());
    }
    let text = native_term_win::ssh_message(&output.stderr);
    if text.contains(MISSING) {
        return Err(Failure::NoSession);
    }
    let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
    Err(Failure::Ssh(if last.is_empty() { format!("ssh: {}", output.status) } else { last }))
}

/// `text` in single quotes for a POSIX shell (a `'` inside as `'\''`).
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_servers_command() {
        let lines = vec![("echo it's 你好".to_string(), true), ("-v".to_string(), false), (String::new(), true)];
        let cmd = remote_command("nt-web-0f3a9c21", &lines);
        assert_eq!(
            cmd,
            "tmux has-session -t '=nt-web-0f3a9c21' 2>/dev/null || { echo nativeterm-no-such-session >&2; exit 3; }; \
             tmux send-keys -t '=nt-web-0f3a9c21:' -l -- 'echo it'\\''s 你好' \\; \
             send-keys -t '=nt-web-0f3a9c21:' Enter \\; \
             send-keys -t '=nt-web-0f3a9c21:' -l -- '-v' \\; \
             send-keys -t '=nt-web-0f3a9c21:' Enter"
        );
    }

    /// Through a real POSIX shell (Git's `sh`, if there): the quoting
    /// gives tmux exactly the arguments meant.
    #[test]
    #[cfg(windows)]
    fn quoting_survives_a_shell() {
        let sh = std::path::Path::new(r"C:\Program Files\Git\usr\bin\sh.exe");
        if !sh.exists() {
            return;
        }
        let text = "a 'b' \"c\" $HOME `x` \\ ; & | 中文";
        let out = Command::new(sh).arg("-c").arg(format!("printf '%s' {}", quote(text))).output().unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), text);
    }
}
