//! Passwords: saved ones given, and the others asked in NativeTerm's
//! window (per account, or in the host's shared credential set). Every
//! ssh the shim starts for an account it knows runs with the shim as its
//! askpass helper (forced, per process only).
//!
//! - The helper asks this shim over its private pipe, for ssh's own
//!   password prompt for this account only (`Target::answers`), and only
//!   when the helper's parent is the ssh this shim started. Every other
//!   prompt (host key, passphrase, codes, a jump host) the helper asks in
//!   the console, as ssh would.
//! - A saved password is read from the system's store when asked, never
//!   kept, and tried once (`NumberOfPasswordPrompts=1`): a login that
//!   fails after it was given marks it refused, no retries into a
//!   server-side ban; the person gives a new one.
//! - Where none is saved (or the saved one was refused), NativeTerm asks
//!   in a window of its own, as SecureCRT does: the password (kept, where
//!   "Save password" is ticked, once the login worked, and only then), or
//!   "Skip" (asked in the tab), or "Cancel" (the login given up: ssh is
//!   ended, since given nothing it would try an empty password). ssh
//!   asks as often as it would (three times): the window says when the
//!   one before was wrong. Where NativeTerm can't be reached, the tab
//!   asks.

use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use native_term_config::password::{self, Target, REFUSED};
use native_term_os::credentials;
use native_term_session::pipe;
use native_term_session::protocol::{AppMessage, PasswordAnswer, Role, ShimMessage};

use crate::askpass::Reply;

/// What the pipe serves for the current attempt.
#[derive(Default)]
struct Armed {
    target: Option<Target>,
    ssh_pid: u32,
    /// A usable password is saved for it.
    stored: bool,
    /// The saved one was refused (the window says so).
    refused: bool,
    /// Asked in the window so far, this attempt.
    asked: u32,
    /// The person chose to be asked in the tab, this attempt.
    skipped: bool,
    /// Given in the window, to be kept once the login worked.
    keep: Option<String>,
}

struct Server {
    pipe: String,
    armed: Arc<Mutex<Armed>>,
    served: Arc<AtomicU32>,
}

fn armed(armed: &Mutex<Armed>) -> std::sync::MutexGuard<'_, Armed> {
    armed.lock().unwrap_or_else(|e| e.into_inner())
}

fn server() -> Option<&'static Server> {
    static SERVER: OnceLock<Option<Server>> = OnceLock::new();
    SERVER
        .get_or_init(|| {
            let armed_: Arc<Mutex<Armed>> = Arc::default();
            let served: Arc<AtomicU32> = Arc::default();
            let (a, s) = (Arc::clone(&armed_), Arc::clone(&served));
            let pipe = crate::askpass::serve_with(move |prompt, helper| answer(&a, &s, prompt, helper)).ok()?;
            Some(Server { pipe, armed: armed_, served })
        })
        .as_ref()
}

/// What the helper of `helper` (a process id) is told for `prompt`.
fn answer(state: &Mutex<Armed>, served: &AtomicU32, prompt: &str, helper: u32) -> Reply {
    let (target, stored, refused, retry, ssh) = {
        let armed = armed(state);
        let Some(target) = armed.target.clone() else { return Reply::Ask };
        // the helper of our own ssh only, for this account's own prompt
        let ours =
            native_term_os::process::parent_pid(helper).is_some_and(|p| armed.ssh_pid != 0 && p == armed.ssh_pid);
        if !ours || !target.answers(prompt) || armed.skipped {
            return Reply::Ask;
        }
        (target, armed.stored, armed.refused, armed.asked > 0, armed.ssh_pid)
    };
    if stored {
        let saved = credentials::read(&target.name).ok().flatten().filter(|s| s.comment != REFUSED);
        return match saved {
            Some(saved) => {
                served.fetch_add(1, Ordering::SeqCst);
                Reply::Answer(saved.secret)
            }
            None => Reply::Ask,
        };
    }
    // (asked without the lock held: the person may take their time)
    let asked = ask_nativeterm(&target, retry, refused);
    let mut armed = armed(state);
    match asked {
        Some(PasswordAnswer::Given { secret, save }) => {
            armed.asked += 1;
            armed.keep = save.then(|| secret.clone());
            Reply::Answer(secret)
        }
        Some(PasswordAnswer::Cancel) => {
            // given up: ssh itself would only try an empty password, and
            // ask again
            native_term_os::process::terminate(ssh);
            Reply::Cancel
        }
        Some(PasswordAnswer::Skip) | None => {
            armed.skipped = true;
            Reply::Ask
        }
    }
}

/// How long the window may take: as long as NativeTerm waits for it.
const PATIENCE: Duration = Duration::from_secs(620);

/// NativeTerm's window's answer; `None` where NativeTerm isn't there.
fn ask_nativeterm(target: &Target, retry: bool, refused: bool) -> Option<PasswordAnswer> {
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
    let question = ShimMessage::AskPassword {
        user: target.user.clone(),
        host: target.host.clone(),
        retry,
        refused,
        can_save: credentials::supported(),
    };
    conn.send(&hello).ok()?;
    conn.send(&question).ok()?;
    loop {
        match conn.recv::<AppMessage>(PATIENCE) {
            Ok(Some(AppMessage::Password { answer })) => return Some(answer),
            // `Welcome`
            Ok(Some(_)) => continue,
            _ => return None,
        }
    }
}

/// The credential set the host `alias` uses (`NativeTermCredential`, its
/// own or its folder's), if any.
pub fn credential_set(alias: &str) -> Option<String> {
    let tree = native_term_config::tree::SessionTree::load(&crate::plink::ssh_dir());
    let (folder, host) = tree.find(alias)?;
    password::set_for_host(folder, host).map(str::to_string)
}

/// This attempt's account, and whether a password is saved for it.
pub struct Attempt {
    target: Target,
    stored: bool,
    refused: bool,
}

impl Attempt {
    /// The account (from `ssh -G`): in the credential set `set` if the host
    /// uses one, else its own.
    pub fn find(effective: &[(String, String)], set: Option<&str>) -> Option<Attempt> {
        let target = password::target(effective, set)?;
        let saved = credentials::read(&target.name).ok().flatten();
        let refused = saved.as_ref().is_some_and(|s| s.comment == REFUSED);
        Some(Attempt { stored: saved.is_some() && !refused, refused, target })
    }

    /// ssh asks the shim for every prompt (per process: never the user's
    /// environment); a saved password is tried once.
    pub fn configure(&self, ssh: &mut Command, arguments: &mut Vec<std::ffi::OsString>) -> bool {
        let Some(server) = server() else { return false };
        let Ok(shim) = std::env::current_exe() else { return false };
        ssh.env("SSH_ASKPASS", shim).env("SSH_ASKPASS_REQUIRE", "force").env(crate::askpass::PIPE_VAR, &server.pipe);
        if self.stored {
            let at = arguments.iter().position(|a| a == "--").unwrap_or(arguments.len());
            arguments.splice(at..at, ["-o".into(), "NumberOfPasswordPrompts=1".into()]);
        }
        server.served.store(0, Ordering::SeqCst);
        *armed(&server.armed) =
            Armed { target: Some(self.target.clone()), stored: self.stored, refused: self.refused, ..Armed::default() };
        true
    }

    /// The ssh that may ask.
    pub fn started(&self, ssh_pid: u32) {
        if let Some(server) = server() {
            armed(&server.armed).ssh_pid = ssh_pid;
        }
    }

    /// Logged in: a password given in the window with "Save password"
    /// ticked is kept now (a new one for a refused entry clears its mark).
    pub fn logged_in(&self) {
        let Some(server) = server() else { return };
        let Some(secret) = armed(&server.armed).keep.take() else { return };
        let saved = credentials::Saved { user: self.target.user.clone(), secret, comment: String::new() };
        if let Err(e) = credentials::write(&self.target.name, &saved) {
            println!("\r\n{}", crate::t!("saved-password-not-kept", error = e.to_string()));
        }
    }

    /// ssh has exited: nothing more is served. Whether the saved password
    /// was given.
    pub fn finish(&self) -> bool {
        let Some(server) = server() else { return false };
        *armed(&server.armed) = Armed::default();
        server.served.load(Ordering::SeqCst) > 0
    }

    /// The credential set it came from, if shared.
    pub fn set(&self) -> Option<&str> {
        self.target.set.as_deref()
    }

    /// The server refused it: keep it, marked, until the user saves a new one.
    pub fn refused(&self) {
        let _ = credentials::update(&self.target.name, |saved| saved.comment = REFUSED.to_string());
    }
}
