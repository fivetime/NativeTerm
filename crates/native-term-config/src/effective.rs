//! Effective settings come from ssh itself (`ssh -G`), which applies
//! `Match`, wildcards, `Include`, and defaults. NativeTerm never
//! reimplements OpenSSH's precedence rules.

use std::io;
use std::path::Path;
use std::process::Command;

/// `ssh -G <alias>` as `(keyword, value)` pairs in ssh's output order.
/// Fails with ssh's own message, e.g. "Bad owner or permissions".
pub fn effective(ssh: &Path, alias: &str) -> io::Result<Vec<(String, String)>> {
    if alias.starts_with('-') || alias.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("invalid alias {alias:?}")));
    }
    let output = Command::new(ssh).arg("-G").arg(alias).output()?;
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
}
