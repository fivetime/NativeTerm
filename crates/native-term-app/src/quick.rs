//! Quick connect: `user@host`, `host:port`, `user@host:port` typed into
//! the search box, opened without saving first.

/// A host typed by hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickTarget {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl QuickTarget {
    /// What `ssh` gets as its destination: `user@host`, or
    /// `ssh://user@host:port` when there is a port (ssh has no other
    /// single-argument form for it).
    pub fn destination(&self) -> String {
        let user = self.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
        match self.port {
            Some(port) => format!("ssh://{user}{}:{port}", self.host),
            None => format!("{user}{}", self.host),
        }
    }

    /// The same, as a person writes it.
    pub fn label(&self) -> String {
        let user = self.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
        match self.port {
            Some(port) => format!("{user}{}:{port}", self.host),
            None => format!("{user}{}", self.host),
        }
    }
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with(['-', '.'])
        && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn valid_user(user: &str) -> bool {
    !user.is_empty() && !user.starts_with('-') && user.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '\\' | '$'))
}

/// Parse what was typed. Something that could just be a search word (no
/// `@`, no port, no dot) isn't a target.
pub fn parse(text: &str) -> Option<QuickTarget> {
    let text = text.trim().strip_prefix("ssh ").map(str::trim).unwrap_or(text.trim());
    if text.is_empty() || text.contains(char::is_whitespace) {
        return None;
    }
    let (user, rest) = match text.rsplit_once('@') {
        Some((user, rest)) => (Some(user), rest),
        None => (None, text),
    };
    if let Some(user) = user {
        if !valid_user(user) {
            return None;
        }
    }
    // IPv6: bare address only (this ssh doesn't take `ssh://[v6]:port`)
    if rest.matches(':').count() >= 2 {
        let addr = rest.trim_start_matches('[').trim_end_matches(']');
        return addr
            .parse::<std::net::Ipv6Addr>()
            .ok()
            .map(|_| QuickTarget { user: user.map(str::to_string), host: addr.to_string(), port: None });
    }
    let (host, port) = match rest.split_once(':') {
        Some((host, port)) => (host, Some(port.parse::<u16>().ok().filter(|p| *p != 0)?)),
        None => (rest, None),
    };
    if !valid_host(host) {
        return None;
    }
    let looks_like_host = user.is_some() || port.is_some() || host.contains('.');
    looks_like_host.then(|| QuickTarget { user: user.map(str::to_string), host: host.to_string(), port })
}

/// A destination NativeTerm opened for a quick connect (`user@host` or
/// `ssh://user@host:port`), back as a target.
pub fn from_destination(destination: &str) -> Option<QuickTarget> {
    parse(destination.strip_prefix("ssh://").unwrap_or(destination))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(user: Option<&str>, host: &str, port: Option<u16>) -> Option<QuickTarget> {
        Some(QuickTarget { user: user.map(str::to_string), host: host.to_string(), port })
    }

    #[test]
    fn targets() {
        assert_eq!(parse("root@10.0.0.5"), t(Some("root"), "10.0.0.5", None));
        assert_eq!(parse(" web01.example.com:2222 "), t(None, "web01.example.com", Some(2222)));
        assert_eq!(parse("ssh admin@db:2200"), t(Some("admin"), "db", Some(2200)));
        assert_eq!(parse("10.32.16.66"), t(None, "10.32.16.66", None));
        assert_eq!(parse("ops@fe80::1"), t(Some("ops"), "fe80::1", None));
        assert_eq!(parse("[2001:db8::5]"), t(None, "2001:db8::5", None));
        assert_eq!(parse("DOMAIN\\user@host"), t(Some("DOMAIN\\user"), "host", None));
    }

    #[test]
    fn not_targets() {
        for text in ["web", "生产", "a b", "root@", "@host", "-oProxyCommand=x@h", "host:0", "host:99999", "host:x", "a@-h", "", "x@h;y"] {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn destinations() {
        let with_port = parse("root@10.0.0.5:2222").unwrap();
        assert_eq!(with_port.destination(), "ssh://root@10.0.0.5:2222");
        assert_eq!(with_port.label(), "root@10.0.0.5:2222");
        assert_eq!(parse("root@10.0.0.5").unwrap().destination(), "root@10.0.0.5");
        assert_eq!(from_destination("ssh://root@10.0.0.5:2222"), Some(with_port));
        assert_eq!(from_destination("web01"), None, "a saved alias");
    }
}
