//! The per-user named pipe between NativeTerm and its shims.
//!
//! - Name: `\\.\pipe\nativeterm-<user SID>-<logon session id>`, so each
//!   user and sign-in has its own NativeTerm.
//! - ACL: the user and SYSTEM only; remote clients rejected.
//! - The first instance is created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so
//!   a second NativeTerm notices the first (`AddrInUse`).
//! - Reads never block: the handles are synchronous, and a blocked
//!   `ReadFile` would stall `WriteFile` on the same handle. Readers check
//!   `PeekNamedPipe` first and poll.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{de::DeserializeOwned, Serialize};
use windows::core::HSTRING;
use windows::Win32::Foundation::{ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PeekNamedPipe, WaitNamedPipeW,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

use native_term_win::SecurityDescriptor;

use crate::protocol;

const POLL: Duration = Duration::from_millis(20);
const BUFFER: u32 = 64 * 1024;

/// This user's and sign-in's pipe name.
pub fn pipe_name() -> io::Result<String> {
    Ok(format!(
        "{}{}-{}",
        crate::PIPE_NAME_PREFIX,
        native_term_win::user_sid()?,
        native_term_win::logon_session_id()?
    ))
}

/// Server side (NativeTerm).
pub struct PipeListener {
    name: HSTRING,
    security: SecurityDescriptor,
    /// Instance waiting for the next client.
    pending: Option<File>,
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

    fn create_instance(&self, first: bool) -> io::Result<File> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.security.as_ptr(),
            bInheritHandle: false.into(),
        };
        let mut open_mode = PIPE_ACCESS_DUPLEX;
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
        Ok(unsafe { File::from_raw_handle(handle.0 as _) })
    }

    /// Wait for the next client.
    pub fn accept(&mut self) -> io::Result<PipeConnection> {
        let file = match self.pending.take() {
            Some(file) => file,
            None => self.create_instance(false)?,
        };
        let result = unsafe { ConnectNamedPipe(handle(&file), None) };
        if let Err(e) = result {
            // ERROR_NO_DATA: a quick client (the login helper) already wrote
            // and closed; its messages are still readable
            if e.code() != ERROR_PIPE_CONNECTED.to_hresult() && e.code() != ERROR_NO_DATA.to_hresult() {
                return Err(e.into());
            }
        }
        // keep an instance ready so clients never find none
        self.pending = Some(self.create_instance(false)?);
        Ok(PipeConnection::new(file))
    }
}

/// Client side (shim): connect, waiting up to `timeout` for the server.
pub fn connect(name: &str, timeout: Duration) -> io::Result<PipeConnection> {
    let started = Instant::now();
    loop {
        match OpenOptions::new().read(true).write(true).open(name) {
            Ok(file) => return Ok(PipeConnection::new(file)),
            Err(e) if started.elapsed() >= timeout => return Err(e),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => unsafe {
                let _ = WaitNamedPipeW(&HSTRING::from(name), 50);
            },
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// One connection, usable from a reader and a writer thread at once.
pub struct PipeConnection {
    file: File,
    inbox: Mutex<Vec<u8>>,
    write_lock: Mutex<()>,
}

impl PipeConnection {
    fn new(file: File) -> PipeConnection {
        PipeConnection { file, inbox: Mutex::new(Vec::new()), write_lock: Mutex::new(()) }
    }

    /// Server side: the connected process.
    pub fn client_pid(&self) -> io::Result<u32> {
        let mut pid = 0u32;
        unsafe { GetNamedPipeClientProcessId(handle(&self.file), &mut pid)? };
        Ok(pid)
    }

    pub fn send<T: Serialize>(&self, message: &T) -> io::Result<()> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        (&self.file).write_all(protocol::encode(message).as_bytes())?;
        (&self.file).flush()
    }

    /// Next message, or `Ok(None)` after `timeout`. `UnexpectedEof` when
    /// the other side has gone.
    pub fn recv<T: DeserializeOwned>(&self, timeout: Duration) -> io::Result<Option<T>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(line) = self.take_line() {
                return protocol::decode(&line).map(Some);
            }
            let available = self.available()?;
            if available > 0 {
                let mut buf = vec![0u8; available as usize];
                let n = (&self.file).read(&mut buf)?;
                self.inbox.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&buf[..n]);
                continue;
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(POLL);
        }
    }

    fn take_line(&self) -> Option<String> {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        let end = inbox.iter().position(|&b| b == b'\n')?;
        let line: Vec<u8> = inbox.drain(..=end).collect();
        Some(String::from_utf8_lossy(&line).into_owned())
    }

    fn available(&self) -> io::Result<u32> {
        let mut available = 0u32;
        unsafe { PeekNamedPipe(handle(&self.file), None, 0, None, Some(&mut available), None) }.map_err(|e| {
            // ERROR_BROKEN_PIPE / ERROR_PIPE_NOT_CONNECTED: the peer is gone
            io::Error::new(io::ErrorKind::UnexpectedEof, e)
        })?;
        Ok(available)
    }
}

fn handle(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle() as _)
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
            })
            .unwrap();
        assert_eq!(client.recv::<AppMessage>(Duration::from_secs(5)).unwrap(), Some(AppMessage::Welcome { protocol: 1 }));
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
        assert_eq!(client.recv::<AppMessage>(Duration::from_secs(5)).unwrap(), Some(AppMessage::Welcome { protocol: 1 }));
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
        client.send(&ShimMessage::Connecting).unwrap();
        assert!(started.elapsed() < Duration::from_millis(500), "write was blocked by the pending read");
        assert_eq!(server_conn.recv::<ShimMessage>(Duration::from_secs(5)).unwrap(), Some(ShimMessage::Connecting));
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
