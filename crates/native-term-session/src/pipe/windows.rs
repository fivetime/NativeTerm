//! The per-user named pipe between NativeTerm and its shims.
//!
//! - Name: `\\.\pipe\nativeterm-<user SID>-<logon session id>`, so each
//!   user and sign-in has its own NativeTerm.
//! - ACL: the user and SYSTEM only; remote clients rejected.
//! - The first instance is created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so
//!   a second NativeTerm notices the first (`AddrInUse`).
//! - Overlapped I/O: a reader waits on an event (no polling, no wakeups
//!   while idle), and a writer on another thread isn't blocked by it, as it
//!   would be on a synchronous handle.

use std::io;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{de::DeserializeOwned, Serialize};
use windows::core::HSTRING;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0,
};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, WaitNamedPipeW, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

use native_term_win::SecurityDescriptor;

use crate::protocol;

const BUFFER: u32 = 64 * 1024;
const READ_CHUNK: usize = 16 * 1024;

/// This user's and sign-in's pipe name.
pub fn pipe_name() -> io::Result<String> {
    Ok(format!("{}{}-{}", crate::PIPE_NAME_PREFIX, native_term_win::user_sid()?, native_term_win::logon_session_id()?))
}

/// A kernel handle closed on drop.
struct Handle(HANDLE);

// SAFETY: kernel handles may be used and closed from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// An overlapped operation's state, with its own completion event.
struct Pending {
    overlapped: OVERLAPPED,
    _event: Handle,
}

// SAFETY: an operation using this state always completes (or is cancelled
// and awaited) before the call that started it returns, so no pending I/O
// refers to it while it moves or is used from another thread; access is
// serialized by the connection's mutexes.
unsafe impl Send for Pending {}
unsafe impl Sync for Pending {}

impl Pending {
    fn new() -> io::Result<Pending> {
        let event = unsafe { CreateEventW(None, true, false, None)? };
        let overlapped = OVERLAPPED { hEvent: event, ..Default::default() };
        Ok(Pending { overlapped, _event: Handle(event) })
    }

    /// Wait for the operation; `None` on timeout (after cancelling it; a
    /// result that completed meanwhile is still returned).
    fn finish(&mut self, pipe: HANDLE, timeout: Option<Duration>) -> io::Result<Option<u32>> {
        let millis = timeout.map_or(INFINITE, |t| t.as_millis().min(u128::from(INFINITE - 1)) as u32);
        let waited = unsafe { WaitForSingleObject(self.overlapped.hEvent, millis) };
        if waited != WAIT_OBJECT_0 {
            unsafe {
                let _ = CancelIoEx(pipe, Some(&self.overlapped));
            }
        }
        let mut transferred = 0u32;
        match unsafe { GetOverlappedResult(pipe, &self.overlapped, &mut transferred, true) } {
            Ok(()) => Ok(Some(transferred)),
            Err(e) if e.code() == ERROR_OPERATION_ABORTED.to_hresult() => Ok(None),
            Err(e) => Err(map_error(e)),
        }
    }
}

fn map_error(e: windows::core::Error) -> io::Error {
    let gone = [ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED, ERROR_NO_DATA].map(|c| c.to_hresult());
    if gone.contains(&e.code()) {
        io::Error::new(io::ErrorKind::UnexpectedEof, e)
    } else {
        e.into()
    }
}

/// Server side (NativeTerm).
pub struct PipeListener {
    name: HSTRING,
    security: SecurityDescriptor,
    /// Instance waiting for the next client.
    pending: Option<Handle>,
}

impl PipeListener {
    /// Fails with `AddrInUse` if another process already serves `name`.
    pub fn bind(name: &str) -> io::Result<PipeListener> {
        let sddl = format!("D:P(A;;GA;;;{})(A;;GA;;;SY)", native_term_win::user_sid()?);
        let mut listener =
            PipeListener { name: HSTRING::from(name), security: SecurityDescriptor::from_sddl(&sddl)?, pending: None };
        listener.pending = Some(listener.create_instance(true)?);
        Ok(listener)
    }

    fn create_instance(&self, first: bool) -> io::Result<Handle> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.security.as_ptr(),
            bInheritHandle: false.into(),
        };
        let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let handle = unsafe {
            CreateNamedPipeW(
                &self.name,
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER,
                BUFFER,
                0,
                Some(&attributes),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let error = io::Error::last_os_error();
            // ERROR_ACCESS_DENIED with FILE_FLAG_FIRST_PIPE_INSTANCE: already served
            return Err(if first && error.raw_os_error() == Some(5) {
                io::Error::new(io::ErrorKind::AddrInUse, "another NativeTerm owns the pipe")
            } else {
                error
            });
        }
        Ok(Handle(handle))
    }

    /// Wait for the next client.
    pub fn accept(&mut self) -> io::Result<PipeConnection> {
        let instance = match self.pending.take() {
            Some(instance) => instance,
            None => self.create_instance(false)?,
        };
        let mut pending = Pending::new()?;
        let result = unsafe { ConnectNamedPipe(instance.0, Some(&mut pending.overlapped)) };
        if let Err(e) = result {
            let code = e.code();
            if code == ERROR_IO_PENDING.to_hresult() {
                pending.finish(instance.0, None)?;
            } else if code != ERROR_PIPE_CONNECTED.to_hresult() && code != ERROR_NO_DATA.to_hresult() {
                // ERROR_NO_DATA: a quick client (the login helper) already
                // wrote and closed; its messages are still readable
                return Err(e.into());
            }
        }
        // keep an instance ready so clients never find none
        self.pending = Some(self.create_instance(false)?);
        PipeConnection::new(instance)
    }
}

/// Client side (shim): connect, waiting up to `timeout` for the server.
pub fn connect(name: &str, timeout: Duration) -> io::Result<PipeConnection> {
    let started = Instant::now();
    let wide = HSTRING::from(name);
    loop {
        let opened = unsafe {
            CreateFileW(
                &wide,
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
        };
        match opened {
            Ok(handle) => return PipeConnection::new(Handle(handle)),
            Err(e) if started.elapsed() >= timeout => return Err(e.into()),
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => unsafe {
                let _ = WaitNamedPipeW(&wide, 50);
            },
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// One connection, usable from a reader and a writer thread at once.
pub struct PipeConnection {
    pipe: Handle,
    closed: std::sync::atomic::AtomicBool,
    inbox: Mutex<Vec<u8>>,
    /// Held while reading; a read in progress owns this state.
    reader: Mutex<Pending>,
    writer: Mutex<Pending>,
}

impl PipeConnection {
    fn new(pipe: Handle) -> io::Result<PipeConnection> {
        Ok(PipeConnection {
            pipe,
            closed: std::sync::atomic::AtomicBool::new(false),
            inbox: Mutex::new(Vec::new()),
            reader: Mutex::new(Pending::new()?),
            writer: Mutex::new(Pending::new()?),
        })
    }

    /// Server side: the connected process.
    pub fn client_pid(&self) -> io::Result<u32> {
        let mut pid = 0u32;
        unsafe { GetNamedPipeClientProcessId(self.pipe.0, &mut pid)? };
        Ok(pid)
    }

    /// Stop using the connection: a waiting `recv` returns at once, and
    /// later calls fail. The handle closes when the last owner drops it.
    pub fn close(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        unsafe {
            let _ = CancelIoEx(self.pipe.0, None);
        }
    }

    fn check_open(&self) -> io::Result<()> {
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            Err(io::Error::new(io::ErrorKind::ConnectionAborted, "connection closed"))
        } else {
            Ok(())
        }
    }

    pub fn send<T: Serialize>(&self, message: &T) -> io::Result<()> {
        self.check_open()?;
        let line = protocol::encode(message);
        let mut data = line.as_bytes();
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        while !data.is_empty() {
            unsafe {
                let _ = windows::Win32::System::Threading::ResetEvent(writer.overlapped.hEvent);
            }
            writer.overlapped.Internal = 0;
            writer.overlapped.InternalHigh = 0;
            let result = unsafe { WriteFile(self.pipe.0, Some(data), None, Some(&mut writer.overlapped)) };
            if let Err(e) = result {
                if e.code() != ERROR_IO_PENDING.to_hresult() {
                    return Err(map_error(e));
                }
            }
            let written = writer.finish(self.pipe.0, None)?.unwrap_or(0) as usize;
            if written == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "pipe write made no progress"));
            }
            data = &data[written..];
        }
        Ok(())
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
            match self.read_some(left)? {
                Some(bytes) => self.inbox.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&bytes),
                None if Instant::now() >= deadline => return Ok(None),
                None => {}
            }
        }
    }

    /// Bytes that arrived within `timeout`; `None` if none did.
    fn read_some(&self, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        let mut reader = self.reader.lock().unwrap_or_else(|e| e.into_inner());
        let mut buf = vec![0u8; READ_CHUNK];
        unsafe {
            let _ = windows::Win32::System::Threading::ResetEvent(reader.overlapped.hEvent);
        }
        reader.overlapped.Internal = 0;
        reader.overlapped.InternalHigh = 0;
        let result = unsafe { ReadFile(self.pipe.0, Some(&mut buf), None, Some(&mut reader.overlapped)) };
        if let Err(e) = result {
            if e.code() != ERROR_IO_PENDING.to_hresult() {
                return Err(map_error(e));
            }
        }
        match reader.finish(self.pipe.0, Some(timeout))? {
            None => Ok(None),
            Some(0) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "pipe closed")),
            Some(n) => {
                buf.truncate(n as usize);
                Ok(Some(buf))
            }
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
        format!(r"\\.\pipe\nativeterm-test-{tag}-{}", std::process::id())
    }

    #[test]
    fn name_is_per_user_and_sign_in() {
        let name = pipe_name().unwrap();
        assert!(name.starts_with(r"\\.\pipe\nativeterm-S-1-"), "{name}");
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
    fn last_message_survives_a_concurrent_reader() {
        // the client is already polling when the server sends and hangs up
        for round in 0..200 {
            let name = test_name(&format!("race{round}"));
            let mut listener = PipeListener::bind(&name).unwrap();
            let client = std::thread::spawn({
                let name = name.clone();
                move || {
                    let client = connect(&name, Duration::from_secs(5)).unwrap();
                    let mut got = Vec::new();
                    loop {
                        match client.recv::<AppMessage>(Duration::from_secs(5)) {
                            Ok(Some(m)) => got.push(m),
                            Ok(None) => return got,
                            Err(_) => return got,
                        }
                    }
                }
            });
            let conn = listener.accept().unwrap();
            conn.send(&AppMessage::Welcome { protocol: 1 }).unwrap();
            std::thread::sleep(Duration::from_millis(round % 3));
            conn.send(&AppMessage::Close).unwrap();
            drop(conn);
            let got = client.join().unwrap();
            assert_eq!(got, vec![AppMessage::Welcome { protocol: 1 }, AppMessage::Close], "round {round}");
        }
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
    fn idle_reader_uses_no_cpu() {
        let name = test_name("idle");
        let mut listener = PipeListener::bind(&name).unwrap();
        let server = std::thread::spawn(move || (listener.accept().unwrap(), listener));
        let client = connect(&name, Duration::from_secs(5)).unwrap();
        let (_conn, _listener) = server.join().unwrap();
        let before = thread_cpu();
        assert_eq!(client.recv::<AppMessage>(Duration::from_secs(2)).unwrap(), None);
        let used = thread_cpu() - before;
        assert!(used < Duration::from_millis(20), "waiting cost {used:?}");
    }

    fn thread_cpu() -> Duration {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
        let (mut a, mut b, mut kernel, mut user) =
            (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { GetThreadTimes(GetCurrentThread(), &mut a, &mut b, &mut kernel, &mut user).unwrap() };
        let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
    }

    #[test]
    fn second_server_is_refused() {
        let name = test_name("single");
        let _first = PipeListener::bind(&name).unwrap();
        let second = PipeListener::bind(&name).err().expect("second bind must fail");
        assert_eq!(second.kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn reader_and_writer_threads_do_not_block_each_other() {
        let name = test_name("duplex");
        let mut listener = PipeListener::bind(&name).unwrap();
        let server = std::thread::spawn(move || listener.accept().unwrap());
        let client = Arc::new(connect(&name, Duration::from_secs(5)).unwrap());
        let server_conn = server.join().unwrap();

        // the client waits for a message while another thread writes on
        // the same handle
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
    }

    #[test]
    fn connect_times_out_without_a_server() {
        let started = Instant::now();
        assert!(connect(&test_name("nobody"), Duration::from_millis(200)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
