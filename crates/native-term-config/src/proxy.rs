//! A SOCKS or HTTP proxy for ssh's connection. Windows' OpenSSH ships no
//! `nc` or `connect`, so the shim is the helper: the proxy is written as a
//! real `ProxyCommand` running `nativeterm-shim --proxy <url> %h %p`. Every
//! ssh run then uses it (tabs, files, background checks, and plain `ssh`,
//! `scp` or VS Code Remote too), and `~/.ssh` stays the only place it is
//! kept. A `ProxyCommand` of any other form is left as it is.
//!
//! A proxy that wants a login has its user name in the URL
//! (`socks5://alice@gw:1080`) and its password in Credential Manager
//! (`NativeTerm/proxy/<url>`), never in the config.

use std::path::Path;

/// The shim's option for the helper mode.
pub const FLAG: &str = "--proxy";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// SOCKS5, the name resolved by the proxy.
    Socks5,
    /// SOCKS4a (the name resolved by the proxy).
    Socks4,
    /// HTTP `CONNECT`.
    Http,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Socks5, Kind::Socks4, Kind::Http];

    pub fn scheme(self) -> &'static str {
        match self {
            Kind::Socks5 => "socks5",
            Kind::Socks4 => "socks4",
            Kind::Http => "http",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Socks5 => "SOCKS5",
            Kind::Socks4 => "SOCKS4",
            Kind::Http => "HTTP",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Kind::Socks5 | Kind::Socks4 => 1080,
            Kind::Http => 8080,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proxy {
    pub kind: Kind,
    pub host: String,
    pub port: u16,
    /// The login's user name (SOCKS4: its user id), if the proxy wants one.
    pub user: Option<String>,
}

impl Proxy {
    /// `socks5://host:port` (`socks5h`, `socks4a` and `https`-less `http`
    /// too); the port may be left out.
    pub fn parse(url: &str) -> Result<Proxy, String> {
        let (scheme, rest) = url.trim().split_once("://").ok_or_else(|| format!("not a proxy URL: {url:?}"))?;
        let kind = match scheme.to_ascii_lowercase().as_str() {
            "socks5" | "socks5h" | "socks" => Kind::Socks5,
            "socks4" | "socks4a" => Kind::Socks4,
            "http" => Kind::Http,
            other => return Err(format!("unknown proxy type {other:?} (socks5, socks4, http)")),
        };
        let rest = rest.trim_end_matches('/');
        let (user, rest) = match rest.rsplit_once('@') {
            Some((user, rest)) => {
                if user.contains(':') {
                    return Err("no password in the proxy URL: NativeTerm keeps it in Credential Manager".into());
                }
                (Some(user), rest)
            }
            None => (None, rest),
        };
        let mut proxy = Proxy::from_address(kind, rest)?;
        proxy.user = user.map(check_user).transpose()?;
        Ok(proxy)
    }

    /// `host:port` as typed in the dialog.
    pub fn from_address(kind: Kind, address: &str) -> Result<Proxy, String> {
        let (host, port) = parse_address(address.trim(), kind.default_port())?;
        Ok(Proxy { kind, host, port, user: None })
    }

    /// With a login's user name (none if `user` is blank).
    pub fn with_user(mut self, user: &str) -> Result<Proxy, String> {
        let user = user.trim();
        self.user = (!user.is_empty()).then(|| check_user(user)).transpose()?;
        Ok(self)
    }

    /// Whether the login has a password (SOCKS4 has a user id only).
    pub fn takes_password(&self) -> bool {
        self.user.is_some() && self.kind != Kind::Socks4
    }

    /// The Credential Manager entry of the login's password.
    pub fn password_entry(&self) -> Option<String> {
        self.takes_password().then(|| format!("{}/proxy/{}", crate::password::prefix(), self.url()))
    }

    pub fn address(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn url(&self) -> String {
        match &self.user {
            Some(user) => format!("{}://{user}@{}", self.kind.scheme(), self.address()),
            None => format!("{}://{}", self.kind.scheme(), self.address()),
        }
    }

    /// The `ProxyCommand` value that runs `shim` as the helper.
    pub fn command(&self, shim: &Path) -> String {
        format!("\"{}\" {FLAG} {} %h %p", shim.display(), self.url())
    }

    /// The proxy of a `ProxyCommand` value written by `command` (with any
    /// shim's path), or `None` for any other command.
    pub fn from_command(value: &str) -> Option<Proxy> {
        let value = value.trim();
        // the program, quoted or not
        let rest = match value.strip_prefix('"') {
            Some(quoted) => {
                let (program, rest) = quoted.split_once('"')?;
                is_shim(program).then_some(rest)?
            }
            None => {
                let (program, rest) = value.split_once(' ')?;
                is_shim(program).then_some(rest)?
            }
        };
        let words: Vec<&str> = rest.split_whitespace().collect();
        match words.as_slice() {
            [flag, url, "%h", "%p"] if *flag == FLAG => Proxy::parse(url).ok(),
            _ => None,
        }
    }
}

/// A user name as written in the URL (unquoted in `ProxyCommand`): no
/// blanks, quotes, `@ : / % #`. `DOMAIN\user` is fine.
fn check_user(user: &str) -> Result<String, String> {
    let bad = |c: char| c.is_whitespace() || c.is_control() || matches!(c, '"' | '@' | ':' | '/' | '%' | '#');
    if user.is_empty() || user.len() > 256 || user.chars().any(bad) {
        return Err(format!("not a proxy user name: {user:?}"));
    }
    Ok(user.to_string())
}

fn is_shim(program: &str) -> bool {
    let name = program.rsplit(['\\', '/']).next().unwrap_or(program).to_ascii_lowercase();
    name == "nativeterm-shim.exe" || name == "nativeterm-shim"
}

/// `host:port`, `[v6]:port`, or a host alone (`default` port).
fn parse_address(text: &str, default: u16) -> Result<(String, u16), String> {
    let bad = || format!("not a host and port: {text:?}");
    let (host, port) = if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or_else(bad)?;
        (host, after.strip_prefix(':'))
    } else {
        match text.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, Some(port)),
            _ => (text, None),
        }
    };
    if host.is_empty() || host.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '%' | '/' | '@')) {
        return Err(bad());
    }
    let port = match port {
        Some(p) => p.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(bad)?,
        None => default,
    };
    Ok((host.to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let p = Proxy::parse("socks5://proxy.lan:1081").unwrap();
        assert_eq!(p, Proxy { kind: Kind::Socks5, host: "proxy.lan".into(), port: 1081, user: None });
        assert_eq!(Proxy::parse("SOCKS5H://10.0.0.1").unwrap().port, 1080);
        assert_eq!(Proxy::parse("socks4a://gw:9050").unwrap().kind, Kind::Socks4);
        assert_eq!(Proxy::parse("http://[fe80::1]:3128/").unwrap().address(), "[fe80::1]:3128");
        assert_eq!(Proxy::parse("http://squid").unwrap().url(), "http://squid:8080");
        assert!(Proxy::parse("https://squid:443").is_err());
        assert!(Proxy::parse("socks5://user:pw@gw:1080").is_err(), "no passwords in the config");
        let login = Proxy::parse(r"http://CORP\alice@squid:3128").unwrap();
        assert_eq!(login.user.as_deref(), Some(r"CORP\alice"));
        assert_eq!(login.url(), r"http://CORP\alice@squid:3128");
        assert!(Proxy::from_address(Kind::Http, "squid").unwrap().with_user("a b").is_err());
        assert_eq!(Proxy::from_address(Kind::Http, "squid").unwrap().with_user(" ").unwrap().user, None);
        assert!(Proxy::parse("socks5://gw:0").is_err());
        assert!(Proxy::parse("socks5://gw:99999").is_err());
        assert!(Proxy::parse("gw:1080").is_err());
        assert!(Proxy::from_address(Kind::Http, "a b:80").is_err());
        assert!(Proxy::from_address(Kind::Http, "%h:80").is_err(), "no ssh tokens");
    }

    #[test]
    fn where_the_password_is() {
        let p = Proxy::parse("socks5://alice@gw:1080").unwrap();
        assert!(p.password_entry().unwrap().ends_with("/proxy/socks5://alice@gw:1080"));
        assert_eq!(Proxy::parse("socks5://gw").unwrap().password_entry(), None, "no login");
        assert_eq!(Proxy::parse("socks4://alice@gw").unwrap().password_entry(), None, "a user id only");
    }

    #[test]
    fn commands_both_ways() {
        let shim = Path::new(r"C:\Program Files\NativeTerm\nativeterm-shim.exe");
        let p = Proxy::from_address(Kind::Socks5, "127.0.0.1:1080").unwrap();
        let command = p.command(shim);
        assert_eq!(
            command,
            r#""C:\Program Files\NativeTerm\nativeterm-shim.exe" --proxy socks5://127.0.0.1:1080 %h %p"#
        );
        assert_eq!(Proxy::from_command(&command), Some(p.clone()));
        let login = p.clone().with_user("alice").unwrap();
        assert_eq!(Proxy::from_command(&login.command(shim)), Some(login));
        // another install's shim, unquoted
        let other = r"D:\nt\nativeterm-shim.exe --proxy http://squid:3128 %h %p";
        assert_eq!(Proxy::from_command(other).unwrap().kind, Kind::Http);
        // anything else is someone else's command
        assert_eq!(Proxy::from_command("connect -S gw:1080 %h %p"), None);
        assert_eq!(Proxy::from_command(r#""C:\x\nc.exe" --proxy socks5://gw %h %p"#), None);
        assert_eq!(Proxy::from_command(r"D:\nt\nativeterm-shim.exe --proxy socks5://gw %p %h"), None);
    }
}
