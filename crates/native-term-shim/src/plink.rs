//! Non-SSH sessions: the shim runs NativeTerm's `ntplink.exe` in the tab,
//! or PuTTY's `plink.exe` when ntplink isn't there (see
//! `native_term_config::plink` and ARCHITECTURE.md, "Other protocols via
//! plink").
//!
//! - "Connected": one of the client's TCP connections is established
//!   (serial: it has run for a second), reported through the same event
//!   as an ssh login, so post-login commands and the session card work
//!   alike.
//! - ntplink (the PuTTY fork's, see `tools/get-ntplink.ps1`)
//!   takes the session's PuTTY options on its command line (`-set`), never
//!   reads the registry, follows the tab's size, keeps Ctrl+C a key, ends
//!   when the far end closes, and takes Break and Telnet's commands over a
//!   control pipe. Exit codes: 0 closed by the far end, 2 couldn't
//!   connect, 3 connection lost (all connection-level, 255), 1 a usage
//!   error.
//! - plink needs workarounds for all of that: it always loads a temporary
//!   saved session, `NativeTerm-<pid>-<attempt>`, deleted once plink has
//!   read it (the options plink only takes from a saved session, and the
//!   tab's size; without it plink would start from PuTTY's "Default
//!   Settings"); the watcher keeps Ctrl+C a key and ends plink when a raw
//!   connection is half-closed by the server (`CLOSE_WAIT`: plink only
//!   notices at the next keystroke). Its exits 0, 1 (its own error) and
//!   INT_MAX (socket error) are all connection-level.
//! - The console code pages follow the session's charset while the client
//!   runs.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use native_term_config::plink::{self, PlinkSession, Protocol, PuttyValue};
use native_term_session::protocol::ShimMessage;

use crate::link::{Link, LinkSender};
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

/// `--ssh-dir` or `NATIVETERM_SSH_DIR` (tests): a folder other than
/// `~/.ssh`.
pub fn custom_ssh_dir() -> Option<PathBuf> {
    if let Some(dir) = SSH_DIR.get() {
        return Some(dir.clone());
    }
    std::env::var_os("NATIVETERM_SSH_DIR").filter(|d| !d.is_empty()).map(PathBuf::from)
}

/// `--ssh-dir`, `NATIVETERM_SSH_DIR` (tests), or `~/.ssh`.
pub fn ssh_dir() -> PathBuf {
    if let Some(dir) = custom_ssh_dir() {
        return dir;
    }
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
    home.join(".ssh")
}

/// The program that runs the session.
enum Client {
    Ntplink(PathBuf),
    Plink(PathBuf),
}

impl Client {
    fn path(&self) -> &PathBuf {
        match self {
            Client::Ntplink(p) | Client::Plink(p) => p,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Client::Ntplink(_) => "ntplink",
            Client::Plink(_) => "plink",
        }
    }
}

/// `NATIVETERM_NTPLINK` or `NATIVETERM_PLINK` (tests), an `ntplink.exe`
/// next to the shim or in its `tools` folder, else PuTTY's plink.
fn client() -> Option<Client> {
    let var = |name: &str| std::env::var_os(name).filter(|p| !p.is_empty()).map(PathBuf::from);
    if let Some(p) = var("NATIVETERM_NTPLINK") {
        return Some(Client::Ntplink(p));
    }
    if let Some(p) = var("NATIVETERM_PLINK") {
        return Some(Client::Plink(p));
    }
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let ntplink = [exe_dir.join("ntplink.exe"), exe_dir.join("tools").join("ntplink.exe")].into_iter().find(|p| p.is_file());
    ntplink.map(Client::Ntplink).or_else(|| plink(&exe_dir).map(Client::Plink))
}

/// A `plink.exe` next to the shim or in its `tools` folder, on `PATH`, or
/// PuTTY's installation.
fn plink(exe_dir: &std::path::Path) -> Option<PathBuf> {
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
        crate::look::apply(alias);
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
    let Some(client) = client() else {
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
    let ntplink = matches!(client, Client::Ntplink(_));
    if let Some(file) = session.log_file() {
        match ntplink {
            // PuTTY doesn't make the folder; one with `&` codes can't be
            // made before they are filled in
            true => {
                let folder = std::path::Path::new(&file).parent().filter(|f| !f.as_os_str().is_empty());
                if let Some(folder) = folder.filter(|f| !f.to_string_lossy().contains('&')) {
                    if let Err(e) = std::fs::create_dir_all(folder) {
                        let folder = folder.display().to_string();
                        println!("{}", t!("plink-log-folder", folder = folder, error = e.to_string()));
                    }
                }
            }
            false => println!("{}", t!("plink-no-log")),
        }
    }
    let temporary = match ntplink {
        true => None,
        false => match TemporarySession::create(&session, attempt) {
            Ok(t) => t,
            Err(e) => {
                println!("{}", t!("plink-load-failed", error = e.to_string()));
                return failed;
            }
        },
    };
    // Break and Telnet's commands; without the pipe the session still runs
    let control = match ntplink {
        true => match win::ControlPipe::create(&format!("{}-{attempt}", std::process::id())) {
            Ok(control) => Some(control),
            Err(e) => {
                println!("{}", t!("plink-no-control", error = e.to_string()));
                None
            }
        },
        false => None,
    };
    println!(
        "{}",
        t!("plink-connecting", target = session.target(), protocol = session.protocol.name(), client = client.name())
    );
    let _pages = win::CodePages::set(code_page);
    if let Some(auth) = auth {
        auth.reset();
    }
    let arguments = match ntplink {
        true => ntplink_arguments(&session, control.as_ref().map(|c| c.name.as_str())),
        false => session.arguments(temporary.as_ref().map(|t| t.name.as_str())),
    };
    // its own process group: Ctrl+C never ends plink (see `Watch`)
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
    let mut child = match Command::new(client.path()).args(&arguments).creation_flags(CREATE_NEW_PROCESS_GROUP).spawn() {
        Ok(child) => child,
        Err(e) => {
            let path = client.path().display().to_string();
            println!("{}", t!("plink-not-started", client = client.name(), path = path, error = e.to_string()));
            return failed;
        }
    };
    if let Some(control) = &control {
        control.started(child.id());
        let names = specials(session.protocol);
        if let (Some(link), false) = (link, names.is_empty()) {
            link.send(ShimMessage::Specials { names: names.iter().map(|n| n.to_string()).collect() });
        }
    }
    // plink reads the saved session at start; it goes when plink is done
    // at the latest (the shim may exit right after)
    let temporary = Arc::new(Mutex::new(temporary));
    let later = Arc::clone(&temporary);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        drop(later.lock().map(|mut t| t.take()));
    });
    let watch = Watch::start(child.id(), session.protocol, !ntplink, link.map(Link::sender));
    let supervised = supervise(&mut child, link, auth, control.as_ref());
    watch.stop();
    drop(temporary.lock().map(|mut t| t.take()));
    let code = match supervised {
        Supervised::Exited(code) => code,
        Supervised::Close => return Attempt::Close,
    };
    // ntplink's 0 (closed by the far end) and 3 (lost) mean it was
    // connected, even when that was too short for the watcher to see
    let connected = auth.is_some_and(|a| a.is_set()) || watch.connected() || (ntplink && matches!(code, 0 | 3));
    let ended = match ntplink {
        // closed by the far end, couldn't connect, connection lost
        true => matches!(code, 0 | 2 | 3),
        // a server's close (Telnet and raw: 0), plink's own error (1), a
        // socket error (Telnet and raw: INT_MAX), or the watcher ending a
        // half-closed raw connection
        false => matches!(code, 0 | 1 | i32::MAX) || watch.server_closed(),
    };
    Attempt::Exited { code: if ended { 255 } else { code }, connected }
}

/// ntplink's command line: the session's PuTTY options as `-set`, its
/// control pipe, then what plink would get.
fn ntplink_arguments(session: &PlinkSession, control: Option<&str>) -> Vec<String> {
    let mut args = Vec::new();
    for (key, value) in &session.putty {
        let value = match value {
            PuttyValue::Number(n) => n.to_string(),
            PuttyValue::Text(s) => s.clone(),
        };
        args.extend(["-set".to_string(), format!("{key}={value}")]);
    }
    if let Some(control) = control {
        args.extend(["-nt-control".to_string(), control.to_string()]);
    }
    args.extend(session.arguments(None));
    args
}

/// The commands ntplink takes over the control pipe for `protocol`.
fn specials(protocol: Protocol) -> &'static [&'static str] {
    match protocol {
        Protocol::Serial => &["brk"],
        Protocol::Telnet => &["brk", "ayt", "ip", "ao", "ec", "el", "ga", "nop", "abort", "susp", "eor", "eof", "synch"],
        _ => &[],
    }
}

/// Polls the client's connections and reports the connection as the
/// login; a serial line's silence is reported too. For plink
/// (`workarounds`) it also ends plink when a raw connection is half-closed
/// by the server, and keeps Ctrl+C a key: plink sets the console to
/// "processed input", where
/// Windows turns Ctrl+C into a signal that ends it instead of sending ^C
/// (the usual way to stop a command on a switch). While plink reads key
/// by key (remote echo: Telnet devices, serial) that mode is taken off
/// again; with local line editing it stays, since Backspace needs it
/// there, and the process group plink runs in ignores the signal.
struct Watch {
    stop: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
}

const WATCH_EVERY: Duration = Duration::from_millis(300);
/// plink sets the console mode at start and on every echo change.
const KEY_MODE_EVERY: Duration = Duration::from_millis(50);
const SERIAL_SETTLE: Duration = Duration::from_secs(1);
/// A serial line with nothing arriving for this long is reported quiet.
const QUIET_AFTER: Duration = Duration::from_secs(30);

fn quiet_after() -> Duration {
    // tests: NATIVETERM_QUIET_SECS
    std::env::var("NATIVETERM_QUIET_SECS").ok().and_then(|s| s.parse().ok()).map(Duration::from_secs).unwrap_or(QUIET_AFTER)
}

fn unix_seconds(at: std::time::SystemTime) -> u64 {
    at.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Watch {
    fn start(pid: u32, protocol: Protocol, workarounds: bool, link: Option<LinkSender>) -> Watch {
        let watch = Watch { stop: Arc::default(), connected: Arc::default(), closed: Arc::default() };
        let (stop, connected, closed) = (watch.stop.clone(), watch.connected.clone(), watch.closed.clone());
        let shim = std::process::id();
        let started = Instant::now();
        let keys = watch.stop.clone();
        if workarounds {
            std::thread::spawn(move || {
                while !keys.load(Ordering::Relaxed) {
                    win::keep_ctrl_c_as_input();
                    std::thread::sleep(KEY_MODE_EVERY);
                }
            });
        }
        std::thread::spawn(move || {
            // serial: what the console shows, and since when it hasn't changed
            let quiet_after = quiet_after();
            let mut screen = win::screen_fingerprint();
            let mut changed = std::time::SystemTime::now();
            let mut quiet = false;
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(WATCH_EVERY);
                if protocol == Protocol::Serial {
                    let now = win::screen_fingerprint();
                    if now != screen {
                        screen = now;
                        changed = std::time::SystemTime::now();
                        if std::mem::take(&mut quiet) {
                            if let Some(link) = &link {
                                link.send(ShimMessage::Heard);
                            }
                        }
                    } else if !quiet && changed.elapsed().unwrap_or_default() >= quiet_after {
                        quiet = true;
                        if let Some(link) = &link {
                            link.send(ShimMessage::Quiet { since: unix_seconds(changed) });
                        }
                    }
                }
                let now_connected = match protocol {
                    Protocol::Serial => started.elapsed() >= SERIAL_SETTLE,
                    _ => {
                        let states = win::tcp_states(pid);
                        if workarounds && protocol == Protocol::Raw && states.contains(&win::TCP_CLOSE_WAIT) {
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
    /// Always one, even without options: without `-load`, plink would
    /// start from PuTTY's own "Default Settings" (proxy, keepalive,
    /// terminal type, …), so a session's behaviour would depend on the
    /// user's PuTTY. With it, what isn't in the session is PuTTY's built-in
    /// default. The tab's size goes in too, for Telnet's window size.
    fn create(session: &PlinkSession, attempt: u32) -> std::io::Result<Option<TemporarySession>> {
        use native_term_win::registry::{self, RegValue};
        let base = native_term_config::putty::sessions_key();
        remove_stale(&base);
        let name = format!("{PREFIX}{}-{attempt}", std::process::id());
        if registry::user_subkeys(&base)?.iter().any(|k| k.eq_ignore_ascii_case(&name)) {
            // never overwrite a session that isn't ours
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{base}\\{name} exists")));
        }
        // plink writes no log; without a file it would never ask either
        let mut values: Vec<(&str, RegValue)> = session
            .putty
            .iter()
            .filter(|(k, _)| !plink::PUTTY_LOG.iter().any(|o| o.key() == k.as_str()))
            .map(|(k, v)| {
                let value = match v {
                    PuttyValue::Number(n) => RegValue::Dword(*n),
                    PuttyValue::Text(s) => RegValue::Str(s.clone()),
                };
                (k.as_str(), value)
            })
            .collect();
        if let Some((columns, rows)) = win::console_size() {
            values.push(("TermWidth", RegValue::Dword(columns)));
            values.push(("TermHeight", RegValue::Dword(rows)));
        }
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
