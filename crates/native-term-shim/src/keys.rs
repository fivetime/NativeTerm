//! Key tools run in a tab: installing a public key on a host (the user
//! types the host's password once more, here) and creating a key pair.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::t;

const OK_MARK: &str = "NATIVETERM-KEY-OK";
const PRESENT_MARK: &str = "NATIVETERM-KEY-PRESENT";

/// `type base64 [comment]` → the line to add, with a harmless comment.
pub fn key_line(text: &str) -> Option<(String, String)> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#'))?;
    let mut fields = line.split_whitespace();
    let (key_type, blob) = (fields.next()?, fields.next()?);
    let plain = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '+' | '/' | '='));
    if !plain(key_type) || !plain(blob) || !(key_type.starts_with("ssh-") || key_type.starts_with("ecdsa-") || key_type.starts_with("sk-")) {
        return None;
    }
    let comment: String = fields.collect::<Vec<_>>().join("_").chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@')).collect();
    let comment = if comment.is_empty() { "nativeterm".to_string() } else { comment };
    Some((format!("{key_type} {blob} {comment}"), blob.to_string()))
}

/// The POSIX shell script that adds the key unless it is there.
pub fn remote_script(line: &str, blob: &str) -> String {
    format!(
        "umask 077; mkdir -p ~/.ssh && touch ~/.ssh/authorized_keys && \
         if grep -qF '{blob}' ~/.ssh/authorized_keys; then echo {PRESENT_MARK}; else \
         if [ -s ~/.ssh/authorized_keys ] && [ \"$(tail -c1 ~/.ssh/authorized_keys)\" != \"\" ]; then echo >> ~/.ssh/authorized_keys; fi; \
         printf '%s\\n' '{line}' >> ~/.ssh/authorized_keys && echo {OK_MARK}; fi"
    )
}

pub fn install(key_file: &str, alias: &str) -> i32 {
    let text = match std::fs::read_to_string(key_file) {
        Ok(t) => t,
        Err(e) => {
            println!("{}", t!("key-unreadable", path = key_file, error = e.to_string()));
            return 1;
        }
    };
    let Some((line, blob)) = key_line(&text) else {
        println!("{}", t!("key-unreadable", path = key_file, error = "not a public key"));
        return 1;
    };
    println!("{}", t!("key-installing", alias = alias, path = key_file));
    let ssh = native_term_session::ssh_program();
    // stdout is read for the result; the password prompt uses the console
    let child = Command::new(&ssh)
        .args(["-o", "ClearAllForwardings=yes", "-o", "RequestTTY=no", "--", alias])
        .arg(remote_script(&line, &blob))
        .stdout(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            println!("{}", t!("ssh-not-started", path = ssh.display().to_string(), error = e.to_string()));
            return 1;
        }
    };
    let mut output = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut output);
    }
    let status = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
    if output.contains(OK_MARK) {
        println!("{}", t!("key-installed", alias = alias));
        0
    } else if output.contains(PRESENT_MARK) {
        println!("{}", t!("key-present", alias = alias));
        0
    } else {
        print!("{output}");
        println!("{}", t!("key-failed", alias = alias, code = status));
        1
    }
}

pub fn create(path: &str) -> i32 {
    if Path::new(path).exists() {
        println!("{}", t!("key-exists", path = path));
        return 1;
    }
    if let Some(dir) = Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    println!("{}", t!("key-creating", path = path));
    let ssh = native_term_session::ssh_program();
    let keygen = native_term_config::known_hosts::ssh_keygen_for(&ssh);
    let status = Command::new(&keygen).args(["-t", "ed25519", "-f", path]).status();
    match status {
        Ok(s) if s.success() => {
            println!("{}", t!("key-created", path = format!("{path}.pub")));
            0
        }
        Ok(s) => {
            println!("{}", t!("key-create-failed", error = format!("exit {}", s.code().unwrap_or(-1))));
            1
        }
        Err(e) => {
            println!("{}", t!("key-create-failed", error = e.to_string()));
            1
        }
    }
}

pub fn add_to_agent() -> i32 {
    let ssh = native_term_session::ssh_program();
    let ssh_add = native_term_config::keys::tool_for(&ssh, "ssh-add");
    println!("{}", t!("agent-adding"));
    match Command::new(&ssh_add).status() {
        Ok(s) if s.success() => {
            println!("{}", t!("agent-added"));
            0
        }
        Ok(s) => {
            println!("{}", t!("agent-add-failed", error = format!("exit {}", s.code().unwrap_or(-1))));
            1
        }
        Err(e) => {
            println!("{}", t!("agent-add-failed", error = e.to_string()));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_lines() {
        let (line, blob) = key_line("ssh-ed25519 AAAAC3Nz me@PC 'x'\n").unwrap();
        assert_eq!(line, "ssh-ed25519 AAAAC3Nz me@PC_x");
        assert_eq!(blob, "AAAAC3Nz");
        assert_eq!(key_line("ssh-rsa AAAA").unwrap().0, "ssh-rsa AAAA nativeterm");
        assert!(key_line("-----BEGIN OPENSSH PRIVATE KEY-----").is_none());
        assert!(key_line("ssh-ed25519 AAA'B").is_none());
    }

    #[test]
    fn script_has_no_stray_quotes() {
        let script = remote_script("ssh-ed25519 AAAA c", "AAAA");
        assert_eq!(script.matches('\'').count() % 2, 0);
        assert!(script.contains("grep -qF 'AAAA'") && script.contains("'ssh-ed25519 AAAA c'"));
    }

    /// Runs the script with a POSIX shell if there is one (Git's sh).
    #[test]
    fn script_adds_once() {
        let sh = Path::new(r"C:\Program Files\Git\usr\bin\sh.exe");
        if !sh.exists() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let script = remote_script("ssh-ed25519 AAAA c", "AAAA");
        let run = || {
            let out = Command::new(sh).arg("-c").arg(&script).env("HOME", home.path()).output().unwrap();
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
        std::fs::write(home.path().join(".ssh").join("authorized_keys"), "ssh-rsa BBBB old").unwrap();
        assert!(run().contains(OK_MARK));
        assert!(run().contains(PRESENT_MARK));
        let text = std::fs::read_to_string(home.path().join(".ssh").join("authorized_keys")).unwrap();
        assert_eq!(text, "ssh-rsa BBBB old\nssh-ed25519 AAAA c\n");
    }
}
