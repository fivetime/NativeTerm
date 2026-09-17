//! The user's key pairs and ssh-agent, as far as NativeTerm needs to know:
//! which keys exist, whether they have a passphrase, and whether the agent
//! holds keys. NativeTerm never reads private keys itself; it asks
//! `ssh-keygen` and `ssh-add`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Private keys in `ssh_dir`: files with a matching `.pub` next to them.
pub fn private_keys(ssh_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(ssh_dir) else { return Vec::new() };
    let mut keys: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "pub"))
        .map(|p| p.with_extension(""))
        .filter(|p| p.is_file())
        .collect();
    keys.sort();
    keys
}

fn quiet(program: &Path) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // no console window
    }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    command
}

/// Whether the key needs a passphrase: `ssh-keygen -y` with an empty one
/// succeeds only for unprotected keys. `None` if that can't be told.
pub fn has_passphrase(ssh_keygen: &Path, private_key: &Path) -> Option<bool> {
    let status = quiet(ssh_keygen).arg("-y").arg("-P").arg("").arg("-f").arg(private_key).status().ok()?;
    Some(!status.success())
}

/// What `ssh-add -l` says about the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentKeys {
    Some,
    None,
    /// No agent to talk to.
    Unreachable,
}

pub fn agent_keys(ssh_add: &Path) -> AgentKeys {
    match quiet(ssh_add).arg("-l").status().ok().and_then(|s| s.code()) {
        Some(0) => AgentKeys::Some,
        Some(1) => AgentKeys::None,
        _ => AgentKeys::Unreachable,
    }
}

/// A tool next to the `ssh` NativeTerm uses (`ssh-add`, `ssh-keygen`).
pub fn tool_for(ssh: &Path, name: &str) -> PathBuf {
    match ssh.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(dir) if dir.join(format!("{name}.exe")).exists() => dir.join(format!("{name}.exe")),
        _ => PathBuf::from(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_pairs_and_passphrases() {
        let keygen = Path::new(r"C:\Windows\System32\OpenSSH\ssh-keygen.exe");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config"), "").unwrap();
        std::fs::write(dir.path().join("lonely.pub"), "").unwrap();
        if !keygen.exists() {
            return;
        }
        for (name, pass) in [("open", ""), ("locked", "correct horse")] {
            let status = Command::new(keygen)
                .args(["-q", "-t", "ed25519", "-N", pass, "-f"])
                .arg(dir.path().join(name))
                .status()
                .unwrap();
            assert!(status.success());
        }
        let keys = private_keys(dir.path());
        let names: Vec<String> = keys.iter().map(|k| k.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["locked", "open"]);
        assert_eq!(has_passphrase(keygen, &keys[0]), Some(true));
        assert_eq!(has_passphrase(keygen, &keys[1]), Some(false));
    }

    #[test]
    fn tools_next_to_ssh() {
        let ssh = Path::new(r"C:\Windows\System32\OpenSSH\ssh.exe");
        if ssh.exists() {
            assert_eq!(tool_for(ssh, "ssh-add"), Path::new(r"C:\Windows\System32\OpenSSH\ssh-add.exe"));
        }
        assert_eq!(tool_for(Path::new("ssh"), "ssh-add"), Path::new("ssh-add"));
    }
}
