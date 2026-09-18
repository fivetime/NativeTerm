//! Command line: `nativeterm-shim [--session <id>] [--wait] [--no-forwards]
//! [<host-alias>]` (`--wait`: don't connect until told to, for restored
//! sessions; `--no-forwards`: a clone, which would clash with the original's
//! port forwards), or
//! `nativeterm-shim --authenticated <shim-pid>` (the `LocalCommand` login
//! signal).

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Shim { session: Option<String>, alias: Option<String>, flags: Flags },
    Authenticated { shim_pid: u32 },
    /// Add the public key in `key` to the host's `authorized_keys`.
    InstallKey { key: String, alias: String },
    /// The same on several hosts, the password asked once.
    InstallKeys { key: String, aliases: Vec<String> },
    /// Create a key pair at `path` (ssh-keygen asks for the passphrase).
    CreateKey { path: String },
    /// Load the default keys into ssh-agent (ssh-add asks for passphrases).
    AddKeys,
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
            "--create-key" => {
                let path = args.next().ok_or("--create-key needs a path")?;
                return Ok(Mode::CreateKey { path });
            }
            "--session" => session = Some(args.next().ok_or("--session needs a value")?),
            "--wait" => flags.wait = true,
            "--no-forwards" => flags.no_forwards = true,
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag:?}")),
            _ if alias.is_some() => return Err(format!("unexpected argument {arg:?}")),
            _ => alias = Some(arg),
        }
    }
    Ok(Mode::Shim { session, alias, flags })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Mode, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn modes() {
        let none = Flags::default();
        assert_eq!(p(&[]).unwrap(), Mode::Shim { session: None, alias: None, flags: none });
        assert_eq!(
            p(&["--session", "s1", "web01"]).unwrap(),
            Mode::Shim { session: Some("s1".into()), alias: Some("web01".into()), flags: none }
        );
        assert_eq!(p(&["web01"]).unwrap(), Mode::Shim { session: None, alias: Some("web01".into()), flags: none });
        assert_eq!(
            p(&["--session", "s1", "--wait", "--no-forwards", "web01"]).unwrap(),
            Mode::Shim {
                session: Some("s1".into()),
                alias: Some("web01".into()),
                flags: Flags { wait: true, no_forwards: true }
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
