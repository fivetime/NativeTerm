//! Saved passwords (opt-in): which Credential Manager entry belongs to a
//! host, and which prompts may be answered from it. The store itself is
//! `native_term_win::credentials`; see ARCHITECTURE.md, "Optional: saved
//! passwords via Windows Credential Manager".

/// The entry's note once a server refused the password: it is no longer
/// used until the user saves a new one.
pub const REFUSED: &str = "refused by the server";

/// Where the password for a connection is kept, from `ssh -G` (`user`,
/// `hostname`, `port`): `NativeTerm:<user>@<host>:<port>`, so every alias
/// of one account shares it. `None` without a user or host.
pub fn target(effective: &[(String, String)]) -> Option<Target> {
    let value = |key: &str| effective.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty());
    let user = value("user")?.to_string();
    let host = value("hostname")?.to_string();
    let port = value("port").unwrap_or("22").to_string();
    // tests: NATIVETERM_CRED_PREFIX keeps their entries apart
    let prefix =
        std::env::var("NATIVETERM_CRED_PREFIX").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "NativeTerm".into());
    Some(Target { name: format!("{prefix}:{user}@{host}:{port}"), user, host })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The Credential Manager entry.
    pub name: String,
    pub user: String,
    pub host: String,
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
        let t = target(&effective(&[("user", "ops"), ("hostname", "10.0.0.5"), ("port", "2222")])).unwrap();
        assert_eq!(t.name, "NativeTerm:ops@10.0.0.5:2222");
        assert!(target(&effective(&[("hostname", "h")])).is_none(), "no user");
    }

    #[test]
    fn only_this_accounts_password_prompt() {
        let t = Target { name: String::new(), user: "ops".into(), host: "10.0.0.5".into() };
        assert!(t.answers("ops@10.0.0.5's password: "));
        assert!(t.answers("(ops@10.0.0.5) Password: "));
        assert!(!t.answers("jump@10.0.0.1's password: "), "a jump host");
        assert!(!t.answers("Password: "), "whose?");
        assert!(!t.answers("Enter passphrase for key 'C:\\k': "));
        assert!(!t.answers("(ops@10.0.0.5) Verification code: "));
        assert!(!t.answers("ops@10.0.0.5's new password: "));
    }
}
