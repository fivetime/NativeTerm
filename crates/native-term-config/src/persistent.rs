//! Persistent sessions (`NativeTermPersistent tmux|screen|off`, per host
//! or as a folder default): the shell runs inside tmux or screen on the
//! server, so it survives drops, sleep, closed tabs and restarts, and a
//! reconnect lands back in it. See ARCHITECTURE.md, "Persistent remote
//! sessions (tmux)".

use crate::tree::{Folder, HostEntry};

/// The `NativeTerm*` key (lowercase, without the prefix).
pub const KEY: &str = "persistent";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Persistence {
    Tmux,
    Screen,
}

impl Persistence {
    pub fn name(self) -> &'static str {
        match self {
            Persistence::Tmux => "tmux",
            Persistence::Screen => "screen",
        }
    }
}

/// A `NativeTermPersistent` value: `tmux`, `screen`, or anything else
/// (`off`) for none.
pub fn parse(value: &str) -> Option<Persistence> {
    match value.trim().to_ascii_lowercase().as_str() {
        "tmux" => Some(Persistence::Tmux),
        "screen" => Some(Persistence::Screen),
        _ => None,
    }
}

/// The host's own setting, else its folder's.
pub fn for_host(folder: &Folder, host: &HostEntry) -> Option<Persistence> {
    folder.nt(host, KEY).and_then(parse)
}

/// The server-side session's name: `nt-<alias>-<8 hex of the session id>`.
/// tmux names can't hold `.` or `:`, and a `%` would be expanded by ssh in
/// `RemoteCommand`, so only letters, digits, `-` and `_` are kept.
pub fn session_name(alias: &str, session_id: &str) -> String {
    let clean = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect::<String>();
    let alias: String = clean(alias).chars().take(40).collect();
    let id: String = session_id.chars().filter(|c| c.is_ascii_hexdigit()).take(8).collect();
    format!("nt-{alias}-{}", id.to_ascii_lowercase())
}

/// The `RemoteCommand` that runs the shell inside tmux or screen:
/// attach to the named session if it exists, create it otherwise. Where
/// the program is missing, `missing` is printed and a normal login shell
/// runs instead. Wrapped in `sh -c` so it works whatever the user's login
/// shell is (bash, zsh, fish, csh).
pub fn remote_command(persistence: Persistence, name: &str, missing: &str) -> String {
    // inside single quotes: no ', and nothing ssh would expand (%)
    let missing: String = missing.chars().filter(|c| !matches!(c, '\'' | '%' | '\\' | '"' | '\r' | '\n')).collect();
    let (program, run) = match persistence {
        Persistence::Tmux => ("tmux", format!("tmux new-session -A -s {name}")),
        Persistence::Screen => ("screen", format!("screen -D -R -S {name}")),
    };
    format!(
        "sh -c 'if command -v {program} >/dev/null 2>&1; then exec {run}; fi; \
         echo \"{missing}\" >&2; exec \"${{SHELL:-/bin/sh}}\" -l'"
    )
}

/// A NativeTerm session found on a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteSession {
    pub name: String,
    pub program: Persistence,
    /// Seconds since the Unix epoch (tmux only).
    pub created: Option<u64>,
    /// A client is attached (a tab, or someone else) right now.
    pub attached: bool,
}

/// Lists tmux's and screen's sessions; the output is read by
/// `parse_sessions`. Neither program being there is no error.
pub const LIST_COMMAND: &str = "sh -c 'tmux ls -F \"tmux #{session_name} #{session_created} #{session_attached}\" 2>/dev/null; \
     screen -ls 2>/dev/null; true'";

/// NativeTerm's sessions (`nt-…`) in the output of `LIST_COMMAND`.
pub fn parse_sessions(output: &str) -> Vec<RemoteSession> {
    let mut sessions = Vec::new();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("tmux ") {
            let mut words = rest.split_whitespace();
            let (Some(name), created, attached) = (words.next(), words.next(), words.next()) else { continue };
            sessions.push(RemoteSession {
                name: name.to_string(),
                program: Persistence::Tmux,
                created: created.and_then(|c| c.parse().ok()),
                attached: attached.and_then(|a| a.parse::<u32>().ok()).is_some_and(|n| n > 0),
            });
        } else if line.starts_with('\t') {
            // screen: "\t<pid>.<name>\t(<date>)\t(Attached|Detached)"
            let Some(first) = line.split('\t').find(|f| !f.is_empty()) else { continue };
            let Some((_, name)) = first.split_once('.') else { continue };
            sessions.push(RemoteSession {
                name: name.to_string(),
                program: Persistence::Screen,
                created: None,
                attached: line.contains("(Attached)"),
            });
        }
    }
    sessions.retain(|s| s.name.starts_with("nt-"));
    sessions
}

/// Ends a session on the server.
pub fn kill_command(session: &RemoteSession) -> String {
    let name: String = session.name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    match session.program {
        Persistence::Tmux => format!("sh -c 'tmux kill-session -t ={name}'"),
        Persistence::Screen => format!("sh -c 'screen -S {name} -X quit'"),
    }
}

/// Whether `name` is one of `alias`'s sessions (`nt-<alias>-<id8>`).
pub fn belongs_to(alias: &str, name: &str) -> bool {
    id8(alias, name).is_some()
}

fn id8<'a>(alias: &str, name: &'a str) -> Option<&'a str> {
    let prefix = session_name(alias, "");
    let id = name.strip_prefix(&prefix)?;
    (id.len() == 8 && id.chars().all(|c| c.is_ascii_hexdigit())).then_some(id)
}

/// A session id for a new tab that attaches to `name`: `fresh` (a UUID)
/// with its first 8 hex digits taken from the name, so the shim's
/// `session_name` is `name` again, on every reconnect too.
pub fn session_id_for(alias: &str, name: &str, fresh: &str) -> Option<String> {
    let id = id8(alias, name)?;
    let rest = fresh.get(8..).unwrap_or("");
    Some(format!("{id}{rest}"))
}

/// Runs `command` on `alias` without a terminal, without asking anything
/// (keys or the agent only), without the host's port forwards (an open
/// tab may hold them). `config`: `-F` for a folder other than `~/.ssh`.
pub fn run_remote(
    ssh: &std::path::Path,
    config: Option<&std::path::Path>,
    alias: &str,
    command: &str,
) -> Result<String, String> {
    if alias.starts_with('-') || alias.is_empty() {
        return Err(format!("invalid alias {alias:?}"));
    }
    let mut run = std::process::Command::new(ssh);
    if let Some(config) = config {
        run.arg("-F").arg(config);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        run.creation_flags(0x0800_0000); // no console window
    }
    for option in ["BatchMode=yes", "ConnectTimeout=10", "RequestTTY=no", "ClearAllForwardings=yes", "PermitLocalCommand=no"] {
        run.arg("-o").arg(option);
    }
    // -o RemoteCommand: a host's own RemoteCommand plus a command line
    // command would make ssh refuse
    run.arg("-o").arg(format!("RemoteCommand={command}")).arg("--").arg(alias);
    let output = run.stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if message.is_empty() { format!("ssh exited with {:?}", output.status.code()) } else { message })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_are_read_from_both_programs() {
        let output = "tmux nt-web01-0f3a9c21 1789735000 1\ntmux nt-web01-11111111 1789736000 0\ntmux work 1789730000 0\n\
                      There are screens on:\n\t3797.nt-web01-5c4b3a21\t(09/18/26 13:58:19)\t(Detached)\n\
                      \t3801.other\t(09/18/26 13:59:00)\t(Attached)\n2 Sockets in /run/screen/S-root.\n";
        let sessions = parse_sessions(output);
        assert_eq!(sessions.len(), 3, "{sessions:?}");
        assert_eq!(
            sessions[0],
            RemoteSession { name: "nt-web01-0f3a9c21".into(), program: Persistence::Tmux, created: Some(1789735000), attached: true }
        );
        assert!(!sessions[1].attached);
        assert_eq!(sessions[2].program, Persistence::Screen);
        assert_eq!(sessions[2].name, "nt-web01-5c4b3a21");
        assert_eq!(parse_sessions("No Sockets found in /run/screen/S-root.\n"), []);
    }

    #[test]
    fn reopening_keeps_the_name() {
        let fresh = "abcdef01-2345-4678-9abc-def012345678";
        let id = session_id_for("ceph.osd1", "nt-ceph_osd1-0f3a9c21", fresh).unwrap();
        assert_eq!(id, "0f3a9c21-2345-4678-9abc-def012345678");
        assert_eq!(session_name("ceph.osd1", &id), "nt-ceph_osd1-0f3a9c21");
        assert!(belongs_to("ceph.osd1", "nt-ceph_osd1-0f3a9c21"));
        assert!(!belongs_to("ceph", "nt-ceph_osd1-0f3a9c21"), "another host's");
        assert!(session_id_for("web", "nt-web-xyz", fresh).is_none());
        let tmux = RemoteSession { name: "nt-web-0f3a9c21".into(), program: Persistence::Tmux, created: None, attached: false };
        assert_eq!(kill_command(&tmux), "sh -c 'tmux kill-session -t =nt-web-0f3a9c21'");
    }

    #[test]
    fn values() {
        assert_eq!(parse("tmux"), Some(Persistence::Tmux));
        assert_eq!(parse(" Screen "), Some(Persistence::Screen));
        assert_eq!(parse("off"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn names_are_safe_for_tmux_and_ssh() {
        let name = session_name("ceph-cluster.osd1", "0f3A9c21-7d4e-4b8a-9c1d-2e3f4a5b6c7d");
        assert_eq!(name, "nt-ceph-cluster_osd1-0f3a9c21");
        assert!(!session_name("a:b%c", "x").contains([':', '%', '.']));
    }

    #[test]
    fn command_attaches_or_creates_and_falls_back() {
        let command = remote_command(Persistence::Tmux, "nt-web01-0f3a9c21", "[NativeTerm] no tmux: a plain shell");
        assert_eq!(
            command,
            "sh -c 'if command -v tmux >/dev/null 2>&1; then exec tmux new-session -A -s nt-web01-0f3a9c21; fi; \
             echo \"[NativeTerm] no tmux: a plain shell\" >&2; exec \"${SHELL:-/bin/sh}\" -l'"
        );
        let screen = remote_command(Persistence::Screen, "nt-x-1", "it's 100% \"missing\"");
        assert!(screen.contains("exec screen -D -R -S nt-x-1;"), "{screen}");
        assert!(screen.contains("echo \"its 100 missing\""), "quotes and % dropped: {screen}");
    }
}
