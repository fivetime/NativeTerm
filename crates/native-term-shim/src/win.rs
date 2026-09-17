//! Console and synchronization helpers for the shim.

use std::io;
use std::sync::OnceLock;
use std::time::Duration;

use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{CloseHandle, BOOL, GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::Console::{
    ReadConsoleInputW, SetConsoleCtrlHandler, WriteConsoleInputW, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT,
    INPUT_RECORD, INPUT_RECORD_0, KEY_EVENT, KEY_EVENT_RECORD, KEY_EVENT_RECORD_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, OpenEventW, ResetEvent, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
};

struct OwnedHandle(HANDLE);

// SAFETY: a kernel handle may be used and closed from any thread.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn auth_event_name(shim_pid: u32) -> HSTRING {
    HSTRING::from(format!("Local\\NativeTerm-auth-{shim_pid}"))
}

/// Set by the `LocalCommand` helper when ssh has logged in.
pub struct AuthEvent(OwnedHandle);

impl AuthEvent {
    pub fn create(shim_pid: u32) -> io::Result<AuthEvent> {
        let handle = unsafe { CreateEventW(None, true, false, &auth_event_name(shim_pid))? };
        Ok(AuthEvent(OwnedHandle(handle)))
    }

    pub fn is_set(&self) -> bool {
        unsafe { WaitForSingleObject(self.0 .0, 0) == WAIT_OBJECT_0 }
    }

    pub fn reset(&self) {
        unsafe {
            let _ = ResetEvent(self.0 .0);
        }
    }
}

/// Called by the helper; silently does nothing if the shim is gone.
pub fn signal_authenticated(shim_pid: u32) {
    unsafe {
        if let Ok(handle) = OpenEventW(EVENT_MODIFY_STATE, false, &auth_event_name(shim_pid)) {
            let _ = SetEvent(handle);
            let _ = CloseHandle(handle);
        }
    }
}

fn open_console_input() -> io::Result<OwnedHandle> {
    let handle = unsafe {
        CreateFileW(
            w!("CONIN$"),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )?
    };
    Ok(OwnedHandle(handle))
}

/// Type `text` into this console: one key-down record per UTF-16 unit,
/// then Enter if asked (verified end to end with Windows OpenSSH).
pub fn inject(text: &str, enter: bool) -> io::Result<()> {
    let console = open_console_input()?;
    let units = text.encode_utf16().chain(enter.then_some(u16::from(b'\r')));
    let records: Vec<INPUT_RECORD> = units
        .map(|unit| INPUT_RECORD {
            EventType: KEY_EVENT as u16,
            Event: INPUT_RECORD_0 {
                KeyEvent: KEY_EVENT_RECORD {
                    bKeyDown: BOOL(1),
                    wRepeatCount: 1,
                    wVirtualKeyCode: 0,
                    wVirtualScanCode: 0,
                    uChar: KEY_EVENT_RECORD_0 { UnicodeChar: unit },
                    dwControlKeyState: 0,
                },
            },
        })
        .collect();
    if records.is_empty() {
        return Ok(());
    }
    let mut written = 0u32;
    unsafe { WriteConsoleInputW(console.0, &records, &mut written)? };
    Ok(())
}

/// Reads single key presses from the console (only while ssh isn't
/// running: ssh owns the input then).
pub struct KeyReader(OwnedHandle);

impl KeyReader {
    pub fn open() -> io::Result<KeyReader> {
        Ok(KeyReader(open_console_input()?))
    }

    /// The next typed character, or `None` after `timeout`.
    pub fn read_key(&self, timeout: Duration) -> io::Result<Option<char>> {
        let wait = unsafe { WaitForSingleObject(self.0 .0, timeout.as_millis() as u32) };
        if wait != WAIT_OBJECT_0 {
            return Ok(None);
        }
        let mut records = [INPUT_RECORD::default()];
        let mut read = 0u32;
        unsafe { ReadConsoleInputW(self.0 .0, &mut records, &mut read)? };
        if read == 0 || records[0].EventType != KEY_EVENT as u16 {
            return Ok(None);
        }
        let key = unsafe { records[0].Event.KeyEvent };
        let unit = unsafe { key.uChar.UnicodeChar };
        if !key.bKeyDown.as_bool() || unit == 0 {
            return Ok(None);
        }
        Ok(char::from_u32(u32::from(unit)))
    }
}

static ON_CLOSE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

unsafe extern "system" fn ctrl_handler(event: u32) -> BOOL {
    match event {
        // after ssh exits, Ctrl+C would otherwise end the shim and the tab
        CTRL_C_EVENT | CTRL_BREAK_EVENT => BOOL(1),
        CTRL_CLOSE_EVENT => {
            // Windows ends the process about five seconds later anyway
            if let Some(on_close) = ON_CLOSE.get() {
                on_close();
            }
            BOOL(0)
        }
        _ => BOOL(0),
    }
}

pub fn install_ctrl_handler(on_close: impl Fn() + Send + Sync + 'static) {
    let _ = ON_CLOSE.set(Box::new(on_close));
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(ctrl_handler), true);
    }
}
