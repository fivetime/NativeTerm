//! Persistent sessions in the shim: a host with `NativeTermPersistent tmux`
//! (or `screen`, or its folder's default) gets its shell inside tmux on the
//! server, named after the tab's NativeTerm session, so a reconnect lands
//! back in it (see `native_term_config::persistent`).

use native_term_config::persistent;
use native_term_config::tree::SessionTree;

use crate::{plink, t};

/// The `RemoteCommand` for this attempt, if the host is persistent. Needs
/// the tab's session id (the server-side name must stay the same across
/// reconnects); a host with its own `RemoteCommand` is left as it is.
pub fn remote_command(alias: &str, session: Option<&str>, effective: &[(String, String)]) -> Option<String> {
    let tree = SessionTree::load(&plink::ssh_dir());
    let (folder, host) = tree.find(alias)?;
    let persistence = persistent::for_host(folder, host)?;
    let log = persistence == persistent::Persistence::Tmux && persistent::logged_for_host(folder, host);
    let session = session?;
    let own = effective.iter().any(|(k, v)| k == "remotecommand" && !v.eq_ignore_ascii_case("none"));
    if own {
        println!("{}", t!("persistent-own-command", alias = alias));
        return None;
    }
    let name = persistent::session_name(alias, session);
    println!("{}", t!("persistent-session", program = persistence.name(), name = name.as_str()));
    if log {
        println!("{}", t!("persistent-log", file = persistent::log_file(&name)));
    }
    let missing = t!("persistent-missing", program = persistence.name());
    Some(persistent::remote_command(persistence, &name, &missing, log))
}
