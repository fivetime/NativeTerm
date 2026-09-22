//! File ACLs as Windows OpenSSH expects them: the config and every
//! included file must be owned by the user (or Administrators/SYSTEM) and
//! writable by nobody else, or ssh aborts with "Bad owner or permissions".

use std::io;
use std::path::Path;

pub use native_term_win::{file_dacl_sddl as dacl_sddl, set_file_dacl as set_dacl, user_sid as current_user_sid};

/// Protected DACL: full control for the user, Administrators, and SYSTEM
/// only (no inherited entries).
pub fn restrict_to_owner(path: &Path) -> io::Result<()> {
    set_dacl(path, &native_term_win::owner_only_sddl()?)
}

/// Whether ssh would refuse to use this file because someone other than
/// its owner can change it. Windows OpenSSH checks the file's DACL and
/// stops with "Bad owner or permissions" when an entry gives write
/// access to anyone but the user, Administrators or SYSTEM.
///
/// `Ok(None)`: the file's security couldn't be read, so nothing is said
/// about it.
pub fn open_to_others(path: &Path) -> Option<Vec<String>> {
    let sddl = dacl_sddl(path).ok()?;
    let mine = current_user_sid().ok()?;
    Some(strangers(&sddl, &mine))
}

/// Entries in a DACL that let someone else write: their SID, as the
/// descriptor writes it. Deny entries and read-only entries are fine.
fn strangers(sddl: &str, mine: &str) -> Vec<String> {
    let allowed = ["BA", "SY", "LA", "OW", "CO", mine, "S-1-5-18", "S-1-5-32-544"];
    let mut found = Vec::new();
    for ace in sddl.split('(').skip(1) {
        let Some((ace, _)) = ace.split_once(')') else { continue };
        let fields: Vec<&str> = ace.split(';').collect();
        if fields.len() < 6 || !fields[0].starts_with('A') {
            continue; // deny and audit entries don't grant anything
        }
        let (rights, sid) = (fields[2], fields[5]);
        if allowed.contains(&sid) || !writes(rights) {
            continue;
        }
        if !found.iter().any(|s| s == sid) {
            found.push(sid.to_string());
        }
    }
    found
}

/// Whether these rights include changing the file (the abbreviations
/// SDDL uses, or a hexadecimal mask).
fn writes(rights: &str) -> bool {
    if let Some(hex) = rights.strip_prefix("0x").or_else(|| rights.strip_prefix("0X")) {
        // FILE_WRITE_DATA | APPEND | WRITE_EA | WRITE_ATTRIBUTES | DELETE
        // | WRITE_DAC | WRITE_OWNER | GENERIC_WRITE | GENERIC_ALL
        const WRITE: u32 = 0x0000_0116 | 0x0001_0000 | 0x0004_0000 | 0x0008_0000 | 0x4000_0000 | 0x1000_0000;
        return u32::from_str_radix(hex, 16).is_ok_and(|mask| mask & WRITE != 0);
    }
    ["FA", "GA", "GW", "FW", "KA", "KW", "SD", "WD", "WO", "CC", "DC"].iter().any(|w| rights.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_entries_that_let_someone_else_write_count() {
        let me = "S-1-5-21-1-2-3-1001";
        let other = "S-1-5-21-1-2-3-1002";
        let owner_only = format!("D:P(A;;FA;;;{me})(A;;FA;;;BA)(A;;FA;;;SY)");
        assert!(strangers(&owner_only, me).is_empty());
        // what NativeTerm writes itself passes
        assert!(strangers(&native_term_win::owner_only_sddl().unwrap(), &current_user_sid().unwrap()).is_empty());
        // a colleague with full control is what ssh refuses
        let shared = format!("D:AI(A;;FA;;;{me})(A;;FA;;;{other})");
        assert_eq!(strangers(&shared, me), vec![other.to_string()]);
        // reading is allowed, denying is not granting
        let read_only = format!("D:AI(A;;FA;;;{me})(A;;FR;;;{other})");
        assert!(strangers(&read_only, me).is_empty());
        let denied = format!("D:AI(A;;FA;;;{me})(D;;FA;;;{other})");
        assert!(strangers(&denied, me).is_empty());
        // inherited entries count too: ssh looks at what the file allows
        let inherited = format!("D:AI(A;ID;FA;;;{me})(A;ID;0x1301bf;;;{other})");
        assert_eq!(strangers(&inherited, me), vec![other.to_string()]);
        let hex_read = format!("D:AI(A;ID;FA;;;{me})(A;ID;0x120089;;;{other})");
        assert!(strangers(&hex_read, me).is_empty());
    }

    #[test]
    fn a_file_nativeterm_wrote_is_not_open_to_others() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config");
        std::fs::write(&file, "Host a\n").unwrap();
        restrict_to_owner(&file).unwrap();
        assert_eq!(open_to_others(&file), Some(Vec::new()));
        assert_eq!(open_to_others(&dir.path().join("not there")), None);
    }
}
