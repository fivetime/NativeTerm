//! Saved passwords (opt-in, per account in Windows Credential Manager):
//! a session whose account has one runs ssh with the shim as its askpass
//! helper (forced, per process only) and `NumberOfPasswordPrompts=1`.
//!
//! - The helper gets the password over this shim's private pipe, only for
//!   ssh's own password prompt for this account (`Target::answers`), and
//!   only when the helper's parent is the ssh this shim started. Every
//!   other prompt (host key, passphrase, codes, a jump host) the helper
//!   asks in the console.
//! - The password is read from Credential Manager when asked, never kept.
//! - A login that fails after the password was given marks the entry
//!   refused: no retries into a server-side ban; the user updates it.

use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use native_term_config::password::{self, Target, REFUSED};
use native_term_win::credentials;

/// What the pipe serves for the current attempt.
#[derive(Default)]
struct Armed {
    target: Option<Target>,
    ssh_pid: u32,
}

struct Server {
    pipe: String,
    armed: Arc<Mutex<Armed>>,
    served: Arc<AtomicU32>,
}

fn server() -> Option<&'static Server> {
    static SERVER: OnceLock<Option<Server>> = OnceLock::new();
    SERVER
        .get_or_init(|| {
            let armed: Arc<Mutex<Armed>> = Arc::default();
            let served: Arc<AtomicU32> = Arc::default();
            let (a, s) = (Arc::clone(&armed), Arc::clone(&served));
            let pipe = crate::askpass::serve_with(move |prompt, helper| {
                let armed = a.lock().unwrap_or_else(|e| e.into_inner());
                let target = armed.target.as_ref()?;
                // the helper of our own ssh only
                let parent = native_term_win::parent_pid(helper)?;
                if armed.ssh_pid == 0 || parent != armed.ssh_pid || !target.answers(prompt) {
                    return None;
                }
                let saved = credentials::read(&target.name).ok()??;
                if saved.comment == REFUSED {
                    return None;
                }
                s.fetch_add(1, Ordering::SeqCst);
                Some(saved.secret)
            })
            .ok()?;
            Some(Server { pipe, armed, served })
        })
        .as_ref()
}

/// The saved password for this attempt's account.
pub struct Attempt {
    target: Target,
}

impl Attempt {
    /// If the account (from `ssh -G`) has a usable saved password.
    pub fn find(effective: &[(String, String)]) -> Option<Attempt> {
        let target = password::target(effective)?;
        let saved = credentials::read(&target.name).ok()??;
        (saved.comment != REFUSED).then_some(Attempt { target })
    }

    /// ssh asks the shim for every prompt (per process: never the user's
    /// environment) and tries a password once.
    pub fn configure(&self, ssh: &mut Command, arguments: &mut Vec<std::ffi::OsString>) -> bool {
        let Some(server) = server() else { return false };
        let Ok(shim) = std::env::current_exe() else { return false };
        ssh.env("SSH_ASKPASS", shim).env("SSH_ASKPASS_REQUIRE", "force").env(crate::askpass::PIPE_VAR, &server.pipe);
        let at = arguments.iter().position(|a| a == "--").unwrap_or(arguments.len());
        arguments.splice(at..at, ["-o".into(), "NumberOfPasswordPrompts=1".into()]);
        server.served.store(0, Ordering::SeqCst);
        *server.armed.lock().unwrap_or_else(|e| e.into_inner()) = Armed { target: Some(self.target.clone()), ssh_pid: 0 };
        true
    }

    /// The ssh that may ask.
    pub fn started(&self, ssh_pid: u32) {
        if let Some(server) = server() {
            server.armed.lock().unwrap_or_else(|e| e.into_inner()).ssh_pid = ssh_pid;
        }
    }

    /// ssh has exited: nothing more is served. Whether the password was
    /// given.
    pub fn finish(&self) -> bool {
        let Some(server) = server() else { return false };
        *server.armed.lock().unwrap_or_else(|e| e.into_inner()) = Armed::default();
        server.served.load(Ordering::SeqCst) > 0
    }

    /// The server refused it: keep it, marked, until the user saves a new one.
    pub fn refused(&self) {
        if let Ok(Some(mut saved)) = credentials::read(&self.target.name) {
            saved.comment = REFUSED.to_string();
            let _ = credentials::write(&self.target.name, &saved);
        }
    }
}
