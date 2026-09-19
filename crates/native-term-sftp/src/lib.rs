//! NativeTerm's SFTP client: SFTP version 3 over `ssh -s <alias> sftp`
//! (Windows' own OpenSSH), so a host's config, keys, agent, jump hosts and
//! known_hosts work as in its terminal tab, and nothing has to be
//! installed on the server (the SFTP subsystem comes with OpenSSH's
//! server). See ARCHITECTURE.md, "File transfer (SFTP)".
//!
//! A `Session` can be used from several threads at once: requests are
//! written under a lock and a reader thread hands each reply to the
//! request with its id, so a directory can be listed while a transfer
//! runs. Transfers keep several requests in flight (like OpenSSH's sftp:
//! 64 × 32 KB), which is what makes them fast over a long round trip.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

pub mod transfer;
pub mod wire;

use wire::{Attrs, Body, Fields};

/// Bytes per read or write request.
pub const CHUNK: u32 = 32 * 1024;
/// Requests in flight during a transfer.
pub const WINDOW: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The server's answer (`SSH_FX_*`) and its message.
    Status { code: u32, message: String },
    /// The connection is gone; ssh's last words, if any.
    Closed(String),
    /// A local file or the stream.
    Io(String),
    /// Something the server shouldn't have sent.
    Protocol(String),
    Cancelled,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Status { code, message } if message.is_empty() => write!(f, "SFTP error {code}"),
            Error::Status { message, .. } => f.write_str(message),
            Error::Closed(why) => f.write_str(why),
            Error::Io(e) | Error::Protocol(e) => f.write_str(e),
            Error::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e.to_string())
    }
}

impl Error {
    pub fn is_status(&self, code: u32) -> bool {
        matches!(self, Error::Status { code: c, .. } if *c == code)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A reply to one request.
#[derive(Debug)]
enum Reply {
    Status { code: u32, message: String },
    Handle(Vec<u8>),
    Data(Vec<u8>),
    Name(Vec<Entry>),
    Attrs(Attrs),
    /// An extension's own reply (none used yet).
    Extended,
}

/// A directory entry (or `realpath`'s answer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// As the server sent it (bytes; UTF-8 in practice).
    pub name: Vec<u8>,
    /// `ls -l`'s line for it.
    pub long_name: Vec<u8>,
    pub attrs: Attrs,
}

impl Entry {
    /// The name for showing, the automatic way (see `Names`).
    pub fn name_lossy(&self) -> String {
        Names::default().decode(&self.name)
    }
}

/// How a server's file names become text and back. SFTP v3 names are
/// bytes in whatever encoding the files were named in: UTF-8 today, GBK,
/// Big5, Shift_JIS, EUC-KR, a Windows or ISO-8859 code page on older
/// systems. Requests always use the bytes as listed, so every file can be
/// opened, renamed or deleted whatever its name; this only decides how a
/// name is shown and how a new one (upload, rename, new folder) is
/// written. The encodings are `encoding_rs`'s (the WHATWG Encoding
/// Standard, Firefox's): every one a name is likely to be in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Names {
    /// UTF-8 where a name is valid UTF-8, else `fallback` (the system's
    /// ANSI code page by default); new names in UTF-8.
    Auto { fallback: &'static Encoding },
    /// Every name in this encoding.
    Fixed(&'static Encoding),
}

pub use encoding_rs::Encoding;

impl Default for Names {
    fn default() -> Names {
        #[cfg(windows)]
        let fallback = codepage::to_encoding(native_term_win::ansi_code_page() as u16).unwrap_or(encoding_rs::WINDOWS_1252);
        #[cfg(not(windows))]
        let fallback = encoding_rs::WINDOWS_1252;
        Names::Auto { fallback }
    }
}

impl Names {
    /// An encoding by one of its names (`gbk`, `big5`, `shift_jis`,
    /// `euc-kr`, `windows-1251`, `iso-8859-2`, …) or a Windows code page
    /// number (`936`); `auto` or empty: `Names::default()`. Encodings that
    /// can't be written back (UTF-16, the replacement encoding) aren't
    /// offered.
    pub fn from_label(label: &str) -> Option<Names> {
        let label = label.trim();
        if label.is_empty() || label.eq_ignore_ascii_case("auto") {
            return Some(Names::default());
        }
        let encoding = match label.parse::<u16>() {
            Ok(cp) => codepage::to_encoding(cp)?,
            Err(_) => Encoding::for_label(label.as_bytes())?,
        };
        (encoding.output_encoding() == encoding).then_some(Names::Fixed(encoding))
    }

    /// The setting's value: `auto`, or the encoding's name.
    pub fn label(&self) -> &'static str {
        match self {
            Names::Auto { .. } => "auto",
            Names::Fixed(e) => e.name(),
        }
    }

    /// A name as text; bytes that are valid in neither encoding show as
    /// `\xNN`, so no name is ever hidden.
    pub fn decode(&self, bytes: &[u8]) -> String {
        let decoded = match *self {
            Names::Auto { fallback } => std::str::from_utf8(bytes).ok().map(str::to_string).or_else(|| strict(fallback, bytes)),
            Names::Fixed(encoding) => strict(encoding, bytes),
        };
        decoded.unwrap_or_else(|| escaped(bytes))
    }

    /// A new name's bytes; `None` if a character can't be written in the
    /// chosen encoding (never a substitute).
    pub fn encode(&self, text: &str) -> Option<Vec<u8>> {
        match *self {
            Names::Auto { .. } => Some(text.as_bytes().to_vec()),
            Names::Fixed(encoding) => {
                let (bytes, _, unmappable) = encoding.encode(text);
                (!unmappable).then(|| bytes.into_owned())
            }
        }
    }
}

fn strict(encoding: &'static Encoding, bytes: &[u8]) -> Option<String> {
    encoding.decode_without_bom_handling_and_without_replacement(bytes).map(|t| t.into_owned())
}

/// UTF-8 where it is, `\xNN` for the other bytes.
fn escaped(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
        for b in chunk.invalid() {
            out.push_str(&format!("\\x{b:02X}"));
        }
    }
    out
}

/// ssh's stderr as text (its `\ooo` escapes undone).
fn ssh_text(bytes: &[u8]) -> String {
    #[cfg(windows)]
    return native_term_win::ssh_message(bytes);
    #[cfg(not(windows))]
    return String::from_utf8_lossy(bytes).into_owned();
}

/// ssh's prompts (password, passphrase, host key, codes) go to `program`
/// (`SSH_ASKPASS`, forced), with `env` added for it.
pub struct Askpass {
    pub program: std::path::PathBuf,
    pub env: Vec<(String, String)>,
    /// `NumberOfPasswordPrompts` (1 with a saved password: no retries).
    pub password_prompts: Option<u32>,
}

type Waiters = HashMap<u32, Sender<Result<Reply>>>;

struct Shared {
    waiting: Waiters,
    /// Set when the stream ended: every later request fails with it.
    closed: Option<String>,
}

pub struct Session {
    writer: Mutex<Box<dyn Write + Send>>,
    next_id: AtomicU32,
    shared: Arc<Mutex<Shared>>,
    child: Mutex<Option<Child>>,
    /// Extensions the server named in its VERSION packet.
    pub extensions: Vec<(String, Vec<u8>)>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Session {
    /// SFTP to `alias` through ssh: `ssh [-F config] <options> -s -- alias
    /// sftp`. Without `askpass` ssh runs in batch mode (keys or the agent
    /// only); with it, every prompt goes to that helper (see `Askpass`).
    /// `on_spawn` gets ssh's process id before it asks anything.
    pub fn connect(
        ssh: &Path,
        config: Option<&Path>,
        alias: &str,
        askpass: Option<&Askpass>,
        on_spawn: impl FnOnce(u32),
    ) -> Result<Session> {
        if alias.is_empty() || alias.starts_with('-') {
            return Err(Error::Protocol(format!("invalid host {alias:?}")));
        }
        let mut command = Command::new(ssh);
        if let Some(config) = config {
            command.arg("-F").arg(config);
        }
        // no terminal, forwards, local command or remote command of the
        // host's (a RemoteCommand next to -s makes ssh refuse)
        for option in [
            "RequestTTY=no",
            "ClearAllForwardings=yes",
            "PermitLocalCommand=no",
            "RemoteCommand=none",
            "ForwardAgent=no",
            "ForwardX11=no",
            "ConnectTimeout=15",
            "ServerAliveInterval=15",
        ] {
            command.arg("-o").arg(option);
        }
        match askpass {
            Some(a) => {
                command.env("SSH_ASKPASS", &a.program).env("SSH_ASKPASS_REQUIRE", "force");
                for (key, value) in &a.env {
                    command.env(key, value);
                }
                if let Some(n) = a.password_prompts {
                    command.arg("-o").arg(format!("NumberOfPasswordPrompts={n}"));
                }
            }
            None => {
                command.arg("-o").arg("BatchMode=yes");
            }
        }
        command.arg("-s").arg("--").arg(alias).arg("sftp");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // no console window
        }
        Session::spawn_then(command, on_spawn)
    }

    /// Runs `command` (an SFTP server on its stdin and stdout, e.g. `ssh
    /// -s … sftp` or a local `sftp-server`) and starts the session.
    pub fn spawn(command: Command) -> Result<Session> {
        Session::spawn_then(command, |_| {})
    }

    /// `spawn`, telling `on_spawn` the process id first.
    pub fn spawn_then(mut command: Command, on_spawn: impl FnOnce(u32)) -> Result<Session> {
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| Error::Io(e.to_string()))?;
        on_spawn(child.id());
        let stdin: ChildStdin = child.stdin.take().ok_or_else(|| Error::Io("no stdin".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| Error::Io("no stdout".into()))?;
        // ssh's messages: why it couldn't connect, if it can't
        let stderr_text = Arc::new(Mutex::new(Vec::<u8>::new()));
        if let Some(mut stderr) = child.stderr.take() {
            let text = Arc::clone(&stderr_text);
            std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                while let Ok(n) = stderr.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let mut t = lock(&text);
                    if t.len() < 8192 {
                        t.extend_from_slice(&buf[..n]);
                    }
                }
            });
        }
        let last_words = move || {
            // give the stderr thread a moment to catch ssh's last line
            std::thread::sleep(std::time::Duration::from_millis(100));
            let t = ssh_text(&lock(&stderr_text)).trim().to_string();
            if t.is_empty() { "the connection closed".to_string() } else { t }
        };
        let mut session = Session::start(Box::new(stdin), Box::new(stdout), last_words)?;
        session.child = Mutex::new(Some(child));
        Ok(session)
    }

    /// The session over any stream (tests).
    pub fn start(
        mut writer: Box<dyn Write + Send>,
        mut reader: Box<dyn Read + Send>,
        last_words: impl Fn() -> String + Send + 'static,
    ) -> Result<Session> {
        writer.write_all(&wire::packet(wire::INIT, &Body::default().u32(3).0))?;
        writer.flush()?;
        let (kind, body) = wire::read_packet(&mut reader).map_err(|_| Error::Closed(last_words()))?;
        if kind != wire::VERSION {
            return Err(Error::Protocol(format!("expected VERSION, got packet type {kind}")));
        }
        let mut f = Fields::new(&body);
        let version = f.u32()?;
        if version < 3 {
            return Err(Error::Protocol(format!("SFTP version {version} isn't supported")));
        }
        let mut extensions = Vec::new();
        while !f.rest().is_empty() {
            let name = String::from_utf8_lossy(&f.string()?).into_owned();
            extensions.push((name, f.string()?));
        }
        let shared = Arc::new(Mutex::new(Shared { waiting: HashMap::new(), closed: None }));
        let reading = Arc::clone(&shared);
        std::thread::spawn(move || {
            let why = loop {
                let (kind, body) = match wire::read_packet(&mut reader) {
                    Ok(p) => p,
                    Err(_) => break last_words(),
                };
                let (id, reply) = match parse_reply(kind, &body) {
                    Ok(r) => r,
                    Err(e) => break format!("a broken SFTP reply: {e}"),
                };
                if let Some(tx) = lock(&reading).waiting.remove(&id) {
                    let _ = tx.send(Ok(reply));
                }
            };
            let mut s = lock(&reading);
            for (_, tx) in s.waiting.drain() {
                let _ = tx.send(Err(Error::Closed(why.clone())));
            }
            s.closed = Some(why);
        });
        Ok(Session { writer: Mutex::new(writer), next_id: AtomicU32::new(1), shared, child: Mutex::new(None), extensions })
    }

    /// Ends the connection now: ssh is killed, everything waiting fails
    /// with `Closed`.
    pub fn disconnect(&self) {
        if let Some(child) = lock(&self.child).as_mut() {
            let _ = child.kill();
        }
    }

    pub fn has_extension(&self, name: &str) -> bool {
        self.extensions.iter().any(|(n, _)| n == name)
    }

    /// Why the session ended, once it has.
    pub fn closed(&self) -> Option<String> {
        lock(&self.shared).closed.clone()
    }

    /// Sends a request; its reply arrives on the receiver.
    fn send(&self, kind: u8, body: Body) -> Receiver<Result<Reply>> {
        let (tx, rx) = mpsc::channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut s = lock(&self.shared);
            if let Some(why) = &s.closed {
                let _ = tx.send(Err(Error::Closed(why.clone())));
                return rx;
            }
            s.waiting.insert(id, tx.clone());
        }
        let mut payload = Body::default().u32(id);
        payload.0.extend_from_slice(&body.0);
        let written = {
            let mut w = lock(&self.writer);
            w.write_all(&wire::packet(kind, &payload.0)).and_then(|()| w.flush())
        };
        if let Err(e) = written {
            if let Some(tx) = lock(&self.shared).waiting.remove(&id) {
                let _ = tx.send(Err(Error::Closed(e.to_string())));
            }
        }
        rx
    }

    fn call(&self, kind: u8, body: Body) -> Result<Reply> {
        wait(self.send(kind, body))
    }

    fn ok(&self, kind: u8, body: Body) -> Result<()> {
        match self.call(kind, body)? {
            Reply::Status { code: wire::FX_OK, .. } => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    fn handle(&self, kind: u8, body: Body) -> Result<Vec<u8>> {
        match self.call(kind, body)? {
            Reply::Handle(h) => Ok(h),
            other => Err(unexpected(other)),
        }
    }

    fn attrs(&self, kind: u8, path: &[u8]) -> Result<Attrs> {
        match self.call(kind, Body::default().string(path))? {
            Reply::Attrs(a) => Ok(a),
            other => Err(unexpected(other)),
        }
    }

    /// The absolute form of `path` (`.` is the home directory).
    pub fn realpath(&self, path: &[u8]) -> Result<Vec<u8>> {
        match self.call(wire::REALPATH, Body::default().string(path))? {
            Reply::Name(mut names) if !names.is_empty() => Ok(names.remove(0).name),
            other => Err(unexpected(other)),
        }
    }

    /// Follows links.
    pub fn stat(&self, path: &[u8]) -> Result<Attrs> {
        self.attrs(wire::STAT, path)
    }

    pub fn lstat(&self, path: &[u8]) -> Result<Attrs> {
        self.attrs(wire::LSTAT, path)
    }

    pub fn readlink(&self, path: &[u8]) -> Result<Vec<u8>> {
        match self.call(wire::READLINK, Body::default().string(path))? {
            Reply::Name(mut names) if !names.is_empty() => Ok(names.remove(0).name),
            other => Err(unexpected(other)),
        }
    }

    /// A directory's entries, without `.` and `..`, in the server's order.
    pub fn read_dir(&self, path: &[u8]) -> Result<Vec<Entry>> {
        let handle = self.handle(wire::OPENDIR, Body::default().string(path))?;
        let mut entries = Vec::new();
        let result = loop {
            match self.call(wire::READDIR, Body::default().string(&handle)) {
                Ok(Reply::Name(names)) => entries.extend(names.into_iter().filter(|e| e.name != b"." && e.name != b"..")),
                Ok(Reply::Status { code: wire::FX_EOF, .. }) => break Ok(()),
                Ok(other) => break Err(unexpected(other)),
                Err(e) => break Err(e),
            }
        };
        let _ = self.close(&handle);
        result.map(|()| entries)
    }

    pub fn close(&self, handle: &[u8]) -> Result<()> {
        self.ok(wire::CLOSE, Body::default().string(handle))
    }

    pub fn mkdir(&self, path: &[u8]) -> Result<()> {
        self.ok(wire::MKDIR, Body::default().string(path).attrs(&Attrs::default()))
    }

    pub fn rmdir(&self, path: &[u8]) -> Result<()> {
        self.ok(wire::RMDIR, Body::default().string(path))
    }

    pub fn remove(&self, path: &[u8]) -> Result<()> {
        self.ok(wire::REMOVE, Body::default().string(path))
    }

    pub fn setstat(&self, path: &[u8], attrs: &Attrs) -> Result<()> {
        self.ok(wire::SETSTAT, Body::default().string(path).attrs(attrs))
    }

    /// Renames; with `replace`, an existing `to` is replaced in one step
    /// (OpenSSH's `posix-rename@openssh.com`; plain SFTP v3 refuses to
    /// overwrite).
    pub fn rename(&self, from: &[u8], to: &[u8], replace: bool) -> Result<()> {
        if replace && self.has_extension("posix-rename@openssh.com") {
            let body = Body::default().string(b"posix-rename@openssh.com").string(from).string(to);
            return self.ok(wire::EXTENDED, body);
        }
        self.ok(wire::RENAME, Body::default().string(from).string(to))
    }

    fn open(&self, path: &[u8], flags: u32, attrs: &Attrs) -> Result<Vec<u8>> {
        self.handle(wire::OPEN, Body::default().string(path).u32(flags).attrs(attrs))
    }

    /// Downloads `remote` into `local` (created or truncated). `progress`
    /// gets the bytes done so far and returns false to cancel. Several
    /// reads are in flight; each chunk is written where it belongs, so
    /// replies may come in any order and a short read is asked again.
    pub fn download(&self, remote: &[u8], local: &Path, progress: &mut dyn FnMut(u64) -> bool) -> Result<u64> {
        let handle = self.open(remote, wire::OPEN_READ, &Attrs::default())?;
        let result = File::create(local).map_err(Error::from).and_then(|file| self.read_into(&handle, &file, progress));
        let closed = self.close(&handle);
        let done = result?;
        closed?;
        Ok(done)
    }

    /// Reads until the server says EOF, `WINDOW` reads in flight; the
    /// file ends where the last byte read ends (it may have changed size
    /// since `stat`, which is only a hint here).
    fn read_into(&self, handle: &[u8], file: &File, progress: &mut dyn FnMut(u64) -> bool) -> Result<u64> {
        // offset → (length asked, reply), oldest first
        let mut in_flight: BTreeMap<u64, (u32, Receiver<Result<Reply>>)> = BTreeMap::new();
        let mut next = 0u64;
        let mut eof_at: Option<u64> = None;
        let mut done = 0u64;
        let mut end = 0u64;
        let read = |offset: u64, len: u32| self.send(wire::READ, Body::default().string(handle).u64(offset).u32(len));
        loop {
            while eof_at.is_none() && in_flight.len() < WINDOW {
                in_flight.insert(next, (CHUNK, read(next, CHUNK)));
                next += CHUNK as u64;
            }
            let Some(offset) = in_flight.keys().next().copied() else { break };
            let (asked, rx) = in_flight.remove(&offset).expect("present");
            let reply = match wait(rx) {
                Ok(r) => r,
                Err(e) => {
                    drain(in_flight.into_values().map(|(_, rx)| rx));
                    return Err(e);
                }
            };
            match reply {
                Reply::Data(data) if !data.is_empty() => {
                    write_at(file, offset, &data)?;
                    let got = data.len() as u32;
                    done += got as u64;
                    end = end.max(offset + got as u64);
                    if got < asked {
                        // a short read: the rest of this chunk again
                        let rest = offset + got as u64;
                        in_flight.insert(rest, (asked - got, read(rest, asked - got)));
                    }
                }
                Reply::Data(_) | Reply::Status { code: wire::FX_EOF, .. } => {
                    eof_at = Some(eof_at.map_or(offset, |e| e.min(offset)));
                }
                other => {
                    drain(in_flight.into_values().map(|(_, rx)| rx));
                    return Err(unexpected(other));
                }
            }
            // reads at or past the end aren't needed
            if let Some(eof) = eof_at {
                let beyond: Vec<u64> = in_flight.range(eof..).map(|(o, _)| *o).collect();
                drain(beyond.into_iter().filter_map(|o| in_flight.remove(&o)).map(|(_, rx)| rx));
            }
            if !progress(done) {
                drain(in_flight.into_values().map(|(_, rx)| rx));
                return Err(Error::Cancelled);
            }
        }
        file.set_len(end)?;
        Ok(done)
    }

    /// Uploads `local` to `remote` (created or truncated, with
    /// `permissions` if new). Several writes are in flight.
    pub fn upload(&self, local: &Path, remote: &[u8], permissions: Option<u32>, progress: &mut dyn FnMut(u64) -> bool) -> Result<u64> {
        let mut file = File::open(local)?;
        let attrs = Attrs { permissions, ..Default::default() };
        let handle = self.open(remote, wire::OPEN_WRITE | wire::OPEN_CREAT | wire::OPEN_TRUNC, &attrs)?;
        let result = self.write_from(&handle, &mut file, progress);
        let closed = self.close(&handle);
        let done = result?;
        closed?;
        Ok(done)
    }

    fn write_from(&self, handle: &[u8], file: &mut File, progress: &mut dyn FnMut(u64) -> bool) -> Result<u64> {
        let mut in_flight = std::collections::VecDeque::new();
        let mut offset = 0u64;
        let mut done = 0u64;
        let mut buf = vec![0u8; CHUNK as usize];
        let mut end = false;
        loop {
            while !end && in_flight.len() < WINDOW {
                let n = read_full(file, &mut buf)?;
                if n == 0 {
                    end = true;
                    break;
                }
                let rx = self.send(wire::WRITE, Body::default().string(handle).u64(offset).string(&buf[..n]));
                in_flight.push_back((n as u64, rx));
                offset += n as u64;
            }
            let Some((n, rx)) = in_flight.pop_front() else { break };
            match wait(rx)? {
                Reply::Status { code: wire::FX_OK, .. } => done += n,
                other => {
                    drain(in_flight.into_iter().map(|(_, rx)| rx));
                    return Err(unexpected(other));
                }
            }
            if !progress(done) {
                drain(in_flight.into_iter().map(|(_, rx)| rx));
                return Err(Error::Cancelled);
            }
        }
        Ok(done)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // closing stdin ends sftp and ssh; kill if it lingers
        if let Some(mut child) = lock(&self.child).take() {
            drop(child.stdin.take());
            std::thread::spawn(move || {
                for _ in 0..20 {
                    if let Ok(Some(_)) = child.try_wait() {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                let _ = child.kill();
                let _ = child.wait();
            });
        }
    }
}

fn wait(rx: Receiver<Result<Reply>>) -> Result<Reply> {
    match rx.recv() {
        Ok(Ok(Reply::Status { code, message })) if code != wire::FX_OK && code != wire::FX_EOF => {
            Err(Error::Status { code, message })
        }
        Ok(r) => r,
        Err(_) => Err(Error::Closed("the connection closed".into())),
    }
}

/// Waits for replies nobody needs any more (so they don't pile up).
fn drain(replies: impl Iterator<Item = Receiver<Result<Reply>>>) {
    for rx in replies {
        let _ = rx.recv();
    }
}

fn unexpected(reply: Reply) -> Error {
    match reply {
        Reply::Status { code, message } => Error::Status { code, message },
        other => Error::Protocol(format!("unexpected reply {other:?}")),
    }
}

fn parse_reply(kind: u8, body: &[u8]) -> io::Result<(u32, Reply)> {
    let mut f = Fields::new(body);
    let id = f.u32()?;
    let reply = match kind {
        wire::STATUS => {
            let code = f.u32()?;
            let message = if f.rest().is_empty() { Vec::new() } else { f.string()? };
            Reply::Status { code, message: String::from_utf8_lossy(&message).into_owned() }
        }
        wire::HANDLE => Reply::Handle(f.string()?),
        wire::DATA => Reply::Data(f.string()?),
        wire::NAME => {
            let count = f.u32()?;
            let mut names = Vec::with_capacity(count.min(1024) as usize);
            for _ in 0..count {
                names.push(Entry { name: f.string()?, long_name: f.string()?, attrs: f.attrs()? });
            }
            Reply::Name(names)
        }
        wire::ATTRS => Reply::Attrs(f.attrs()?),
        wire::EXTENDED_REPLY => Reply::Extended,
        other => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("packet type {other}"))),
    };
    Ok((id, reply))
}

fn read_full(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match file.read(&mut buf[n..])? {
            0 => break,
            m => n += m,
        }
    }
    Ok(n)
}

#[cfg(windows)]
fn write_at(file: &File, offset: u64, data: &[u8]) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut written = 0;
    while written < data.len() {
        written += file.seek_write(&data[written..], offset + written as u64)?;
    }
    Ok(())
}

#[cfg(unix)]
fn write_at(file: &File, offset: u64, data: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(data, offset)
}

/// Joins a directory and a name the way the server writes paths.
pub fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut out = dir.to_vec();
    if !out.ends_with(b"/") {
        out.push(b'/');
    }
    out.extend_from_slice(name);
    out
}

/// The parent of an absolute server path (`/` stays `/`).
pub fn parent(path: &[u8]) -> Vec<u8> {
    let trimmed = path.strip_suffix(b"/").filter(|p| !p.is_empty()).unwrap_or(path);
    match trimmed.iter().rposition(|&c| c == b'/') {
        Some(0) | None => b"/".to_vec(),
        Some(i) => trimmed[..i].to_vec(),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Windows' own sftp-server over pipes: a real server, no ssh.
    fn local_server(dir: &Path) -> Option<Session> {
        let server = Path::new(r"C:\Windows\System32\OpenSSH\sftp-server.exe");
        if !server.exists() {
            eprintln!("no sftp-server.exe: skipped");
            return None;
        }
        let mut command = Command::new(server);
        command.arg("-d").arg(dir);
        Some(Session::spawn(command).unwrap())
    }

    /// The server's form of a local path (`/C:/…`).
    fn remote(path: &Path) -> Vec<u8> {
        format!("/{}", path.display().to_string().replace('\\', "/")).into_bytes()
    }

    #[test]
    fn names_in_any_encoding() {
        let gbk = [0xd6u8, 0xd0, 0xce, 0xc4, b'.', b't', b'x', b't'];
        let names = |label: &str| Names::from_label(label).unwrap();
        // chosen per host, by name or code page
        assert_eq!(names("gbk").decode(&gbk), "中文.txt");
        assert_eq!(names("936").decode(&gbk), "中文.txt");
        for (label, text) in [
            ("big5", "繁體.txt"),
            ("shift_jis", "日本語.txt"),
            ("euc-jp", "日本語.txt"),
            ("euc-kr", "한국어.txt"),
            ("windows-1251", "Привет.txt"),
            ("koi8-r", "Привет.txt"),
            ("iso-8859-2", "Łódź.txt"),
            ("windows-1256", "مرحبا.txt"),
            ("gb18030", "𠀀.txt"),
        ] {
            let bytes = names(label).encode(text).unwrap_or_else(|| panic!("{label}"));
            assert_ne!(bytes, text.as_bytes(), "{label}: not UTF-8");
            assert_eq!(names(label).decode(&bytes), text, "{label}");
        }
        // automatic: UTF-8 first, then the fallback
        let auto = Names::Auto { fallback: encoding_rs::GBK };
        assert_eq!(auto.decode("文件.txt".as_bytes()), "文件.txt");
        assert_eq!(auto.decode(&gbk), "中文.txt");
        // neither: every byte still shows
        assert_eq!(names("utf-8").decode(&gbk), "\\xD6\\xD0\\xCE\\xC4.txt");
        assert_eq!(names("shift_jis").decode(&[0x81]), "\\x81", "a lead byte alone");
        // new names: strict
        assert_eq!(names("gbk").encode("中文.txt").unwrap(), gbk);
        assert_eq!(names("windows-1251").encode("中文"), None, "can't be written there");
        assert_eq!(names("windows-1252").encode("ā"), None, "no look-alike 'a'");
        assert_eq!(auto.encode("中文").unwrap(), "中文".as_bytes());
        // what isn't offered
        assert_eq!(Names::from_label("utf-16le"), None);
        assert_eq!(Names::from_label("no-such"), None);
        assert_eq!(names("GBK").label(), "GBK");
        assert_eq!(names("auto").label(), "auto");
    }

    #[test]
    fn paths() {
        assert_eq!(join(b"/home/a", b"x"), b"/home/a/x");
        assert_eq!(join(b"/", b"x"), b"/x");
        assert_eq!(parent(b"/home/a/"), b"/home");
        assert_eq!(parent(b"/home"), b"/");
        assert_eq!(parent(b"/"), b"/");
    }

    #[test]
    fn a_real_server_lists_and_transfers() {
        let dir = tempfile::tempdir().unwrap();
        let Some(sftp) = local_server(dir.path()) else { return };
        let base = remote(dir.path());

        // a file with a size that isn't a multiple of the chunk, and a CJK name
        let data: Vec<u8> = (0..(CHUNK as usize * 70 + 1234)).map(|i| (i * 7 % 251) as u8).collect();
        let local = dir.path().join("源 文件.bin");
        std::fs::write(&local, &data).unwrap();
        sftp.mkdir(&join(&base, "上传".as_bytes())).unwrap();
        let target = join(&join(&base, "上传".as_bytes()), "副本.bin".as_bytes());
        let mut calls = 0;
        let sent = sftp.upload(&local, &target, Some(0o644), &mut |_| { calls += 1; true }).unwrap();
        assert_eq!(sent, data.len() as u64);
        assert!(calls > 1);
        assert_eq!(std::fs::read(dir.path().join("上传").join("副本.bin")).unwrap(), data);

        let entries = sftp.read_dir(&join(&base, "上传".as_bytes())).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name_lossy(), "副本.bin");
        assert_eq!(entries[0].attrs.size, Some(data.len() as u64));

        let back = dir.path().join("back.bin");
        let got = sftp.download(&target, &back, &mut |_| true).unwrap();
        assert_eq!(got, data.len() as u64);
        assert_eq!(std::fs::read(&back).unwrap(), data);

        // an empty file both ways
        let empty = dir.path().join("empty");
        std::fs::write(&empty, b"").unwrap();
        sftp.upload(&empty, &join(&base, b"e"), None, &mut |_| true).unwrap();
        assert_eq!(sftp.download(&join(&base, b"e"), &dir.path().join("e2"), &mut |_| true).unwrap(), 0);

        // cancel halfway
        let cancelled = sftp.download(&target, &back, &mut |done| done < CHUNK as u64 * 4);
        assert_eq!(cancelled, Err(Error::Cancelled));
        // and the session still works
        assert!(sftp.stat(&target).unwrap().size.is_some());

        sftp.rename(&target, &join(&base, b"moved.bin"), false).unwrap();
        assert!(sftp.stat(&target).unwrap_err().is_status(wire::FX_NO_SUCH_FILE));
        sftp.remove(&join(&base, b"moved.bin")).unwrap();
        sftp.rmdir(&join(&base, "上传".as_bytes())).unwrap();
        assert!(!dir.path().join("上传").exists());
    }

    #[test]
    fn listing_during_a_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let Some(sftp) = local_server(dir.path()) else { return };
        let sftp = Arc::new(sftp);
        let base = remote(dir.path());
        let big = dir.path().join("big.bin");
        std::fs::write(&big, vec![7u8; 8 * 1024 * 1024]).unwrap();
        let (s, b) = (Arc::clone(&sftp), base.clone());
        let upload = std::thread::spawn(move || s.upload(&big, &join(&b, b"copy.bin"), None, &mut |_| true));
        // meanwhile the directory can be listed
        for _ in 0..5 {
            assert!(sftp.read_dir(&base).unwrap().iter().any(|e| e.name == b"big.bin"));
        }
        assert_eq!(upload.join().unwrap().unwrap(), 8 * 1024 * 1024);
    }

    /// Through ssh to a real server: `NATIVETERM_SFTP_TEST=<ssh config>|<alias>`.
    #[test]
    #[ignore = "needs a server"]
    fn through_ssh() {
        let spec = std::env::var("NATIVETERM_SFTP_TEST").unwrap();
        let (config, alias) = spec.split_once('|').unwrap();
        let ssh = Path::new(r"C:\Windows\System32\OpenSSH\ssh.exe");
        let start = std::time::Instant::now();
        let sftp = Session::connect(ssh, Some(Path::new(config)), alias, None, |_| {}).unwrap();
        let home = sftp.realpath(b".").unwrap();
        println!("connected in {:?}, home {}", start.elapsed(), String::from_utf8_lossy(&home));
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("big.bin");
        let data: Vec<u8> = (0..32 * 1024 * 1024).map(|i: usize| (i * 13 % 251) as u8).collect();
        std::fs::write(&local, &data).unwrap();
        let target = join(&home, "nt-sftp-测试.bin".as_bytes());
        let t = std::time::Instant::now();
        sftp.upload(&local, &target, Some(0o600), &mut |_| true).unwrap();
        let up = t.elapsed();
        let back = dir.path().join("back.bin");
        let t = std::time::Instant::now();
        sftp.download(&target, &back, &mut |_| true).unwrap();
        let down = t.elapsed();
        assert_eq!(std::fs::read(&back).unwrap(), data);
        let mb = data.len() as f64 / 1048576.0;
        println!("32 MB up {:.1} MB/s, down {:.1} MB/s", mb / up.as_secs_f64(), mb / down.as_secs_f64());
        sftp.remove(&target).unwrap();

        // prepared on the server: `中文.txt` named in GBK, and a link to /tmp
        let gbk_name = [0xd6u8, 0xd0, 0xce, 0xc4, b'.', b't', b'x', b't'];
        let entries = sftp.read_dir(&home).unwrap();
        let gbk = entries.iter().find(|e| e.name == gbk_name).expect("the GBK-named file is listed with its bytes");
        println!("GBK name listed: {:?} ({} bytes)", gbk.name_lossy(), gbk.attrs.size.unwrap_or(0));
        let local = dir.path().join("gbk.txt");
        sftp.download(&join(&home, &gbk_name), &local, &mut |_| true).unwrap();
        assert_eq!(std::fs::read(&local).unwrap(), b"gbk content");
        let renamed = join(&home, b"renamed-gbk.txt");
        sftp.rename(&join(&home, &gbk_name), &renamed, false).unwrap();
        sftp.rename(&renamed, &join(&home, &gbk_name), false).unwrap();
        let link = entries.iter().find(|e| e.name == b"link-to-tmp").expect("the link");
        assert!(link.attrs.is_symlink(), "listed as a link: {:?}", link.attrs);
        assert!(sftp.stat(&join(&home, b"link-to-tmp")).unwrap().is_dir(), "stat follows it to a directory");
        assert_eq!(sftp.readlink(&join(&home, b"link-to-tmp")).unwrap(), b"/tmp");

        let wrong = Session::connect(ssh, Some(Path::new(config)), "no-such-host.invalid", None, |_| {});
        println!("unknown host: {}", wrong.err().unwrap());
    }

    /// A connection lost halfway: the transfer fails (no hang), and so does
    /// everything after it.
    #[test]
    fn a_lost_connection_fails_everything_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let Some(sftp) = local_server(dir.path()) else { return };
        let sftp = Arc::new(sftp);
        let base = remote(dir.path());
        let big = dir.path().join("big.bin");
        std::fs::write(&big, vec![1u8; 64 * 1024 * 1024]).unwrap();
        let s = Arc::clone(&sftp);
        let started = std::time::Instant::now();
        let mut killed = false;
        let result = s.upload(&big, &join(&base, b"copy.bin"), None, &mut |done| {
            if done > 4 * 1024 * 1024 && !killed {
                killed = true;
                sftp.disconnect();
            }
            true
        });
        assert!(matches!(result, Err(Error::Closed(_))), "{result:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        assert!(matches!(sftp.stat(&base), Err(Error::Closed(_))));
        assert!(sftp.closed().is_some());
    }

    #[test]
    fn errors_are_reported_and_nothing_is_left_open() {
        let dir = tempfile::tempdir().unwrap();
        let Some(sftp) = local_server(dir.path()) else { return };
        let base = remote(dir.path());
        std::fs::write(dir.path().join("a.txt"), b"old").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();

        // a missing file
        let e = sftp.download(&join(&base, b"missing"), &dir.path().join("x"), &mut |_| true).unwrap_err();
        assert!(e.is_status(wire::FX_NO_SUCH_FILE), "{e:?}");
        // a local file that can't be written: the remote handle is closed again
        let e = sftp.download(&join(&base, b"a.txt"), &dir.path().join("no").join("dir").join("x"), &mut |_| true).unwrap_err();
        assert!(matches!(e, Error::Io(_)), "{e:?}");
        // a directory isn't a file
        assert!(sftp.download(&join(&base, b"sub"), &dir.path().join("y"), &mut |_| true).is_err());
        assert!(sftp.rmdir(&join(&base, b"a.txt")).is_err());
        assert!(sftp.mkdir(&join(&base, b"sub")).is_err(), "exists");
        // an upload replaces an existing file's content
        let local = dir.path().join("new.txt");
        std::fs::write(&local, b"new and longer").unwrap();
        sftp.upload(&local, &join(&base, b"a.txt"), None, &mut |_| true).unwrap();
        assert_eq!(std::fs::read(dir.path().join("a.txt")).unwrap(), b"new and longer");
        // a rename onto an existing file: refused plainly, done with replace
        std::fs::write(dir.path().join("b.txt"), b"b").unwrap();
        assert!(sftp.rename(&join(&base, b"b.txt"), &join(&base, b"a.txt"), false).is_err());
        sftp.rename(&join(&base, b"b.txt"), &join(&base, b"a.txt"), true).unwrap();
        assert_eq!(std::fs::read(dir.path().join("a.txt")).unwrap(), b"b");
        // after all that, the directory can be removed: no handle was left open
        std::fs::remove_file(dir.path().join("a.txt")).unwrap();
        std::fs::remove_file(dir.path().join("new.txt")).unwrap();
        sftp.rmdir(&join(&base, b"sub")).unwrap();
    }

    #[test]
    fn a_large_directory() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3000 {
            std::fs::write(dir.path().join(format!("文件-{i:04}.txt")), b"").unwrap();
        }
        let Some(sftp) = local_server(dir.path()) else { return };
        let started = std::time::Instant::now();
        let entries = sftp.read_dir(&remote(dir.path())).unwrap();
        assert_eq!(entries.len(), 3000);
        assert!(entries.iter().all(|e| e.name_lossy().starts_with("文件-")));
        eprintln!("3000 entries in {:?}", started.elapsed());
    }

    #[test]
    fn a_connection_that_fails_says_why() {
        let command = Command::new(r"C:\Windows\System32\cmd.exe");
        let mut command = command;
        command.args(["/c", "echo ssh: Could not resolve hostname nowhere 1>&2"]);
        match Session::spawn(command) {
            Err(Error::Closed(why)) => assert!(why.contains("Could not resolve"), "{why}"),
            other => panic!("{:?}", other.map(|_| ())),
        }
    }
}
