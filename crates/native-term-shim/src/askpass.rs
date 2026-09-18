//! One password for a batch of hosts: the shim that installs the key asks
//! for it once and serves it on a private pipe; each ssh it starts runs
//! the shim again as its askpass helper (`SSH_ASKPASS`, forced), which
//! gets the password from there.
//!
//! - The password stays in the batch shim's memory: never in an argument,
//!   the environment, a file or the log. The pipe carries it, and only
//!   this user (and SYSTEM) may open it; the name is random.
//! - Only password prompts are answered (`user@host's password:`, a
//!   keyboard-interactive `Password:`). Anything else ssh asks through the
//!   helper — a new host key, a key passphrase, a one-time code — the
//!   helper asks in the console, like ssh would.

use std::hash::{BuildHasher, Hasher};
use std::io::{self, Write};
use std::time::Duration;

use native_term_session::pipe;

/// Set for ssh (and so for the helper): the batch's pipe.
pub const PIPE_VAR: &str = "NATIVETERM_ASKPASS";

/// Whether ssh asks for the account password (not a passphrase, a new
/// password or a code).
pub fn is_password_prompt(prompt: &str) -> bool {
    let prompt = prompt.trim().to_lowercase();
    // keyboard-interactive puts "(user@host) " in front
    let prompt = match prompt.strip_prefix('(').and_then(|p| p.split_once(") ")) {
        Some((_, rest)) => rest.to_string(),
        None => prompt,
    };
    let asks = prompt.ends_with("password:") || prompt.starts_with("password for ");
    asks && !["passphrase", "new ", "retype", "again", "code", "token", "otp"].iter().any(|w| prompt.contains(w))
}

/// Serve `password` to this batch's helpers; the pipe's name.
pub fn serve(password: String) -> io::Result<String> {
    let random = std::collections::hash_map::RandomState::new().build_hasher().finish();
    let name = format!(r"\\.\pipe\NativeTerm-askpass-{}-{random:016x}", std::process::id());
    let mut listener = pipe::PipeListener::bind(&name)?;
    std::thread::spawn(move || {
        while let Ok(conn) = listener.accept() {
            if let Ok(Some(prompt)) = conn.recv::<String>(Duration::from_secs(5)) {
                let answer = is_password_prompt(&prompt).then(|| password.clone());
                let _ = conn.send(&answer);
            }
        }
    });
    Ok(name)
}

/// The helper: print the answer to `prompt` for ssh; exit code.
pub fn answer(pipe_name: &str, prompt: &str) -> i32 {
    let served = pipe::connect(pipe_name, Duration::from_secs(2)).ok().and_then(|conn| {
        conn.send(&prompt.to_string()).ok()?;
        conn.recv::<Option<String>>(Duration::from_secs(10)).ok().flatten().flatten()
    });
    let answer = match served {
        Some(password) => password,
        // not a password: ask here, echoing only a yes/no question
        None => match crate::win::read_line(prompt, prompt.contains("(yes/no")) {
            Ok(line) => line,
            Err(_) => return 1,
        },
    };
    let mut out = io::stdout().lock();
    if writeln!(out, "{answer}").and_then(|_| out.flush()).is_err() {
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_prompts() {
        for yes in [
            "simon@192.0.2.1's password: ",
            "(root@web01) Password: ",
            "Password: ",
            "Password for admin@corp.example: ",
        ] {
            assert!(is_password_prompt(yes), "{yes}");
        }
        for no in [
            "Enter passphrase for key 'C:\\Users\\x\\.ssh\\id_ed25519': ",
            "Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
            "(root@web01) Verification code: ",
            "New password: ",
            "Retype new password: ",
            "(root@web01) One-time password (OTP): ",
        ] {
            assert!(!is_password_prompt(no), "{no}");
        }
    }

    /// The helper gets the password over the pipe for a password prompt,
    /// and nothing for anything else.
    #[test]
    fn serves_only_password_prompts() {
        let name = serve("s3cret".into()).unwrap();
        let ask = |prompt: &str| {
            let conn = pipe::connect(&name, Duration::from_secs(2)).unwrap();
            conn.send(&prompt.to_string()).unwrap();
            conn.recv::<Option<String>>(Duration::from_secs(5)).unwrap().unwrap()
        };
        assert_eq!(ask("u@h's password: ").as_deref(), Some("s3cret"));
        assert_eq!(ask("Enter passphrase for key 'k': "), None);
        assert_eq!(ask("(u@h) Password: ").as_deref(), Some("s3cret"));
    }
}
