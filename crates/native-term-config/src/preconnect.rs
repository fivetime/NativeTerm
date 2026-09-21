//! A command run on this computer before connecting
//! (`NativeTermPreConnect`, per host or as a folder default) —
//! SecureCRT's "Pre-connect". It is what opens the VPN, brings up a
//! tunnel, mounts a drive or wakes the machine before ssh is started.
//!
//! It runs in the tab, as the user, before every attempt, and the session
//! waits for it. What it prints is what the person sees; whether it
//! worked is their business, so a command that fails is reported and the
//! connection goes ahead (a pre-connect that must stop the connection can
//! be written to do so — see `stop_on_failure`).

use crate::tree::{Folder, HostEntry};

/// The `NativeTerm*` key (lowercase, without the prefix).
pub const KEY: &str = "preconnect";

/// A value that means "no command", as a host says it when its folder has
/// one.
pub const NONE: &str = "none";

/// `!` in front of the command: a failure stops the connection, instead
/// of being reported and passed over.
pub const STOP: char = '!';

/// The command for this host: its own, else its folder's. Nothing when
/// neither has one, or when the host says `none`.
#[must_use]
pub fn for_host(folder: &Folder, host: &HostEntry) -> Option<String> {
    command(folder.nt(host, KEY))
}

/// A `NativeTermPreConnect` value as a command.
#[must_use]
pub fn command(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty() && !value.eq_ignore_ascii_case(NONE)).then(|| value.to_string())
}

/// Whether a failure of this command stops the connection (it starts
/// with `!`), and the command without that mark.
#[must_use]
pub fn stop_on_failure(command: &str) -> (bool, &str) {
    match command.strip_prefix(STOP) {
        Some(rest) => (true, rest.trim_start()),
        None => (false, command),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_taken_as_written_and_none_means_none() {
        assert_eq!(command(Some("  rasdial work  ")).as_deref(), Some("rasdial work"));
        assert_eq!(command(Some("none")), None);
        assert_eq!(command(Some("NONE")), None);
        assert_eq!(command(Some("   ")), None);
        assert_eq!(command(None), None);
    }

    #[test]
    fn a_mark_in_front_says_a_failure_stops_the_connection() {
        assert_eq!(stop_on_failure("!ping -n 1 gw"), (true, "ping -n 1 gw"));
        assert_eq!(stop_on_failure("! ping -n 1 gw"), (true, "ping -n 1 gw"));
        assert_eq!(stop_on_failure("ping -n 1 gw"), (false, "ping -n 1 gw"));
    }
}
