//! Command line: `nativeterm-shim [--ssh-dir <dir>] [--session <id>] [--wait]
//! [--no-forwards] [<host-alias>]` (`--ssh-dir`: where NativeTerm's session
//! folders are, if not `~/.ssh`; `--wait`: don't connect until told to, for
//! restored sessions; `--no-forwards`: a clone, which would clash with the
//! original's port forwards), or
//! `nativeterm-shim --authenticated <shim-pid>` (the `LocalCommand` login
//! signal), `nativeterm-shim --proxy <url> <host> <port>` (the
//! `ProxyCommand` helper), or `nativeterm-shim --zmodem download|upload`
//! (rz / sz, run by NativeTerm's ssh).

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Shim {
        session: Option<String>,
        alias: Option<String>,
        flags: Flags,
        ssh_dir: Option<String>,
    },
    Authenticated {
        shim_pid: u32,
    },
    /// Add the public key in `key` to the host's `authorized_keys`.
    InstallKey {
        key: String,
        alias: String,
    },
    /// The same on several hosts, the password asked once.
    InstallKeys {
        key: String,
        aliases: Vec<String>,
    },
    /// Create a key pair at `path` (ssh-keygen asks for the passphrase).
    CreateKey {
        path: String,
    },
    /// Load the default keys into ssh-agent (ssh-add asks for passphrases).
    AddKeys,
    /// ssh's `ProxyCommand`: reach `host:port` through the proxy at `url`.
    Proxy {
        url: String,
        host: String,
        port: String,
    },
    /// A ZMODEM transfer on stdin / stdout: the server ran `sz`
    /// (`download`) or `rz` (`upload`); `escape`: ask the sender to escape
    /// every control character (`--escape-control`, for Telnet); `files`:
    /// in tmux the files window can be offered instead (`--no-files`: not
    /// for this session, e.g. Telnet).
    Zmodem {
        mode: String,
        escape: bool,
        files: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub wait: bool,
    pub no_forwards: bool,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Mode, String> {
    let mut args = args.into_iter();
    let mut session = None;
    let mut alias = None;
    let mut flags = Flags::default();
    let mut ssh_dir = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--authenticated" => {
                let pid = args.next().ok_or("--authenticated needs the shim's pid")?;
                let shim_pid = pid.parse().map_err(|_| format!("invalid pid {pid:?}"))?;
                return Ok(Mode::Authenticated { shim_pid });
            }
            "--install-key" => {
                let key = args.next().ok_or("--install-key needs the public key file")?;
                let alias = args.next().filter(|a| !a.starts_with('-')).ok_or("--install-key needs a host")?;
                return Ok(Mode::InstallKey { key, alias });
            }
            "--install-key-batch" => {
                let key = args.next().ok_or("--install-key-batch needs the public key file")?;
                let aliases: Vec<String> = args.collect();
                if aliases.is_empty() || aliases.iter().any(|a| a.starts_with('-')) {
                    return Err("--install-key-batch needs hosts".into());
                }
                return Ok(Mode::InstallKeys { key, aliases });
            }
            "--add-keys" => return Ok(Mode::AddKeys),
            "--zmodem" => {
                let mode = args.next().filter(|m| matches!(m.as_str(), "download" | "upload" | "tmux"));
                let mode = mode.ok_or("--zmodem needs download, upload or tmux")?;
                let (mut escape, mut files) = (false, true);
                for flag in args {
                    match flag.as_str() {
                        "--escape-control" => escape = true,
                        "--no-files" => files = false,
                        other => return Err(format!("--zmodem: unknown option {other}")),
                    }
                }
                return Ok(Mode::Zmodem { mode, escape, files });
            }
            "--proxy" => {
                let rest: Vec<String> = args.collect();
                let [url, host, port] = rest.as_slice() else {
                    return Err("--proxy needs a proxy URL, a host and a port".into());
                };
                return Ok(Mode::Proxy { url: url.clone(), host: host.clone(), port: port.clone() });
            }
            "--create-key" => {
                let path = args.next().ok_or("--create-key needs a path")?;
                return Ok(Mode::CreateKey { path });
            }
            "--session" => session = Some(args.next().ok_or("--session needs a value")?),
            "--ssh-dir" => ssh_dir = Some(args.next().ok_or("--ssh-dir needs a folder")?),
            "--wait" => flags.wait = true,
            "--no-forwards" => flags.no_forwards = true,
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag:?}")),
            _ if alias.is_some() => return Err(format!("unexpected argument {arg:?}")),
            _ => alias = Some(arg),
        }
    }
    Ok(Mode::Shim { session, alias, flags, ssh_dir })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Mode, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn zmodem_helper() {
        let upload = Mode::Zmodem { mode: "upload".into(), escape: false, files: true };
        assert_eq!(p(&["--zmodem", "upload"]).unwrap(), upload);
        let escaped = p(&["--zmodem", "download", "--escape-control"]).unwrap();
        assert_eq!(escaped, Mode::Zmodem { mode: "download".into(), escape: true, files: true });
        let telnet_tmux = p(&["--zmodem", "tmux", "--escape-control", "--no-files"]).unwrap();
        assert_eq!(telnet_tmux, Mode::Zmodem { mode: "tmux".into(), escape: true, files: false });
        assert!(p(&["--zmodem", "tmux", "--sideways"]).is_err());
        assert!(p(&["--zmodem", "sideways"]).is_err());
        assert!(p(&["--zmodem"]).is_err());
    }

    #[test]
    fn proxy_helper() {
        assert_eq!(
            p(&["--proxy", "socks5://gw:1080", "db.lan", "22"]).unwrap(),
            Mode::Proxy { url: "socks5://gw:1080".into(), host: "db.lan".into(), port: "22".into() }
        );
        assert!(p(&["--proxy", "socks5://gw:1080", "db.lan"]).is_err());
    }

    #[test]
    fn modes() {
        let none = Flags::default();
        assert_eq!(p(&[]).unwrap(), Mode::Shim { session: None, alias: None, flags: none, ssh_dir: None });
        assert_eq!(
            p(&["--session", "s1", "web01"]).unwrap(),
            Mode::Shim { session: Some("s1".into()), alias: Some("web01".into()), flags: none, ssh_dir: None }
        );
        assert_eq!(
            p(&["web01"]).unwrap(),
            Mode::Shim { session: None, alias: Some("web01".into()), flags: none, ssh_dir: None }
        );
        assert_eq!(
            p(&["--session", "s1", "--wait", "--no-forwards", "web01"]).unwrap(),
            Mode::Shim {
                session: Some("s1".into()),
                alias: Some("web01".into()),
                flags: Flags { wait: true, no_forwards: true },
                ssh_dir: None,
            }
        );
        assert_eq!(
            p(&["--ssh-dir", r"D:\my ssh", "--session", "s1", "sw"]).unwrap(),
            Mode::Shim {
                session: Some("s1".into()),
                alias: Some("sw".into()),
                flags: none,
                ssh_dir: Some(r"D:\my ssh".into())
            }
        );
        assert_eq!(p(&["--authenticated", "4242"]).unwrap(), Mode::Authenticated { shim_pid: 4242 });
        assert_eq!(
            p(&["--install-key", "k.pub", "web01"]).unwrap(),
            Mode::InstallKey { key: "k.pub".into(), alias: "web01".into() }
        );
        assert!(p(&["--install-key", "k.pub", "-oProxyCommand=x"]).is_err());
        assert_eq!(
            p(&["--install-key-batch", "k.pub", "web01", "web02"]).unwrap(),
            Mode::InstallKeys { key: "k.pub".into(), aliases: vec!["web01".into(), "web02".into()] }
        );
        assert!(p(&["--install-key-batch", "k.pub"]).is_err());
        assert!(p(&["--install-key-batch", "k.pub", "web01", "-oProxyCommand=x"]).is_err());
        assert_eq!(p(&["--create-key", "id"]).unwrap(), Mode::CreateKey { path: "id".into() });
    }

    #[test]
    fn errors() {
        assert!(p(&["--session"]).is_err());
        assert!(p(&["--authenticated"]).is_err());
        assert!(p(&["--authenticated", "x"]).is_err());
        assert!(p(&["-oProxyCommand=calc", "web01"]).is_err());
        assert!(p(&["a", "b"]).is_err());
    }
}
