//! Host keys, decided in NativeTerm's window instead of the tab (as
//! SecureCRT asks): ssh's own question about a host it doesn't know yet
//! ("The authenticity of host … can't be established"), and a host whose
//! key differs from the known one, which OpenSSH refuses outright and
//! NativeTerm's ssh asks about instead (its prompt starts with `MARKER`,
//! see the fork's `nativeterm/nt_hostkey.c`). Both reach the shim through
//! the askpass helper it forces on ssh (`saved.rs`).
//!
//! - A new host: "Save" answers yes (ssh keeps the key); "Once" answers
//!   yes and the key is taken out again when the session ends; "Cancel"
//!   answers no.
//! - A changed key: "Save" takes the old key out (`ssh-keygen -R`, the
//!   host's name and address, in the file it was in) and answers yes, so
//!   ssh keeps the new one as a new host's; "Cancel" answers no, and ssh
//!   refuses as it would.
//! - NativeTerm not there: a new host is asked in the tab as before; a
//!   changed key is refused as OpenSSH does.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use native_term_session::pipe;
use native_term_session::protocol::{AppMessage, HostKeyAnswer, OldHostKey, Role, ShimMessage};

use crate::askpass::Reply;

/// How NativeTerm's ssh starts the question about a changed key.
pub const MARKER: &str = "NATIVETERM-HOSTKEY-CHANGED";

/// Set for ssh by the shim: ask about a changed key (NativeTerm's ssh).
pub const ENV: &str = "NATIVETERM_HOSTKEY";

/// A question about a host key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub host: String,
    pub ip: String,
    pub key_type: String,
    pub fingerprint: String,
    /// The key known before (a changed key).
    pub old: Option<OldHostKey>,
}

/// The question in one of ssh's prompts, if it is one.
pub fn parse(prompt: &str) -> Option<Question> {
    if prompt.trim_start().starts_with(MARKER) {
        return parse_changed(prompt);
    }
    if !prompt.contains("(yes/no") {
        return None;
    }
    let (_, rest) = prompt.split_once("The authenticity of host '")?;
    let (named, _) = rest.split_once("' can't be established")?;
    // 'host (address)'; the host may be "[name]:port"
    let (host, ip) = match named.rfind(" (") {
        Some(at) if named.ends_with(')') => (&named[..at], &named[at + 2..named.len() - 1]),
        _ => (named, ""),
    };
    let line = prompt.lines().find(|l| l.contains(" key fingerprint is"))?;
    let (key_type, fingerprint) = line.split_once(" key fingerprint is")?;
    let fingerprint = fingerprint.trim_start_matches(':').trim().trim_end_matches('.');
    Some(Question {
        host: host.to_string(),
        ip: ip.to_string(),
        key_type: key_type.trim().to_string(),
        fingerprint: fingerprint.to_string(),
        old: None,
    })
}

/// `MARKER`, then "key value" lines.
fn parse_changed(prompt: &str) -> Option<Question> {
    let value = |key: &str| {
        prompt.lines().find_map(|l| l.strip_prefix(key).and_then(|v| v.strip_prefix(' '))).map(|v| v.trim().to_string())
    };
    let (old_type, old_fingerprint) = value("old")?.split_once(' ').map(|(t, f)| (t.to_string(), f.to_string()))?;
    Some(Question {
        host: value("host")?,
        ip: value("ip").unwrap_or_default(),
        key_type: value("type")?,
        fingerprint: value("new")?,
        old: Some(OldHostKey {
            key_type: old_type,
            fingerprint: old_fingerprint,
            file: value("file")?,
            line: value("line").and_then(|l| l.parse().ok()).unwrap_or(0),
        }),
    })
}

/// New hosts trusted for this connection only: taken out of `known_hosts`
/// when the session ends.
static ONCE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// What ssh is told, after asking in NativeTerm's window.
pub fn ask(question: &Question) -> Reply {
    let changed = question.old.is_some();
    let answer = match ask_nativeterm(question) {
        Some(answer) => answer,
        // nobody to ask in a window: the tab asks about a new host, and a
        // changed key is refused as OpenSSH does
        None if changed => return Reply::Answer("no".into()),
        None => return Reply::Ask,
    };
    match (answer, &question.old) {
        (HostKeyAnswer::Cancel, _) => Reply::Answer("no".into()),
        (HostKeyAnswer::Save | HostKeyAnswer::Once, Some(old)) => {
            let names = names(question);
            match native_term_config::known_hosts::remove(&ssh_keygen(), Path::new(&old.file), &names) {
                Ok(_) => Reply::Answer("yes".into()),
                Err(e) => {
                    println!("\r\n{}", crate::t!("hostkey-not-removed", error = e.to_string()));
                    Reply::Answer("no".into())
                }
            }
        }
        (HostKeyAnswer::Save, None) => Reply::Answer("yes".into()),
        (HostKeyAnswer::Once, None) => {
            ONCE.lock().unwrap_or_else(|e| e.into_inner()).extend(names(question));
            Reply::Answer("yes".into())
        }
    }
}

/// The names a key is kept under: the host, and its address.
fn names(question: &Question) -> Vec<String> {
    let mut names = vec![question.host.clone()];
    if !question.ip.is_empty() && question.ip != question.host {
        names.push(question.ip.clone());
    }
    names
}

/// The session ended: keys trusted once go (from the file ssh adds new
/// hosts to, the first `UserKnownHostsFile`).
pub fn forget_once(effective: &[(String, String)]) {
    let names: Vec<String> = std::mem::take(&mut *ONCE.lock().unwrap_or_else(|e| e.into_inner()));
    if names.is_empty() {
        return;
    }
    let Some(file) = user_known_hosts(effective) else { return };
    if let Err(e) = native_term_config::known_hosts::remove(&ssh_keygen(), &file, &names) {
        println!("\r\n{}", crate::t!("hostkey-not-removed", error = e.to_string()));
    }
}

/// The first `UserKnownHostsFile` ssh uses (`~` expanded).
fn user_known_hosts(effective: &[(String, String)]) -> Option<PathBuf> {
    let value = effective.iter().find(|(k, _)| k == "userknownhostsfile").map(|(_, v)| v.as_str())?;
    let first = value.split_whitespace().next()?.trim_matches('"');
    match first.strip_prefix("~/").or_else(|| first.strip_prefix("~\\")) {
        Some(rest) => native_term_os::home::home_dir().map(|h| h.join(rest)),
        None => Some(PathBuf::from(first)),
    }
}

/// `ssh-keygen` beside the ssh NativeTerm runs, else the one on `PATH`.
fn ssh_keygen() -> PathBuf {
    let name = format!("ssh-keygen{}", std::env::consts::EXE_SUFFIX);
    let ssh = native_term_session::ssh_program();
    match ssh.parent().map(|dir| dir.join(&name)).filter(|p| p.is_file()) {
        Some(beside) => beside,
        None => PathBuf::from(name),
    }
}

/// NativeTerm's window's answer; `None` where NativeTerm isn't there.
fn ask_nativeterm(question: &Question) -> Option<HostKeyAnswer> {
    let name = crate::pipe_name()?;
    let conn = pipe::connect(&name, Duration::from_millis(500)).ok()?;
    let hello = ShimMessage::Hello {
        protocol: native_term_session::PROTOCOL_VERSION,
        role: Role::Request,
        pid: std::process::id(),
        wt_session: crate::wt_session(),
        session: None,
        alias: None,
        terminal_window: None,
    };
    conn.send(&hello).ok()?;
    conn.send(&ShimMessage::AskHostKey {
        host: question.host.clone(),
        ip: question.ip.clone(),
        key_type: question.key_type.clone(),
        fingerprint: question.fingerprint.clone(),
        old: question.old.clone(),
    })
    .ok()?;
    loop {
        match conn.recv::<AppMessage>(Duration::from_secs(620)) {
            Ok(Some(AppMessage::HostKey { answer })) => return Some(answer),
            // `Welcome`
            Ok(Some(_)) => continue,
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_hosts_question() {
        // OpenSSH 10 ("is:") and the older wording ("is")
        let prompt = "The authenticity of host '[10.0.0.5]:2222 ([10.0.0.5]:2222)' can't be established.\n\
                      ED25519 key fingerprint is: SHA256:WYaVZbh7UoD9JZYjl2aLZeoHaiu/GqvqsyUUWQBEAyM\n\
                      This key is not known by any other names.\n\
                      Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
        let q = parse(prompt).unwrap();
        assert_eq!(q.host, "[10.0.0.5]:2222");
        assert_eq!(q.ip, "[10.0.0.5]:2222");
        assert_eq!(q.key_type, "ED25519");
        assert_eq!(q.fingerprint, "SHA256:WYaVZbh7UoD9JZYjl2aLZeoHaiu/GqvqsyUUWQBEAyM");
        assert!(q.old.is_none());
        let older = "The authenticity of host '127.0.0.1 (127.0.0.1)' can't be established.\n\
                     ECDSA key fingerprint is SHA256:abc.\n\
                     Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
        let q = parse(older).unwrap();
        assert_eq!(
            (q.host.as_str(), q.key_type.as_str(), q.fingerprint.as_str()),
            ("127.0.0.1", "ECDSA", "SHA256:abc")
        );
        // anything else is not one
        assert!(parse("Enter passphrase for key 'k': ").is_none());
        assert!(parse("simon@h's password: ").is_none());
    }

    #[test]
    fn a_changed_keys_question() {
        let prompt = "NATIVETERM-HOSTKEY-CHANGED\nhost web01.lan\nip 10.0.0.5\ntype ED25519\n\
                      new SHA256:newnewnew\nold ED25519 SHA256:oldoldold\nfile C:\\Users\\me/.ssh/known_hosts\nline 7\n";
        let q = parse(prompt).unwrap();
        assert_eq!(q.host, "web01.lan");
        assert_eq!(q.ip, "10.0.0.5");
        assert_eq!(q.fingerprint, "SHA256:newnewnew");
        let old = q.old.unwrap();
        assert_eq!(old.fingerprint, "SHA256:oldoldold");
        assert_eq!(old.file, "C:\\Users\\me/.ssh/known_hosts");
        assert_eq!(old.line, 7);
        assert_eq!(names(&Question { ip: String::new(), ..parse(prompt).unwrap() }), ["web01.lan"]);
    }

    #[test]
    fn the_known_hosts_file_ssh_adds_to() {
        let effective = vec![("userknownhostsfile".to_string(), "/tmp/kh /tmp/kh2".to_string())];
        assert_eq!(user_known_hosts(&effective), Some(PathBuf::from("/tmp/kh")));
        assert_eq!(user_known_hosts(&[]), None);
    }
}
