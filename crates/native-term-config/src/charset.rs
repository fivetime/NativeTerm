//! A character set for an SSH session (`NativeTermCharset`, per host or
//! as a folder default).
//!
//! Some servers and network devices speak GBK, Big5 or another legacy
//! code page rather than UTF-8. Nothing in the path converts: `ssh`
//! passes bytes through, and so does the tab. What decides how those
//! bytes are read and drawn is the console's code page, which the shim
//! sets around the session — the same way it already does for Telnet and
//! serial sessions (see "Character sets" in `docs/ARCHITECTURE.md`).
//!
//! That is why NativeTerm needs no second SSH client for these hosts:
//! PuTTY's plink converts nothing either, and the console code page is
//! the whole of it.

use crate::tree::{Folder, HostEntry};

/// The `NativeTerm*` key (lowercase, without the prefix).
pub const KEY: &str = "charset";

/// The host's own charset, else its folder's; `None` (or `utf-8`) means
/// the default.
#[must_use]
pub fn for_host(folder: &Folder, host: &HostEntry) -> Option<String> {
    let value = folder.nt(host, KEY)?.trim();
    (!value.is_empty() && !value.eq_ignore_ascii_case("utf-8") && !value.eq_ignore_ascii_case("none"))
        .then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::SessionTree;

    #[test]
    fn a_host_takes_its_folders_charset_unless_it_has_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lab.conf");
        std::fs::write(
            &file,
            concat!(
                "Host __nativeterm_folder__\n    NativeTermCharset gbk\n\n",
                "Host sw1\n    HostName 10.0.0.1\n\n",
                "Host sw2\n    HostName 10.0.0.2\n    NativeTermCharset big5\n\n",
                "Host modern\n    HostName 10.0.0.3\n    NativeTermCharset utf-8\n",
            ),
        )
        .unwrap();
        std::fs::write(dir.path().join("config"), format!("Include {}\n", file.display())).unwrap();
        let tree = SessionTree::load_with(dir.path(), dir.path());
        let charset = |alias: &str| {
            let (folder, host) = tree.find(alias).expect(alias);
            for_host(folder, host)
        };
        assert_eq!(charset("sw1").as_deref(), Some("gbk"), "the folder's");
        assert_eq!(charset("sw2").as_deref(), Some("big5"), "its own");
        assert_eq!(charset("modern"), None, "utf-8 is the default, not a setting");
    }

    #[test]
    fn the_code_page_comes_from_the_same_table_as_the_other_sessions() {
        assert_eq!(crate::plink::code_page(Some("gbk")), Ok(936));
        assert_eq!(crate::plink::code_page(Some("big5")), Ok(950));
        assert_eq!(crate::plink::code_page(None), Ok(65001));
    }
}
