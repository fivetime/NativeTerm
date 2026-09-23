//! The console and the process, on Unix: the same names as `win.rs`, over
//! file descriptors and signals. The shim shares its terminal with ssh
//! (no pseudo-terminal of its own), so typing into the session and
//! reading its screen are not done here — the terminal backend does
//! both — and `inject` and `screen_text` say so.
//!
//! Waiting is `poll` over descriptors: a pipe pair for an event, the
//! terminal for keys, a FIFO the login helper writes to, and a pipe a
//! SIGCHLD handler writes to for the child.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

/// What `wait_any` waits on.
pub type Handle = RawFd;

/// A pipe pair: reading blocks until `set` writes.
fn pipe_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `fds` is a two-element array the call fills.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for fd in fds {
        // SAFETY: a descriptor pipe() just gave us.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    // SAFETY: both are open descriptors owned by nobody else.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Drain everything waiting on a non-blocking descriptor.
fn drain(fd: RawFd) {
    let mut buf = [0u8; 64];
    loop {
        // SAFETY: `buf` is ours; a non-blocking read returns at once.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            return;
        }
    }
}

fn poke(fd: RawFd) {
    // SAFETY: one byte from a local; a full pipe (EAGAIN) is fine, the
    // reader wakes anyway.
    unsafe {
        libc::write(fd, [1u8].as_ptr().cast(), 1);
    }
}

/// Something to wait for, set and reset by hand.
pub struct Event {
    read: OwnedFd,
    write: OwnedFd,
}

impl Event {
    pub fn new() -> io::Result<Event> {
        let (read, write) = pipe_pair()?;
        Ok(Event { read, write })
    }

    pub fn set(&self) {
        poke(self.write.as_raw_fd());
    }

    pub fn reset(&self) {
        drain(self.read.as_raw_fd());
    }

    pub fn handle(&self) -> Handle {
        self.read.as_raw_fd()
    }
}

/// Wait until one of `handles` is readable; its index, or `None` after
/// `timeout`. On macOS with `select`: its `poll` does not take terminals
/// (it says POLLNVAL for them at once, so the terminal looked readable,
/// the key read blocked, and nothing else was heard).
#[cfg(target_os = "macos")]
pub fn wait_any(handles: &[Handle], timeout: Option<Duration>) -> Option<usize> {
    loop {
        // SAFETY: an fd_set of ours, cleared, then given open descriptors
        // below FD_SETSIZE only.
        let mut set: libc::fd_set = unsafe { std::mem::zeroed() };
        let mut top = -1;
        for &fd in handles {
            if fd < 0 || fd as usize >= libc::FD_SETSIZE {
                return None;
            }
            // SAFETY: see above.
            unsafe { libc::FD_SET(fd, &mut set) };
            top = top.max(fd);
        }
        let mut limit = timeout.map(|t| libc::timeval {
            tv_sec: t.as_secs() as libc::time_t,
            tv_usec: t.subsec_micros() as libc::suseconds_t,
        });
        let limit = limit.as_mut().map_or(std::ptr::null_mut(), |t| t as *mut libc::timeval);
        // SAFETY: `set` and `limit` are ours for the call; no write or
        // error sets.
        let n = unsafe { libc::select(top + 1, &mut set, std::ptr::null_mut(), std::ptr::null_mut(), limit) };
        if n < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return None;
        }
        if n == 0 {
            return None;
        }
        // SAFETY: `set` as select left it.
        return handles.iter().position(|&fd| unsafe { libc::FD_ISSET(fd, &set) });
    }
}

/// Wait until one of `handles` is readable; its index, or `None` after
/// `timeout`.
#[cfg(not(target_os = "macos"))]
pub fn wait_any(handles: &[Handle], timeout: Option<Duration>) -> Option<usize> {
    let mut fds: Vec<libc::pollfd> =
        handles.iter().map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
    let millis = timeout.map_or(-1, |t| t.as_millis().min(i32::MAX as u128) as libc::c_int);
    loop {
        // SAFETY: `fds` is a local array of that many entries.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, millis) };
        if n < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return None;
        }
        if n == 0 {
            return None;
        }
        return fds.iter().position(|p| p.revents != 0);
    }
}

fn auth_path(shim_pid: u32) -> PathBuf {
    // SAFETY: takes nothing, cannot fail.
    let uid = unsafe { libc::geteuid() };
    std::env::temp_dir().join(format!("nativeterm-auth-{uid}-{shim_pid}"))
}

/// The login signal: a FIFO the `LocalCommand` helper writes to. Opened
/// for reading and writing, so opening never blocks and a helper that
/// closed its end leaves no end-of-file behind.
pub struct AuthEvent {
    fifo: File,
    path: PathBuf,
    set: AtomicBool,
}

impl AuthEvent {
    pub fn create(shim_pid: u32) -> io::Result<AuthEvent> {
        let path = auth_path(shim_pid);
        let _ = std::fs::remove_file(&path);
        let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(io::Error::other)?;
        // SAFETY: a NUL-terminated path of ours; the FIFO is ours alone.
        if unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let fifo = OpenOptions::new().read(true).write(true).open(&path)?;
        // SAFETY: an open descriptor of ours.
        unsafe {
            let fd = fifo.as_raw_fd();
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        Ok(AuthEvent { fifo, path, set: AtomicBool::new(false) })
    }

    /// Whether the helper has signalled since the last `reset`.
    pub fn is_set(&self) -> bool {
        let mut buf = [0u8; 16];
        if (&self.fifo).read(&mut buf).is_ok_and(|n| n > 0) {
            self.set.store(true, Ordering::SeqCst);
        }
        self.set.load(Ordering::SeqCst)
    }

    pub fn reset(&self) {
        drain(self.fifo.as_raw_fd());
        self.set.store(false, Ordering::SeqCst);
    }

    pub fn handle(&self) -> Handle {
        self.fifo.as_raw_fd()
    }
}

impl Drop for AuthEvent {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Called by the helper; silently does nothing if the shim is gone.
pub fn signal_authenticated(shim_pid: u32) {
    let path = auth_path(shim_pid);
    let c_path = match std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) {
        Ok(p) => p,
        Err(_) => return,
    };
    // SAFETY: a NUL-terminated path; without a reader the open fails at
    // once (O_NONBLOCK) instead of waiting.
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK) };
    if fd >= 0 {
        poke(fd);
        // SAFETY: the descriptor opened above.
        unsafe {
            libc::close(fd);
        }
    }
}

/// No window of a terminal is known from inside a tab here.
pub fn terminal_window() -> Option<i64> {
    None
}

/// The terminal itself (not stdin, which may be a pipe).
fn open_tty() -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open("/dev/tty")
}

/// The terminal's settings, put back when dropped.
struct Termios {
    fd: RawFd,
    saved: libc::termios,
}

impl Termios {
    fn save(fd: RawFd) -> io::Result<Termios> {
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: `saved` is ours and the size the call expects.
        if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Termios { fd, saved })
    }

    fn apply(&self, change: impl FnOnce(&mut libc::termios)) -> io::Result<()> {
        let mut now = self.saved;
        change(&mut now);
        // SAFETY: a copy of settings tcgetattr gave, changed in place.
        if unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &now) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for Termios {
    fn drop(&mut self) {
        // SAFETY: the settings saved from this descriptor.
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved);
        }
    }
}

/// Show `prompt` and read a line from the terminal itself, without echo
/// unless `echo`. The line is returned without its line break.
pub fn read_line(prompt: &str, echo: bool) -> io::Result<String> {
    let mut tty = open_tty()?;
    tty.write_all(prompt.as_bytes())?;
    tty.flush()?;
    let modes = Termios::save(tty.as_raw_fd())?;
    modes.apply(|t| {
        t.c_lflag |= libc::ICANON;
        if echo {
            t.c_lflag |= libc::ECHO;
        } else {
            t.c_lflag &= !libc::ECHO;
        }
    })?;
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    let result = loop {
        match tty.read(&mut byte) {
            Ok(0) => break Ok(()),
            Ok(_) if byte[0] == b'\n' || byte[0] == b'\r' => break Ok(()),
            Ok(_) => bytes.push(byte[0]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => break Err(e),
        }
    };
    drop(modes);
    if !echo {
        // the Enter wasn't echoed either
        let _ = tty.write_all(b"\n");
    }
    result?;
    let line = String::from_utf8_lossy(&bytes).into_owned();
    bytes.fill(0);
    Ok(line)
}

/// Typing into the session is the terminal backend's on this system.
pub fn inject(_text: &str, _enter: bool) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "no console input to write to"))
}

/// Reads single key presses from the terminal (only while ssh isn't
/// running: ssh owns the input then). The terminal is in key-by-key mode
/// without echo for as long as this lives.
pub struct KeyReader {
    tty: File,
    _modes: Termios,
}

impl KeyReader {
    pub fn open() -> io::Result<KeyReader> {
        let tty = open_tty()?;
        let modes = Termios::save(tty.as_raw_fd())?;
        modes.apply(|t| {
            t.c_lflag &= !(libc::ICANON | libc::ECHO);
            t.c_cc[libc::VMIN] = 1;
            t.c_cc[libc::VTIME] = 0;
        })?;
        Ok(KeyReader { tty, _modes: modes })
    }

    /// Readable while a key is waiting.
    pub fn handle(&self) -> Handle {
        self.tty.as_raw_fd()
    }

    /// The next typed character, or `None` after `timeout`.
    pub fn read_key(&self, timeout: Duration) -> io::Result<Option<char>> {
        if wait_any(&[self.handle()], Some(timeout)).is_none() {
            return Ok(None);
        }
        let mut first = [0u8; 1];
        if (&self.tty).read(&mut first)? == 0 {
            return Ok(None);
        }
        let more = match first[0] {
            0..=0x7F => 0,
            0xC0..=0xDF => 1,
            0xE0..=0xEF => 2,
            0xF0..=0xF7 => 3,
            _ => return Ok(None),
        };
        let mut bytes = vec![first[0]];
        for _ in 0..more {
            let mut next = [0u8; 1];
            if (&self.tty).read(&mut next)? == 0 {
                return Ok(None);
            }
            bytes.push(next[0]);
        }
        Ok(std::str::from_utf8(&bytes).ok().and_then(|s| s.chars().next()))
    }
}

static CLOSING: OnceLock<Event> = OnceLock::new();

extern "C" fn on_hangup(_signal: libc::c_int) {
    if let Some(event) = CLOSING.get() {
        // async-signal-safe: one write to a pipe
        event.set();
    }
}

extern "C" fn on_interrupt(_signal: libc::c_int) {
    // Ctrl+C is ssh's (the same process group gets it); after ssh exits
    // it must not end the shim and the tab. A handler, not SIG_IGN: exec
    // resets handlers, so ssh gets the default back.
}

fn set_handler(signal: libc::c_int, handler: extern "C" fn(libc::c_int)) {
    // SAFETY: installing a handler that only writes to a pipe (or does
    // nothing); `sigaction` is filled by sigemptyset and the fields set.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handler as usize;
        libc::sigemptyset(&mut action.sa_mask);
        action.sa_flags = libc::SA_RESTART;
        libc::sigaction(signal, &action, std::ptr::null_mut());
    }
}

/// `on_close` runs when the terminal hangs up (`SIGHUP`, the tab closed)
/// or the process is asked to end (`SIGTERM`); the process exits after
/// it. Ctrl+C and Ctrl+\ are left to ssh.
pub fn install_ctrl_handler(on_close: impl Fn() + Send + Sync + 'static) {
    let Ok(event) = Event::new() else { return };
    if CLOSING.set(event).is_err() {
        return;
    }
    let fd = CLOSING.get().map(Event::handle).unwrap_or(-1);
    std::thread::spawn(move || {
        wait_any(&[fd], None);
        on_close();
        std::process::exit(0);
    });
    set_handler(libc::SIGHUP, on_hangup);
    set_handler(libc::SIGTERM, on_hangup);
    set_handler(libc::SIGINT, on_interrupt);
    set_handler(libc::SIGQUIT, on_interrupt);
}

/// Code pages are a Windows console's; nothing to set here.
pub struct CodePages;

impl CodePages {
    pub fn set(_code_page: u32) -> CodePages {
        CodePages
    }
}

/// The terminal's settings while a client runs, put back when dropped: a
/// client that dies badly can leave the terminal in raw mode.
pub struct ConsoleModes {
    _tty: Option<File>,
    _modes: Option<Termios>,
}

impl ConsoleModes {
    pub fn save() -> ConsoleModes {
        let tty = open_tty().ok();
        let modes = tty.as_ref().and_then(|t| Termios::save(t.as_raw_fd()).ok());
        ConsoleModes { _tty: tty, _modes: modes }
    }
}

/// The screen is the terminal's; the backend reads it.
pub fn screen_text(_max_lines: usize) -> Option<(u16, Vec<String>)> {
    None
}

/// TCP states as `/proc/<pid>/net/tcp` numbers them.
pub const TCP_ESTABLISHED: i32 = 1;
/// Asked about by the Windows side only (ntplink); named here for parity.
#[allow(dead_code)]
pub const TCP_CLOSE_WAIT: i32 = 8;

/// The states of `pid`'s TCP connections (Linux: its socket inodes looked
/// up in its `net/tcp` tables; nothing elsewhere yet).
pub fn tcp_states(pid: u32) -> Vec<i32> {
    #[cfg(target_os = "linux")]
    {
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { return Vec::new() };
        let inodes: Vec<String> = fds
            .flatten()
            .filter_map(|e| std::fs::read_link(e.path()).ok())
            .filter_map(|l| {
                let l = l.to_string_lossy().into_owned();
                l.strip_prefix("socket:[").and_then(|s| s.strip_suffix(']')).map(str::to_string)
            })
            .collect();
        if inodes.is_empty() {
            return Vec::new();
        }
        let mut states = Vec::new();
        for table in ["tcp", "tcp6"] {
            let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/net/{table}")) else { continue };
            for line in text.lines().skip(1) {
                let fields: Vec<&str> = line.split_whitespace().collect();
                // sl local remote st ... uid timeout inode
                if fields.len() < 10 {
                    continue;
                }
                if inodes.iter().any(|i| i == fields[9]) {
                    if let Ok(state) = i32::from_str_radix(fields[3], 16) {
                        states.push(state);
                    }
                }
            }
        }
        states
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        Vec::new()
    }
}

static CHILD_EXITED: OnceLock<Event> = OnceLock::new();

extern "C" fn on_child(_signal: libc::c_int) {
    if let Some(event) = CHILD_EXITED.get() {
        event.set();
    }
}

/// What to wait on for `child` to exit: a pipe a `SIGCHLD` handler
/// writes to (any child's exit wakes the waiter, which then asks the
/// child it cares about).
pub fn child_handle(_child: &Child) -> Handle {
    let event = CHILD_EXITED.get_or_init(|| {
        let event = Event::new().expect("a pipe for SIGCHLD");
        set_handler(libc::SIGCHLD, on_child);
        event
    });
    event.reset();
    event.handle()
}

/// The user's shell, for a tab that is theirs.
pub fn local_shell() -> Command {
    Command::new(std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into()))
}

/// A command line the way a person types it here: through `sh -c`.
pub fn local_command(command: &str) -> Command {
    let mut sh = Command::new("/bin/sh");
    sh.arg("-c").arg(command);
    sh
}

/// ntplink's control pipe exists on Windows only; nothing is ever sent
/// through this one.
pub struct ControlPipe;

impl ControlPipe {
    pub fn send(&self, _command: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_wakes_a_waiter_once() {
        let event = Event::new().unwrap();
        assert_eq!(wait_any(&[event.handle()], Some(Duration::from_millis(20))), None);
        event.set();
        assert_eq!(wait_any(&[event.handle()], Some(Duration::from_secs(1))), Some(0));
        event.reset();
        assert_eq!(wait_any(&[event.handle()], Some(Duration::from_millis(20))), None);
    }

    #[test]
    fn the_login_signal_arrives_through_the_fifo() {
        let auth = AuthEvent::create(std::process::id()).unwrap();
        assert!(!auth.is_set());
        signal_authenticated(std::process::id());
        assert_eq!(wait_any(&[auth.handle()], Some(Duration::from_secs(1))), Some(0));
        assert!(auth.is_set());
        auth.reset();
        assert!(!auth.is_set());
        // nobody to tell: no error, no wait
        signal_authenticated(u32::MAX);
    }

    #[test]
    fn this_process_has_no_tcp_connections_of_note() {
        let states = tcp_states(std::process::id());
        assert!(!states.contains(&TCP_ESTABLISHED), "{states:?}");
    }
}
