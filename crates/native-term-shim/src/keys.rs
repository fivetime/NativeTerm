//! Key tools run in a tab: installing a public key on a host (the user
//! types the host's password once more, here) and creating a key pair.
//!
//! The key goes in with a POSIX shell script; a Windows `sshd` runs the
//! command in `cmd.exe` or PowerShell, which rejects it, and then a
//! PowerShell script does the same the Windows way.
//!
//! Hosts that share a password go in one tab (`install_batch`): asked once,
//! handed to each ssh by the shim as its askpass helper (`askpass`).

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
    let plain =
        |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '+' | '/' | '='));
    if !plain(key_type)
        || !plain(blob)
        || !(key_type.starts_with("ssh-") || key_type.starts_with("ecdsa-") || key_type.starts_with("sk-"))
    {
        return None;
    }
    let comment: String = fields
        .collect::<Vec<_>>()
        .join("_")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
        .collect();
    let comment = if comment.is_empty() { "nativeterm".to_string() } else { comment };
    Some((format!("{key_type} {blob} {comment}"), blob.to_string()))
}

/// The POSIX shell script that adds the key unless it is there. The
/// markers are assembled when printed, so an error that quotes the script
/// back (PowerShell does) can't look like success.
pub fn remote_script(line: &str, blob: &str) -> String {
    format!(
        "umask 077; mkdir -p ~/.ssh && touch ~/.ssh/authorized_keys && \
         if grep -qF '{blob}' ~/.ssh/authorized_keys; then printf '%s%s\\n' NATIVETERM-KEY- PRESENT; else \
         if [ -s ~/.ssh/authorized_keys ] && [ \"$(tail -c1 ~/.ssh/authorized_keys)\" != \"\" ]; then echo >> ~/.ssh/authorized_keys; fi; \
         printf '%s\\n' '{line}' >> ~/.ssh/authorized_keys && printf '%s%s\\n' NATIVETERM-KEY- OK; fi"
    )
}

/// The PowerShell way, for a Windows `sshd` (Windows OpenSSH's own rules):
/// members of Administrators use `%ProgramData%\ssh\administrators_authorized_keys`
/// (which must be writable by Administrators and SYSTEM only), everyone
/// else `%USERPROFILE%\.ssh\authorized_keys`. `line` and `blob` only hold
/// characters that are safe inside single quotes (see `key_line`).
pub fn windows_script(line: &str, blob: &str) -> String {
    format!(
        r#"$ErrorActionPreference = 'Stop'
$line = '{line}'
$admins = (& "$env:SystemRoot\System32\whoami.exe" /groups) -match 'S-1-5-32-544'
if ($admins) {{ $file = Join-Path $env:ProgramData 'ssh\administrators_authorized_keys' }}
else {{ $file = Join-Path $env:USERPROFILE '.ssh\authorized_keys' }}
New-Item -ItemType Directory -Force -Path (Split-Path $file) | Out-Null
if ((Test-Path $file) -and (Select-String -Path $file -SimpleMatch -Quiet -Pattern '{blob}')) {{ 'NATIVETERM-KEY-' + 'PRESENT'; exit 0 }}
$text = if (Test-Path $file) {{ [IO.File]::ReadAllText($file) }} else {{ '' }}
if ($text.Length -gt 0 -and -not $text.EndsWith("`n")) {{ $line = "`r`n" + $line }}
[IO.File]::AppendAllText($file, $line + "`r`n", [Text.Encoding]::ASCII)
if ($admins) {{ icacls.exe $file /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F' | Out-Null }}
'NATIVETERM-KEY-' + 'OK'
"#
    )
}

/// The remote command for a Windows host: PowerShell with the script
/// encoded, so neither `cmd.exe` nor PowerShell quoting gets in the way.
pub fn windows_command(line: &str, blob: &str) -> String {
    let utf16: Vec<u8> = windows_script(line, blob).encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!("powershell -NoProfile -NonInteractive -EncodedCommand {}", base64(&utf16))
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            out.push(if i <= chunk.len() { TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// Whether the host rejected the POSIX script the way a Windows shell does.
/// Both quote our script back, in any language: cmd.exe stops at the
/// `-qF` of `if grep -qF` ("-qF was unexpected at this time", "此时不应有
/// -qF"), PowerShell's parse error repeats the line starting with `umask`.
/// A POSIX shell prints neither.
pub fn is_windows_shell(output: &str) -> bool {
    !output.contains(OK_MARK) && !output.contains(PRESENT_MARK) && (output.contains("-qF") || output.contains("umask"))
}

/// Run ssh with a remote command; (stdout + stderr, exit code). The
/// password prompt uses the console, not these pipes, or with `askpass`
/// (a batch's pipe) the shim as ssh's askpass helper.
fn run_ssh(ssh: &Path, alias: &str, command: &str, askpass: Option<&str>) -> std::io::Result<(String, i32)> {
    let mut ssh = Command::new(ssh);
    ssh.args(["-o", "ClearAllForwardings=yes", "-o", "RequestTTY=no"]);
    if let Some(pipe) = askpass {
        // one try: a wrong password fails the host instead of repeating
        ssh.args(["-o", "NumberOfPasswordPrompts=1"])
            .env("SSH_ASKPASS", std::env::current_exe()?)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env(crate::askpass::PIPE_VAR, pipe);
    }
    let mut child = ssh.args(["--", alias]).arg(command).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    // bytes, not text: a Windows shell answers in its own code page (GBK
    // here), which read_to_string would reject and drop entirely
    let mut err_pipe = child.stderr.take();
    let err_thread = std::thread::spawn(move || {
        let mut err = Vec::new();
        if let Some(e) = err_pipe.as_mut() {
            let _ = e.read_to_end(&mut err);
        }
        err
    });
    let mut bytes = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut bytes);
    }
    bytes.extend(err_thread.join().unwrap_or_default());
    let output = String::from_utf8_lossy(&bytes).to_string();
    let status = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
    Ok((output, status))
}

/// How it went on one host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Installed,
    Present,
    Failed,
}

/// The line to add and its key blob, from a public key file (or `None`
/// after saying why not).
fn read_key(key_file: &str) -> Option<(String, String)> {
    let text = match std::fs::read_to_string(key_file) {
        Ok(t) => t,
        Err(e) => {
            println!("{}", t!("key-unreadable", path = key_file, error = e.to_string()));
            return None;
        }
    };
    let key = key_line(&text);
    if key.is_none() {
        println!("{}", t!("key-unreadable", path = key_file, error = "not a public key"));
    }
    key
}

/// Add the key on one host, printing how it went.
fn install_on(ssh: &Path, alias: &str, line: &str, blob: &str, askpass: Option<&str>) -> Outcome {
    let (mut output, mut status) = match run_ssh(ssh, alias, &remote_script(line, blob), askpass) {
        Ok(result) => result,
        Err(e) => {
            println!("{}", t!("ssh-not-started", path = ssh.display().to_string(), error = e.to_string()));
            return Outcome::Failed;
        }
    };
    if is_windows_shell(&output) {
        // a Windows sshd: the same the PowerShell way (the password once more)
        println!("{}", t!("key-windows-host", alias = alias));
        match run_ssh(ssh, alias, &windows_command(line, blob), askpass) {
            Ok(result) => (output, status) = result,
            Err(e) => {
                println!("{}", t!("ssh-not-started", path = ssh.display().to_string(), error = e.to_string()));
                return Outcome::Failed;
            }
        }
    }
    if output.contains(OK_MARK) {
        println!("{}", t!("key-installed", alias = alias));
        Outcome::Installed
    } else if output.contains(PRESENT_MARK) {
        println!("{}", t!("key-present", alias = alias));
        Outcome::Present
    } else {
        print!("{output}");
        if is_login_refused(&output, status) {
            println!("{}", t!("key-login-refused", alias = alias));
        } else {
            println!("{}", t!("key-failed", alias = alias, code = status));
        }
        Outcome::Failed
    }
}

/// ssh's own "user@host: Permission denied (publickey,password)." (exit
/// 255): the login failed, the host never saw the script.
fn is_login_refused(output: &str, status: i32) -> bool {
    status == 255 && output.contains("Permission denied (")
}

pub fn install(key_file: &str, alias: &str) -> i32 {
    let Some((line, blob)) = read_key(key_file) else { return 1 };
    println!("{}", t!("key-installing", alias = alias, path = key_file));
    let ssh = native_term_session::ssh_program();
    match install_on(&ssh, alias, &line, &blob, None) {
        Outcome::Failed => 1,
        Outcome::Installed | Outcome::Present => 0,
    }
}

/// The batch password: typed in the console without echo, or a line of
/// stdin when that is a pipe (scripts, the tests).
fn ask_password(prompt: &str) -> String {
    use std::io::{BufRead, IsTerminal};
    if std::io::stdin().is_terminal() {
        return crate::win::read_line(prompt, false).unwrap_or_default();
    }
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    line.trim_end_matches(['\r', '\n']).to_string()
}

/// The key on several hosts, one after another in this tab. The password
/// is asked once, kept in memory only, and given to each ssh through the
/// askpass helper (see `askpass`); an empty answer lets every host ask.
pub fn install_batch(key_file: &str, aliases: &[String]) -> i32 {
    let Some((line, blob)) = read_key(key_file) else { return 1 };
    println!("{}", t!("key-batch-intro", count = aliases.len(), path = key_file));
    let password = ask_password(&format!("{} ", t!("key-batch-password")));
    let askpass = if password.is_empty() {
        println!("{}", t!("key-batch-each"));
        None
    } else {
        match crate::askpass::serve(password) {
            Ok(pipe) => Some(pipe),
            Err(e) => {
                println!("{}", t!("key-batch-no-helper", error = e.to_string()));
                None
            }
        }
    };
    let ssh = native_term_session::ssh_program();
    let mut failed = Vec::new();
    let (mut installed, mut present) = (0, 0);
    for alias in aliases {
        println!();
        println!("{}", t!("key-installing", alias = alias.as_str(), path = key_file));
        match install_on(&ssh, alias, &line, &blob, askpass.as_deref()) {
            Outcome::Installed => installed += 1,
            Outcome::Present => present += 1,
            Outcome::Failed => failed.push(alias.as_str()),
        }
    }
    println!();
    println!("{}", t!("key-batch-summary", installed = installed, present = present, failed = failed.len()));
    if failed.is_empty() {
        0
    } else {
        println!("{}", t!("key-batch-failed-hosts", hosts = failed.join(", ")));
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

    /// cmd.exe, like a Windows sshd's default shell, rejects the POSIX
    /// script in a way that is recognized.
    #[test]
    fn a_windows_shell_is_recognized() {
        let script = remote_script("ssh-ed25519 AAAA c", "AAAA");
        let output = Command::new("cmd.exe").arg("/c").arg(&script).output().unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(is_windows_shell(&text), "{text}");
        assert!(!is_windows_shell("NATIVETERM-KEY-OK"));
        assert!(!is_windows_shell("Permission denied (publickey,password)."));
    }

    fn run_windows_script(script: &str, dir: &Path) -> String {
        let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let output = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &base64(&utf16)])
            .env("USERPROFILE", dir.join("profile"))
            .env("ProgramData", dir.join("programdata"))
            .output()
            .unwrap();
        format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
    }

    /// The PowerShell script, run here with the profile and ProgramData
    /// pointed at a scratch folder: adds the key once (for a member of
    /// Administrators into the admin file, locked to Administrators and
    /// SYSTEM), then reports it as present.
    #[test]
    fn the_windows_script_adds_the_key_once() {
        let dir = tempfile::tempdir().unwrap();
        let script = windows_script("ssh-ed25519 AAAAtest nativeterm", "AAAAtest");
        let first = run_windows_script(&script, dir.path());
        assert!(first.contains(OK_MARK), "{first}");
        let admin = dir.path().join("programdata").join("ssh").join("administrators_authorized_keys");
        let user = dir.path().join("profile").join(".ssh").join("authorized_keys");
        if admin.exists() {
            // locked: a non-elevated test can only look at the ACL
            let acl = Command::new("icacls.exe").arg(&admin).output().unwrap();
            let acl = String::from_utf8_lossy(&acl.stdout).to_string();
            assert!(acl.contains("BUILTIN\\Administrators:(F)") && acl.contains("NT AUTHORITY\\SYSTEM:(F)"), "{acl}");
            assert_eq!(acl.matches(":(").count(), 2, "nobody else: {acl}");
            // the "already there" path, without the lock
            let dir = tempfile::tempdir().unwrap();
            let unlocked: String = script.lines().filter(|l| !l.contains("icacls")).collect::<Vec<_>>().join("\n");
            assert!(run_windows_script(&unlocked, dir.path()).contains(OK_MARK));
            let second = run_windows_script(&unlocked, dir.path());
            assert!(second.contains(PRESENT_MARK), "{second}");
            let text = std::fs::read_to_string(
                dir.path().join("programdata").join("ssh").join("administrators_authorized_keys"),
            )
            .unwrap();
            assert_eq!(text, "ssh-ed25519 AAAAtest nativeterm\r\n");
        } else {
            let second = run_windows_script(&script, dir.path());
            assert!(second.contains(PRESENT_MARK), "{second}");
            assert_eq!(std::fs::read_to_string(&user).unwrap(), "ssh-ed25519 AAAAtest nativeterm\r\n");
        }
    }

    #[test]
    fn encoded_command() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        let command = windows_command("ssh-ed25519 AAAA c", "AAAA");
        assert!(command.starts_with("powershell -NoProfile -NonInteractive -EncodedCommand "));
        assert!(command.len() < 8000, "fits a command line");
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
