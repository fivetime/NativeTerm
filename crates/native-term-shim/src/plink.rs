//! Non-SSH sessions: the shim runs PuTTY's `plink.exe` in the tab (see
//! `native_term_config::plink` and ARCHITECTURE.md, "Other protocols via
//! plink").
//!
//! - "Connected": one of plink's TCP connections is established (serial:
//!   plink has run for a second), reported through the same event as an
//!   ssh login, so post-login commands and the session card work alike.
//! - "Disconnected": plink exits 0 when a Telnet server closes, and a raw
//!   connection closed by the server only shows as `CLOSE_WAIT` (plink
//!   notices at the next keystroke), so the watcher ends plink then. Both,
//!   and plink's own error exit 1, are reported as connection-level (255).
//! - Options plink only reads from a saved session go into a temporary
//!   one, `NativeTerm-<pid>-<attempt>`, deleted once plink has read it.
//! - The console code pages follow the session's charset while plink runs.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use native_term_config::plink::{self, PlinkSession, Protocol, PuttyValue};
use native_term_session::protocol::ShimMessage;

use crate::link::Link;
use crate::{after_exit, supervise, t, win, Next, Supervised};

/// The session `alias`, if it is a non-SSH one.
pub fn lookup(alias: &str) -> Option<PlinkSession> {
    plink::find(&ssh_dir(), alias)
}

static SSH_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// `--ssh-dir`: NativeTerm runs on another folder than `~/.ssh`.
pub fn set_ssh_dir(dir: PathBuf) {
    let _ = SSH_DIR.set(dir);
}

/// `--ssh-dir`, `NATIVETERM_SSH_DIR` (tests), or `~/.ssh`.
fn ssh_dir() -> PathBuf {
    if let Some(dir) = SSH_DIR.get() {
        return dir.clone();
    }
    if let Some(dir) = std::env::var_os("NATIVETERM_SSH_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
    home.join(".ssh")
}

/// `NATIVETERM_PLINK`, a `plink.exe` next to the shim or in its `tools`
/// folder, on `PATH`, or PuTTY's installation.
fn program() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NATIVETERM_PLINK").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let mut candidates = vec![exe_dir.join("plink.exe"), exe_dir.join("tools").join("plink.exe")];
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join("plink.exe")));
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(dir) = std::env::var_os(var) {
            candidates.push(PathBuf::from(dir).join("PuTTY").join("plink.exe"));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

enum Attempt {
    Exited { code: i32, connected: bool },
    Close,
}

pub fn run(alias: &str, link: Option<&Link>, flags: crate::args::Flags) -> i32 {
    let send = |m: ShimMessage| {
        if let Some(link) = link {
            link.send(m);
        }
    };
    let pid = std::process::id();
    let auth = win::AuthEvent::create(pid).ok();
    if flags.wait {
        send(ShimMessage::Waiting);
        println!("{}", t!("restored", alias = alias));
        if let Next::Close = after_exit(link) {
            return 0;
        }
    }
    let mut attempt = 0;
    loop {
        attempt += 1;
        send(ShimMessage::Connecting { attempt });
        match attempt_once(alias, attempt, link, auth.as_ref()) {
            Attempt::Close => return 0,
            Attempt::Exited { code, connected } => {
                send(ShimMessage::Exited { code });
                let text = if connected { t!("plink-disconnected") } else { t!("plink-failed", code = code) };
                println!("\r\n{text}");
            }
        }
        match after_exit(link) {
            Next::Reconnect => continue,
            Next::Close => return 0,
        }
    }
}

fn attempt_once(alias: &str, attempt: u32, link: Option<&Link>, auth: Option<&win::AuthEvent>) -> Attempt {
    let failed = Attempt::Exited { code: 255, connected: false };
    // read again on every attempt: an edit applies at the next connect
    let Some(session) = lookup(alias) else {
        println!("{}", t!("plink-gone", alias = alias));
        return failed;
    };
    let Some(program) = program() else {
        println!("{}", t!("plink-missing"));
        return failed;
    };
    let code_page = match plink::code_page(session.charset.as_deref()) {
        Ok(cp) => cp,
        Err(e) => {
            println!("{}", t!("plink-bad-session", alias = alias, error = e));
            return failed;
        }
    };
    if let (Protocol::Serial, Some(serial)) = (session.protocol, &session.serial) {
        if let Err(e) = win::serial_port_free(&serial.line) {
            let text = if e.raw_os_error() == Some(5) {
                t!("port-busy", line = serial.line.as_str())
            } else {
                t!("port-unavailable", line = serial.line.as_str(), error = e.to_string())
            };
            println!("{text}");
            return failed;
        }
    }
    let temporary = match TemporarySession::create(&session, attempt) {
        Ok(t) => t,
        Err(e) => {
            println!("{}", t!("plink-load-failed", error = e.to_string()));
            return failed;
        }
    };
    println!("{}", t!("plink-connecting", target = session.target(), protocol = session.protocol.name()));
    let _pages = win::CodePages::set(code_page);
    if let Some(auth) = auth {
        auth.reset();
    }
    let arguments = session.arguments(temporary.as_ref().map(|t| t.name.as_str()));
    let mut child = match Command::new(&program).args(&arguments).spawn() {
        Ok(child) => child,
        Err(e) => {
            println!("{}", t!("plink-not-started", path = program.display().to_string(), error = e.to_string()));
            return failed;
        }
    };
    // plink reads the saved session at start; it goes when plink is done
    // at the latest (the shim may exit right after)
    let temporary = Arc::new(Mutex::new(temporary));
    let later = Arc::clone(&temporary);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        drop(later.lock().map(|mut t| t.take()));
    });
    let watch = Watch::start(child.id(), session.protocol);
    let supervised = supervise(&mut child, link, auth);
    watch.stop();
    drop(temporary.lock().map(|mut t| t.take()));
    let code = match supervised {
        Supervised::Exited(code) => code,
        Supervised::Close => return Attempt::Close,
    };
    let connected = auth.is_some_and(|a| a.is_set()) || watch.connected();
    // a server's close (Telnet: 0), plink's own error (1), or the watcher
    // ending a half-closed raw connection: the connection ended
    let code = if matches!(code, 0 | 1) || watch.server_closed() { 255 } else { code };
    Attempt::Exited { code, connected }
}

/// Polls plink's connections: reports the connection as the login, and
/// ends plink when a raw connection is half-closed by the server.
struct Watch {
    stop: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
}

const WATCH_EVERY: Duration = Duration::from_millis(300);
const SERIAL_SETTLE: Duration = Duration::from_secs(1);

impl Watch {
    fn start(pid: u32, protocol: Protocol) -> Watch {
        let watch = Watch { stop: Arc::default(), connected: Arc::default(), closed: Arc::default() };
        let (stop, connected, closed) = (watch.stop.clone(), watch.connected.clone(), watch.closed.clone());
        let shim = std::process::id();
        let started = Instant::now();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(WATCH_EVERY);
                let now_connected = match protocol {
                    Protocol::Serial => started.elapsed() >= SERIAL_SETTLE,
                    _ => {
                        let states = win::tcp_states(pid);
                        if protocol == Protocol::Raw && states.contains(&win::TCP_CLOSE_WAIT) {
                            closed.store(true, Ordering::Relaxed);
                            win::terminate(pid);
                            return;
                        }
                        states.contains(&win::TCP_ESTABLISHED)
                    }
                };
                if now_connected && !connected.swap(true, Ordering::Relaxed) {
                    win::signal_authenticated(shim);
                }
            }
        });
        watch
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    fn server_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}

/// A saved session holding the options plink has no flag for.
struct TemporarySession {
    name: String,
    key: String,
}

impl TemporarySession {
    fn create(session: &PlinkSession, attempt: u32) -> std::io::Result<Option<TemporarySession>> {
        use native_term_win::registry::{self, RegValue};
        if session.putty.is_empty() {
            return Ok(None);
        }
        let base = native_term_config::putty::sessions_key();
        remove_stale(&base);
        let name = format!("{PREFIX}{}-{attempt}", std::process::id());
        if registry::user_subkeys(&base)?.iter().any(|k| k.eq_ignore_ascii_case(&name)) {
            // never overwrite a session that isn't ours
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{base}\\{name} exists")));
        }
        let values: Vec<(&str, RegValue)> = session
            .putty
            .iter()
            .map(|(k, v)| {
                let value = match v {
                    PuttyValue::Number(n) => RegValue::Dword(*n),
                    PuttyValue::Text(s) => RegValue::Str(s.clone()),
                };
                (k.as_str(), value)
            })
            .collect();
        let key = format!("{base}\\{name}");
        registry::write_user_values(&key, &values)?;
        Ok(Some(TemporarySession { name, key }))
    }

}

const PREFIX: &str = "NativeTerm-";

/// Temporary sessions left by a shim that didn't get to delete them (its
/// process is gone). Only NativeTerm's own names are touched.
fn remove_stale(base: &str) {
    use native_term_win::registry;
    for name in registry::user_subkeys(base).unwrap_or_default() {
        let pid = name.strip_prefix(PREFIX).and_then(|rest| rest.split('-').next()).and_then(|p| p.parse::<u32>().ok());
        if let Some(pid) = pid {
            if pid != std::process::id() && native_term_win::process_started(pid).is_none() {
                let _ = registry::delete_user_tree(&format!("{base}\\{name}"));
            }
        }
    }
}

impl Drop for TemporarySession {
    fn drop(&mut self) {
        let _ = native_term_win::registry::delete_user_tree(&self.key);
    }
}
