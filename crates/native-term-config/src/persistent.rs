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

#[cfg(test)]
mod tests {
    use super::*;

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
