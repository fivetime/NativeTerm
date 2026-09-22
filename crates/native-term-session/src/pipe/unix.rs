//! The Unix socket between NativeTerm and its shims.
//!
//! - Path: `$XDG_RUNTIME_DIR/nativeterm/app.sock` (the session's private
//!   tmpfs), else `$TMPDIR/nativeterm-<uid>/app.sock`; a path too long
//!   for `sun_path` falls back to `/tmp/nativeterm-<uid>/app.sock`. The
//!   folder is `0700`, the socket `0600`.
//! - One instance: `bind` first connects to the path; an answer means
//!   another NativeTerm (`AddrInUse`), a refusal means a socket file left
//!   behind, which is removed and bound again.
//! - Only this user: `accept` reads the peer's credentials (`SO_PEERCRED`
//!   on Linux, `getpeereid` and `LOCAL_PEERPID` on macOS) and drops
//!   anyone else's connection.
//! - A reader and a writer share one descriptor, each behind its own
//!   lock; `close` shuts the socket down, which wakes a blocked read.

use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{de::DeserializeOwned, Serialize};

use crate::protocol;

const READ_CHUNK: usize = 16 * 1024;
/// How long a socket path may be (`sun_path`), on the shorter systems.
const SUN_PATH_MAX: usize = 100;
const SOCKET_FILE: &str = "app.sock";

fn uid() -> u32 {
    // SAFETY: takes nothing, cannot fail.
    unsafe { libc::geteuid() }
}

/// This user's and session's socket path.
pub fn pipe_name() -> io::Result<String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_dir());
    let dir = match runtime {
        Some(runtime) => runtime.join("nativeterm"),
        None => std::env::temp_dir().join(format!("nativeterm-{}", uid())),
    };
    let mut path = dir.join(SOCKET_FILE);
    if path.as_os_str().len() > SUN_PATH_MAX {
        path = PathBuf::from("/tmp").join(format!("nativeterm-{}", uid())).join(SOCKET_FILE);
    }
    Ok(path.to_string_lossy().into_owned())
}

/// The socket's folder: made, and then ours alone. One that is already
/// there (`/tmp`, for a test's socket) is left as it is.
fn prepare_dir(path: &Path) -> io::Result<()> {
    let Some(dir) = path.parent() else { return Ok(()) };
    match std::fs::create_dir(dir) {
        Ok(()) => std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        }
        Err(e) => Err(e),
    }
}

fn gone(e: io::Error) -> io::Error {
    match e.raw_os_error() {
        Some(libc::EPIPE | libc::ECONNRESET | libc::ENOTCONN) => io::Error::new(io::ErrorKind::UnexpectedEof, e),
        _ => e,
    }
}

/// Who is at the other end of `stream`: their user and process ids.
fn peer(stream: &UnixStream) -> io::Result<(u32, u32)> {
    let fd = stream.as_raw_fd();
    #[cfg(target_os = "linux")]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: `cred` and `len` are ours and sized for each other; the
        // call fills `cred` for a connected Unix socket.
        let got = unsafe {
            libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len)
        };
        if got != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((cred.uid, cred.pid as u32))
    }
    #[cfg(target_os = "macos")]
    {
        let (mut uid, mut gid) = (0 as libc::uid_t, 0 as libc::gid_t);
        // SAFETY: two integers of ours for the call to fill.
        if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        // SAFETY: `pid` and `len` are ours and sized for each other;
        // LOCAL_PEERPID (2) at SOL_LOCAL (0) gives the peer's process id.
        let got = unsafe { libc::getsockopt(fd, 0, 2, (&mut pid as *mut libc::pid_t).cast(), &mut len) };
        if got != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((uid, pid as u32))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = fd;
        Err(io::Error::new(io::ErrorKind::Unsupported, "peer credentials"))
    }
}

/// Server side (NativeTerm).
pub struct PipeListener {
    listener: UnixListener,
    path: PathBuf,
}

impl PipeListener {
    /// Fails with `AddrInUse` if another process already serves `name`.
    pub fn bind(name: &str) -> io::Result<PipeListener> {
        let path = PathBuf::from(name);
        prepare_dir(&path)?;
        match UnixStream::connect(&path) {
            Ok(_) => return Err(io::Error::new(io::ErrorKind::AddrInUse, "another NativeTerm owns the socket")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                // a socket file left behind: nobody listens on it
                std::fs::remove_file(&path)?;
            }
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(PipeListener { listener, path })
    }

    /// Wait for the next client of this user's.
    pub fn accept(&mut self) -> io::Result<PipeConnection> {
        loop {
            let (stream, _) = self.listener.accept()?;
            match peer(&stream) {
                Ok((peer_uid, pid)) if peer_uid == uid() => return PipeConnection::new(stream, Some(pid)),
                // someone else's process, or nobody we can name: not served
                _ => drop(stream),
            }
        }
    }
}

impl Drop for PipeListener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Client side (shim): connect, waiting up to `timeout` for the server.
pub fn connect(name: &str, timeout: Duration) -> io::Result<PipeConnection> {
    let started = Instant::now();
    loop {
        match UnixStream::connect(name) {
            Ok(stream) => return PipeConnection::new(stream, None),
            Err(e) if started.elapsed() >= timeout => return Err(e),
            Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
}

/// One connection, usable from a reader and a writer thread at once.
pub struct PipeConnection {
    stream: UnixStream,
    peer_pid: Option<u32>,
    closed: AtomicBool,
    inbox: Mutex<Vec<u8>>,
    reader: Mutex<()>,
    writer: Mutex<()>,
}

impl PipeConnection {
    fn new(stream: UnixStream, peer_pid: Option<u32>) -> io::Result<PipeConnection> {
        Ok(PipeConnection {
            stream,
            peer_pid,
            closed: AtomicBool::new(false),
            inbox: Mutex::new(Vec::new()),
            reader: Mutex::new(()),
            writer: Mutex::new(()),
        })
    }

    /// Server side: the connected process.
    pub fn client_pid(&self) -> io::Result<u32> {
        match self.peer_pid {
            Some(pid) => Ok(pid),
            None => peer(&self.stream).map(|(_, pid)| pid),
        }
    }

    /// Stop using the connection: a waiting `recv` returns at once, and
    /// later calls fail. The socket closes when the last owner drops it.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    fn check_open(&self) -> io::Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            Err(io::Error::new(io::ErrorKind::ConnectionAborted, "connection closed"))
        } else {
            Ok(())
        }
    }

    pub fn send<T: Serialize>(&self, message: &T) -> io::Result<()> {
        self.check_open()?;
        let line = protocol::encode(message);
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        (&self.stream).write_all(line.as_bytes()).map_err(gone)
    }

    /// Next message, or `Ok(None)` after `timeout`. `UnexpectedEof` when
    /// the other side has gone.
    pub fn recv<T: DeserializeOwned>(&self, timeout: Duration) -> io::Result<Option<T>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(line) = self.take_line() {
                return protocol::decode(&line).map(Some);
            }
            self.check_open()?;
            let left = deadline.saturating_duration_since(Instant::now());
            match self.read_some(left) {
                Ok(Some(bytes)) => self.inbox.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&bytes),
                Ok(None) if Instant::now() >= deadline => return Ok(None),
                Ok(None) => {}
                // a read cut short by our own `close` is that, not the peer leaving
                Err(_) if self.closed.load(Ordering::SeqCst) => self.check_open()?,
                Err(e) => return Err(e),
            }
        }
    }

    /// Bytes that arrived within `timeout`; `None` if none did.
    fn read_some(&self, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        let _reader = self.reader.lock().unwrap_or_else(|e| e.into_inner());
        // a zero timeout would mean "wait forever"
        self.stream.set_read_timeout(Some(timeout.max(Duration::from_millis(1))))?;
        let mut buf = vec![0u8; READ_CHUNK];
        match (&self.stream).read(&mut buf) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "socket closed")),
            Ok(n) => {
                buf.truncate(n);
                Ok(Some(buf))
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => Ok(None),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(None),
            Err(e) => Err(gone(e)),
        }
    }

    fn take_line(&self) -> Option<String> {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        let end = inbox.iter().position(|&b| b == b'\n')?;
        let line: Vec<u8> = inbox.drain(..=end).collect();
        Some(String::from_utf8_lossy(&line).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AppMessage, Role, ShimMessage};
    use std::sync::Arc;

    fn test_name(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!("nativeterm-test-{}", std::process::id()));
        dir.join(format!("{tag}.sock")).to_string_lossy().into_owned()
    }

    #[test]
    fn name_is_per_user_and_ours_alone() {
        let name = pipe_name().unwrap();
        assert!(name.ends_with("/app.sock"), "{name}");
        assert!(name.len() <= SUN_PATH_MAX + 8, "{name}");
        let path = PathBuf::from(&name);
        assert!(path.is_absolute());
        let _first = PipeListener::bind(&name).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{name}");
        let dir_mode = std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
    }

    #[test]
    fn hello_welcome_and_commands() {
        let name = test_name("hello");
        let mut listener = PipeListener::bind(&name).unwrap();
        let server = std::thread::spawn(move || {
            let conn = listener.accept().unwrap();
            let pid = conn.client_pid().unwrap();
            let hello: ShimMessage = conn.recv(Duration::from_secs(5)).unwrap().unwrap();
            conn.send(&AppMessage::Welcome { protocol: 1 }).unwrap();
            conn.send(&AppMessage::SendText { text: "uptime".into(), enter: true }).unwrap();
            let exited: ShimMessage = conn.recv(Duration::from_secs(5)).unwrap().unwrap();
            (pid, hello, exited)
        });

        let client = connect(&name, Duration::from_secs(5)).unwrap();
        client
            .send(&ShimMessage::Hello {
                protocol: 1,
                role: Role::Shim,
                pid: std::process::id(),
                wt_session: None,
                session: Some("s".into()),
                alias: Some("web01".into()),
                terminal_window: None,
            })
            .unwrap();
        assert_eq!(
            client.recv::<AppMessage>(Duration::from_secs(5)).unwrap(),
            Some(AppMessage::Welcome { protocol: 1 })
        );
        assert_eq!(
            client.recv::<AppMessage>(Duration::from_secs(5)).unwrap(),
            Some(AppMessage::SendText { text: "uptime".into(), enter: true })
        );
        assert_eq!(client.recv::<AppMessage>(Duration::from_millis(50)).unwrap(), None, "timeout, no message");
        client.send(&ShimMessage::Exited { code: 255 }).unwrap();

        let (pid, hello, exited) = server.join().unwrap();
        assert_eq!(pid, std::process::id(), "client pid seen by the server");
        assert!(matches!(hello, ShimMessage::Hello { alias: Some(a), .. } if a == "web01"));
        assert_eq!(exited, ShimMessage::Exited { code: 255 });
    }

    #[test]
    fn client_that_already_closed_is_still_read() {
        // the LocalCommand helper connects, writes and exits at once
        let name = test_name("quick");
        let mut listener = PipeListener::bind(&name).unwrap();
        let client = connect(&name, Duration::from_secs(5)).unwrap();
        client.send(&ShimMessage::Authenticated).unwrap();
        drop(client);
        let conn = listener.accept().unwrap();
        assert_eq!(conn.recv::<ShimMessage>(Duration::from_secs(5)).unwrap(), Some(ShimMessage::Authenticated));
        assert_eq!(conn.recv::<ShimMessage>(Duration::from_secs(1)).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn message_sent_right_before_the_server_closes_arrives() {
        let name = test_name("sendclose");
        let mut listener = PipeListener::bind(&name).unwrap();
        let server = std::thread::spawn(move || {
            let conn = listener.accept().unwrap();
            conn.send(&AppMessage::Welcome { protocol: 1 }).unwrap();
            conn.send(&AppMessage::Close).unwrap();
            drop(conn);
            listener
        });
        let client = connect(&name, Duration::from_secs(5)).unwrap();
        let _listener = server.join().unwrap();
        assert_eq!(
            client.recv::<AppMessage>(Duration::from_secs(5)).unwrap(),
            Some(AppMessage::Welcome { protocol: 1 })
        );
        assert_eq!(client.recv::<AppMessage>(Duration::from_secs(5)).unwrap(), Some(AppMessage::Close));
        assert_eq!(client.recv::<AppMessage>(Duration::from_secs(1)).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn close_wakes_a_waiting_reader() {
        let name = test_name("closewait");
        let mut listener = PipeListener::bind(&name).unwrap();
        let server = std::thread::spawn(move || (listener.accept().unwrap(), listener));
        let client = Arc::new(connect(&name, Duration::from_secs(5)).unwrap());
        let (_conn, _listener) = server.join().unwrap();
        let reader = {
            let client = Arc::clone(&client);
            std::thread::spawn(move || {
                let started = Instant::now();
                let result = client.recv::<AppMessage>(Duration::from_secs(30));
                (started.elapsed(), result.map_err(|e| e.kind()))
            })
        };
        std::thread::sleep(Duration::from_millis(300));
        client.close();
        let (waited, result) = reader.join().unwrap();
        assert!(waited < Duration::from_secs(2), "{waited:?}");
        assert_eq!(result.unwrap_err(), io::ErrorKind::ConnectionAborted);
        assert!(client.send(&ShimMessage::Closing).is_err());
    }

    #[test]
    fn second_server_is_refused_and_a_stale_file_is_not() {
        let name = test_name("single");
        let first = PipeListener::bind(&name).unwrap();
        let second = PipeListener::bind(&name).err().expect("second bind must fail");
        assert_eq!(second.kind(), io::ErrorKind::AddrInUse);
        // the file outlives a listener that never got to clean up
        std::mem::forget(first);
        assert!(Path::new(&name).exists());
        let path = name.clone();
        let again = std::thread::spawn(move || PipeListener::bind(&path).map(|_| ()).map_err(|e| e.kind()));
        // still served: refused
        assert_eq!(again.join().unwrap(), Err(io::ErrorKind::AddrInUse));
    }

    #[test]
    fn a_socket_file_nobody_serves_is_replaced() {
        let name = test_name("stale");
        std::fs::create_dir_all(Path::new(&name).parent().unwrap()).unwrap();
        {
            let listener = UnixListener::bind(&name).unwrap();
            drop(listener);
        }
        assert!(Path::new(&name).exists(), "the file stays after the listener");
        let listener = PipeListener::bind(&name).expect("a stale file is no owner");
        drop(listener);
        assert!(!Path::new(&name).exists(), "removed with the listener");
    }

    #[test]
    fn reader_and_writer_threads_do_not_block_each_other() {
        let name = test_name("duplex");
        let mut listener = PipeListener::bind(&name).unwrap();
        let server = std::thread::spawn(move || listener.accept().unwrap());
        let client = Arc::new(connect(&name, Duration::from_secs(5)).unwrap());
        let server_conn = server.join().unwrap();
        let reader = {
            let client = Arc::clone(&client);
            std::thread::spawn(move || client.recv::<AppMessage>(Duration::from_secs(5)).unwrap())
        };
        std::thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        client.send(&ShimMessage::Connecting { attempt: 1 }).unwrap();
        assert!(started.elapsed() < Duration::from_millis(500), "write was blocked by the pending read");
        assert_eq!(
            server_conn.recv::<ShimMessage>(Duration::from_secs(5)).unwrap(),
            Some(ShimMessage::Connecting { attempt: 1 })
        );
        server_conn.send(&AppMessage::Close).unwrap();
        assert_eq!(reader.join().unwrap(), Some(AppMessage::Close));
    }

    #[test]
    fn peer_gone_is_eof() {
        let name = test_name("eof");
        let mut listener = PipeListener::bind(&name).unwrap();
        let server = std::thread::spawn(move || listener.accept().unwrap());
        let client = connect(&name, Duration::from_secs(5)).unwrap();
        let server_conn = server.join().unwrap();
        drop(client);
        let error = server_conn.recv::<ShimMessage>(Duration::from_secs(2)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert!(server_conn.send(&AppMessage::Close).is_err(), "writing to a gone peer fails, without a signal");
    }

    #[test]
    fn connect_times_out_without_a_server() {
        let started = Instant::now();
        assert!(connect(&test_name("nobody"), Duration::from_millis(200)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
