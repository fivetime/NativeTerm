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
    pub fn name_lossy(&self) -> String {
        String::from_utf8_lossy(&self.name).into_owned()
    }
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
    /// only); with it, `SSH_ASKPASS` is that program, forced.
    pub fn connect(ssh: &Path, config: Option<&Path>, alias: &str, askpass: Option<&Path>) -> Result<Session> {
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
            Some(program) => {
                command.env("SSH_ASKPASS", program).env("SSH_ASKPASS_REQUIRE", "force");
                command.arg("-o").arg("NumberOfPasswordPrompts=1");
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
        Session::spawn(command)
    }

    /// Runs `command` (an SFTP server on its stdin and stdout, e.g. `ssh
    /// -s … sftp` or a local `sftp-server`) and starts the session.
    pub fn spawn(mut command: Command) -> Result<Session> {
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| Error::Io(e.to_string()))?;
        let stdin: ChildStdin = child.stdin.take().ok_or_else(|| Error::Io("no stdin".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| Error::Io("no stdout".into()))?;
        // ssh's messages: why it couldn't connect, if it can't
        let stderr_text = Arc::new(Mutex::new(String::new()));
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
                        t.push_str(&String::from_utf8_lossy(&buf[..n]));
                    }
                }
            });
        }
        let last_words = move || {
            // give the stderr thread a moment to catch ssh's last line
            std::thread::sleep(std::time::Duration::from_millis(100));
            let t = lock(&stderr_text).trim().to_string();
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
        let sftp = Session::connect(ssh, Some(Path::new(config)), alias, None).unwrap();
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
        let wrong = Session::connect(ssh, Some(Path::new(config)), "no-such-host.invalid", None);
        println!("unknown host: {}", wrong.err().unwrap());
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
