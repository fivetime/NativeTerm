//! First process of every NativeTerm tab:
//! `nativeterm-shim [<host-alias>]`
//!
//! Its session GUID comes from `WT_SESSION` (set by Windows Terminal from
//! `wt new-tab --sessionId`). Without a host alias (e.g. a tab copied by
//! Windows Terminal's "Duplicate tab"), it starts a local shell.
//!
//! Planned (see `docs/ROADMAP.md` Phase 0):
//! - register with NativeTerm over the per-user pipe
//!   (`native_term_session::PIPE_NAME_PREFIX` + SID + logon session)
//! - build the ssh command line (LocalCommand, keepalives, ...), report exit
//!   codes, stay alive after ssh exits, re-run on `Reconnect`
//! - exit 0 on `Close` so Windows Terminal closes the tab
//! - ignore Ctrl+C once ssh has exited
//! - on `SendText`, inject the text with `WriteConsoleInputW`

use std::process::Command;

fn main() {
    let session_id = std::env::var("WT_SESSION").ok();
    let Some(host_alias) = std::env::args().nth(1) else {
        let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
        let _ = Command::new(shell).status();
        return;
    };

    match Command::new("ssh").arg(&host_alias).status() {
        Ok(status) => {
            let code = status.code().unwrap_or(-1);
            let msg = match native_term_session::classify_exit(code) {
                native_term_session::SessionEnd::Disconnected => "disconnected",
                native_term_session::SessionEnd::ClosedNormally(_) => "session ended",
            };
            let session = session_id.as_deref().unwrap_or("unknown session");
            eprintln!("[nativeterm] {host_alias}: {msg} (exit code {code}, {session})");
        }
        Err(e) => eprintln!("[nativeterm] failed to start ssh: {e}"),
    }
}
