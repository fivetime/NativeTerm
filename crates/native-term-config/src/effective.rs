//! Effective settings come from ssh itself (`ssh -G`), which applies
//! `Match`, wildcards, `Include`, and defaults. NativeTerm never
//! reimplements OpenSSH's precedence rules.

use std::io;
use std::path::Path;
use std::process::Command;

/// `ssh -G <alias>` as `(keyword, value)` pairs in ssh's output order.
/// Fails with ssh's own message, e.g. "Bad owner or permissions".
pub fn effective(ssh: &Path, alias: &str) -> io::Result<Vec<(String, String)>> {
    effective_with(ssh, None, alias)
}

/// Same, with `-F <config>` instead of the user's `~/.ssh/config`.
pub fn effective_with(ssh: &Path, config: Option<&Path>, alias: &str) -> io::Result<Vec<(String, String)>> {
    if alias.starts_with('-') || alias.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("invalid alias {alias:?}")));
    }
    let mut command = Command::new(ssh);
    if let Some(config) = config {
        command.arg("-F").arg(config);
    }
    let output = command.arg("-G").arg(alias).output()?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(io::Error::other(message));
    }
    Ok(parse(&String::from_utf8_lossy(&output.stdout)))
}

fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(' ').unwrap_or((l, ""));
            (!k.is_empty()).then(|| (k.to_string(), v.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ssh_g_output() {
        let pairs = parse("user root\nhostname 10.32.32.130\nport 22\nforwardagent no\nlocalcommand\n");
        assert_eq!(pairs[0], ("user".into(), "root".into()));
        assert_eq!(pairs[1], ("hostname".into(), "10.32.32.130".into()));
        assert_eq!(pairs[4], ("localcommand".into(), String::new()));
    }

    #[test]
    fn rejects_option_like_aliases() {
        assert!(effective(Path::new("ssh"), "-oProxyCommand=calc").is_err());
    }

    /// Runs the system ssh; `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn system_ssh() {
        let pairs = effective(Path::new("ssh"), "example.invalid").unwrap();
        assert!(pairs.iter().any(|(k, v)| k == "hostname" && v == "example.invalid"));
    }

    /// ssh checks the permissions of every `Include`-d file, even under `-F`
    /// (readconf.c adds SSHCONF_CHECKPERM for includes). A file written by
    /// the safe writer passes; one that others may write fails, and the
    /// writer rolls a change back.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn system_ssh_checks_permissions_of_written_files() {
        use crate::write::{WriteError, Writer};
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let folder = dir.path().join("web.conf");
        std::fs::write(&config, format!("Include \"{}\"\n", folder.display())).unwrap();
        let ssh = Path::new("ssh");
        let writer = Writer::new(dir.path().join("backups"));
        let validate = || effective_with(ssh, Some(&config), "web01").map(|_| ()).map_err(|e| e.to_string());

        writer.write(&folder, "Host web01\n    HostName 10.0.0.1\n", None, validate).unwrap();
        let pairs = effective_with(ssh, Some(&config), "web01").unwrap();
        assert!(pairs.iter().any(|(k, v)| k == "hostname" && v == "10.0.0.1"), "owner-only ACL accepted");

        // let everyone write it, as a synced or shared folder might
        let sid = crate::acl::current_user_sid().unwrap();
        crate::acl::set_dacl(&folder, &format!("D:P(A;;FA;;;{sid})(A;;FA;;;WD)")).unwrap();
        let error = effective_with(ssh, Some(&config), "web01").unwrap_err().to_string();
        assert!(error.to_lowercase().contains("permission"), "{error}");

        let (_, fp) = crate::write::read(&folder).unwrap();
        let result = writer.write(&folder, "Host web01\n    HostName 10.0.0.2\n", fp, validate);
        match result {
            Err(WriteError::Rejected { reason, .. }) => assert!(reason.contains("permission"), "{reason}"),
            other => panic!("{other:?}"),
        }
        assert!(std::fs::read_to_string(&folder).unwrap().contains("10.0.0.1"), "rolled back");
    }
}
