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
