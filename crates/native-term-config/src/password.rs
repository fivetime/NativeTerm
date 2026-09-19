//! Saved passwords (opt-in): which Credential Manager entry belongs to a
//! host, and which prompts may be answered from it. The store itself is
//! `native_term_win::credentials`; see ARCHITECTURE.md, "Optional: saved
//! passwords via Windows Credential Manager".
//!
//! A password is kept either for one account (`NativeTerm:<user>@<host>:
//! <port>`) or in a shared credential set (`NativeTerm/cred/<name>`) that
//! hosts and folders name with `NativeTermCredential <name>`, like
//! SecureCRT's credentials. A set holds a password only: the user is the
//! ssh config's, as always.

use crate::tree::{Folder, HostEntry};

/// The entry's note once a server refused the password: it is no longer
/// used until the user saves a new one.
pub const REFUSED: &str = "refused by the server";

/// `NativeTermCredential`: the credential set of a host, or of a folder's
/// hosts; `none` on a host keeps its folder's set from it.
pub const KEY: &str = "credential";

/// The entries' common start (tests: `NATIVETERM_CRED_PREFIX` keeps
/// theirs apart).
pub(crate) fn prefix() -> String {
    std::env::var("NATIVETERM_CRED_PREFIX").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "NativeTerm".into())
}

/// Where credential sets are kept: `NativeTerm/cred/`, then the name.
pub fn set_prefix() -> String {
    format!("{}/cred/", prefix())
}

/// A credential set's Credential Manager entry.
pub fn set_entry(name: &str) -> String {
    format!("{}{name}", set_prefix())
}

/// Whether `name` can name a credential set: 1 to 64 characters, none of
/// them space, quote, `/ \ : * ?` or a control character (it is written
/// unquoted in the ssh config and is part of an entry name; `*` would
/// match others when they are listed), and not `none`.
pub fn valid_set_name(name: &str) -> bool {
    let chars = name.chars().count();
    (1..=64).contains(&chars)
        && !name.eq_ignore_ascii_case("none")
        && !name.chars().any(|c| c.is_whitespace() || c.is_control() || "\"'/\\:*?".contains(c))
}

/// The credential set a host uses: its own `NativeTermCredential`, else its
/// folder's; `none` (or a name that can't be one) is no set.
pub fn set_for_host<'a>(folder: &'a Folder, host: &'a HostEntry) -> Option<&'a str> {
    folder.nt(host, KEY).filter(|name| valid_set_name(name))
}

/// Where the password for a connection is kept: the credential set `set`
/// if the host uses one, else the account's own entry, from `ssh -G`
/// (`user`, `hostname`, `port`): `NativeTerm:<user>@<host>:<port>`, so
/// every alias of one account shares it. `None` without a user or host
/// (the prompt is still matched against the account).
pub fn target(effective: &[(String, String)], set: Option<&str>) -> Option<Target> {
    let value = |key: &str| effective.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty());
    let user = value("user")?.to_string();
    let host = value("hostname")?.to_string();
    let port = value("port").unwrap_or("22").to_string();
    let (name, set) = match set {
        Some(set) => (set_entry(set), Some(set.to_string())),
        None => (format!("{}:{user}@{host}:{port}", prefix()), None),
    };
    Some(Target { name, user, host, set })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The Credential Manager entry.
    pub name: String,
    pub user: String,
    pub host: String,
    /// The credential set it is kept in, if shared.
    pub set: Option<String>,
}

impl Target {
    /// Whether ssh's `prompt` asks for this account's password: its own
    /// format names the account (`user@host's password:`, or
    /// keyboard-interactive's `(user@host) Password:`). A jump host's
    /// prompt, a bare `Password:`, a passphrase or a code never gets it.
    pub fn answers(&self, prompt: &str) -> bool {
        let account = format!("{}@{}", self.user, self.host).to_lowercase();
        let prompt = prompt.trim().to_lowercase();
        let own = prompt.starts_with(&format!("{account}'s password:"))
            || prompt.starts_with(&format!("({account}) password:"));
        own && prompt.ends_with("password:")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effective(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn named_after_the_account() {
        let t = target(&effective(&[("user", "ops"), ("hostname", "10.0.0.5"), ("port", "2222")]), None).unwrap();
        assert_eq!(t.name, "NativeTerm:ops@10.0.0.5:2222");
        assert_eq!(t.set, None);
        assert!(target(&effective(&[("hostname", "h")]), None).is_none(), "no user");
    }

    /// A set's entry is shared; the prompt is still this account's.
    #[test]
    fn kept_in_a_set() {
        let t = target(&effective(&[("user", "ops"), ("hostname", "10.0.0.5")]), Some("机房-A")).unwrap();
        assert_eq!(t.name, "NativeTerm/cred/机房-A");
        assert_eq!(t.set.as_deref(), Some("机房-A"));
        assert!(t.answers("ops@10.0.0.5's password: "));
        assert!(!t.answers("ops@10.0.0.6's password: "), "another host's prompt");
    }

    #[test]
    fn set_names() {
        for good in ["prod", "机房-A", "db_1.old", "a"] {
            assert!(valid_set_name(good), "{good}");
        }
        let long = "x".repeat(65);
        for bad in ["", "two words", "a/b", "a:b", "a*", "q?", "\"q\"", "none", "NONE", "tab\t", long.as_str()] {
            assert!(!valid_set_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_hosts_set_or_its_folders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("config.d")).unwrap();
        std::fs::write(
            dir.path().join("config"),
            "Include config.d/*.conf\nHost own\n    HostName o\n    NativeTermCredential mine\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("config.d").join("lab.conf"),
            "Host __nativeterm_folder__\n    NativeTermCredential lab\nHost a\n    HostName a\n\
             Host b\n    HostName b\n    NativeTermCredential none\nHost c\n    HostName c\n    NativeTermCredential team\n",
        )
        .unwrap();
        let tree = crate::tree::SessionTree::load(dir.path());
        let set = |alias: &str| tree.find(alias).and_then(|(f, h)| set_for_host(f, h)).map(str::to_string);
        assert_eq!(set("own").as_deref(), Some("mine"));
        assert_eq!(set("a").as_deref(), Some("lab"), "the folder's");
        assert_eq!(set("b"), None, "none keeps the folder's away");
        assert_eq!(set("c").as_deref(), Some("team"), "the host's own first");
    }

    #[test]
    fn only_this_accounts_password_prompt() {
        let t = Target { name: String::new(), user: "ops".into(), host: "10.0.0.5".into(), set: None };
        assert!(t.answers("ops@10.0.0.5's password: "));
        assert!(t.answers("(ops@10.0.0.5) Password: "));
        assert!(!t.answers("jump@10.0.0.1's password: "), "a jump host");
        assert!(!t.answers("Password: "), "whose?");
        assert!(!t.answers("Enter passphrase for key 'C:\\k': "));
        assert!(!t.answers("(ops@10.0.0.5) Verification code: "));
        assert!(!t.answers("ops@10.0.0.5's new password: "));
    }
}
