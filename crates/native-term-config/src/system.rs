//! The system a host runs, as the person says (`NativeTermSystem`, per
//! host or as a folder's default): a short name as systems give
//! themselves in `/etc/os-release` (`fedora`, `rocky`, `macos`).
//!
//! NativeTerm knows a server's system from what the server says of itself
//! as a connection begins, and asks nothing more of it. That is the
//! family a system is of, not always the system, and some systems say
//! nothing. Where it matters, the person says what it is, and that is
//! taken before what the server said. NativeTerm only shows it (the
//! host's picture): nothing is done differently for it.
//!
//! Which names have a picture is NativeTerm's to know (`server::Os`
//! there); a name it does not know is kept as it is written and shows
//! nothing.

use crate::tree::{Folder, HostEntry};

/// The `NativeTerm*` key (lowercase, without the prefix).
pub const KEY: &str = "system";

/// What is a name: some letters, digits, `-`, `_` or `.`, one word.
#[must_use]
pub fn valid(name: &str) -> bool {
    (1..=32).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// The host's own, else its folder's; `None` where none is said.
#[must_use]
pub fn for_host<'a>(folder: &'a Folder, host: &'a HostEntry) -> Option<&'a str> {
    folder.nt(host, KEY).map(str::trim).filter(|name| valid(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_one_short_word() {
        for name in ["fedora", "rocky", "opensuse-leap", "sles_sap", "Ubuntu", "rhel9.4"] {
            assert!(valid(name), "{name}");
        }
        for name in ["", "red hat", "a\nb", "fedora;", "系统", &"x".repeat(33)] {
            assert!(!valid(name), "{name}");
        }
    }
}
