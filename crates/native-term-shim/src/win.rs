//! Console and synchronization helpers for the shim.

use std::io;
use std::sync::OnceLock;
use std::time::Duration;

use windows::core::{w, BOOL, HSTRING};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GA_ROOTOWNER};
use windows::Win32::System::Console::{
    GetConsoleMode, GetConsoleWindow, ReadConsoleInputW, ReadConsoleW, SetConsoleCtrlHandler, SetConsoleMode, WriteConsoleInputW,
    WriteConsoleW, CONSOLE_MODE, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, ENABLE_ECHO_INPUT, INPUT_RECORD, INPUT_RECORD_0,
    KEY_EVENT, KEY_EVENT_RECORD, KEY_EVENT_RECORD_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, OpenEventW, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject, EVENT_MODIFY_STATE,
    INFINITE,
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

/// An unnamed manual-reset event.
pub struct Event(OwnedHandle);

impl Event {
    pub fn new() -> io::Result<Event> {
        Ok(Event(OwnedHandle(unsafe { CreateEventW(None, true, false, None)? })))
    }

    pub fn set(&self) {
        unsafe {
            let _ = SetEvent(self.0 .0);
        }
    }

    pub fn reset(&self) {
        unsafe {
            let _ = ResetEvent(self.0 .0);
        }
    }

    pub fn handle(&self) -> HANDLE {
        self.0 .0
    }
}

/// Block until one of `handles` is signalled (or `timeout`); its index.
pub fn wait_any(handles: &[HANDLE], timeout: Option<Duration>) -> Option<usize> {
    if handles.is_empty() {
        return None;
    }
    let millis = timeout.map_or(INFINITE, |t| t.as_millis().min(u128::from(INFINITE - 1)) as u32);
    let result = unsafe { WaitForMultipleObjects(handles, false, millis) };
    let index = result.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
    (index < handles.len()).then_some(index)
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

    pub fn handle(&self) -> HANDLE {
        self.0 .0
    }
}

/// The Windows Terminal window of this console: ConPTY's hidden console
/// window is owned by the Terminal window hosting the tab.
pub fn terminal_window() -> Option<i64> {
    unsafe {
        let console = GetConsoleWindow();
        if console.is_invalid() {
            return None;
        }
        let owner = GetAncestor(console, GA_ROOTOWNER);
        (!owner.is_invalid() && owner != console).then_some(owner.0 as i64)
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
    open_console(w!("CONIN$"))
}

fn open_console(name: windows::core::PCWSTR) -> io::Result<OwnedHandle> {
    let handle = unsafe {
        CreateFileW(
            name,
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

/// Show `prompt` and read a line from the console itself (not stdin, which
/// may be a pipe), without echo unless `echo`. The line is returned without
/// its line break.
pub fn read_line(prompt: &str, echo: bool) -> io::Result<String> {
    const CR: u16 = 13;
    const LF: u16 = 10;
    let output = open_console(w!("CONOUT$"))?;
    let input = open_console_input()?;
    let prompt: Vec<u16> = prompt.encode_utf16().collect();
    unsafe { WriteConsoleW(output.0, &prompt, None, None)? };
    let mut mode = CONSOLE_MODE::default();
    unsafe { GetConsoleMode(input.0, &mut mode)? };
    let reading = if echo { mode | ENABLE_ECHO_INPUT } else { CONSOLE_MODE(mode.0 & !ENABLE_ECHO_INPUT.0) };
    unsafe { SetConsoleMode(input.0, reading)? };
    let mut units: Vec<u16> = Vec::new();
    let result: io::Result<()> = loop {
        let mut buffer = [0u16; 512];
        let mut read = 0u32;
        if let Err(e) = unsafe { ReadConsoleW(input.0, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read, None) } {
            break Err(e.into());
        }
        if read == 0 {
            break Ok(());
        }
        units.extend_from_slice(&buffer[..read as usize]);
        buffer.fill(0);
        if units.iter().any(|&u| u == CR || u == LF) {
            break Ok(());
        }
    };
    unsafe {
        let _ = SetConsoleMode(input.0, mode);
        if !echo {
            // the Enter wasn't echoed either
            let _ = WriteConsoleW(output.0, &[CR, LF], None, None);
        }
    }
    result?;
    let end = units.iter().position(|&u| u == CR || u == LF).unwrap_or(units.len());
    let line = String::from_utf16_lossy(&units[..end]);
    units.fill(0);
    Ok(line)
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

    /// Signalled while console input is waiting.
    pub fn handle(&self) -> HANDLE {
        self.0 .0
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

/// The console's input and output code pages for a while (a plink session's
/// charset); the previous ones come back on drop.
pub struct CodePages {
    input: u32,
    output: u32,
}

impl CodePages {
    pub fn set(code_page: u32) -> CodePages {
        use windows::Win32::System::Console::{GetConsoleCP, GetConsoleOutputCP, SetConsoleCP, SetConsoleOutputCP};
        let saved = unsafe { CodePages { input: GetConsoleCP(), output: GetConsoleOutputCP() } };
        unsafe {
            let _ = SetConsoleCP(code_page);
            let _ = SetConsoleOutputCP(code_page);
        }
        saved
    }
}

impl Drop for CodePages {
    fn drop(&mut self) {
        use windows::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};
        unsafe {
            if self.input != 0 {
                let _ = SetConsoleCP(self.input);
            }
            if self.output != 0 {
                let _ = SetConsoleOutputCP(self.output);
            }
        }
    }
}

/// `\\.\COM12`: ports above COM9 need the device namespace.
fn serial_device(line: &str) -> String {
    format!(r"\\.\{line}")
}

/// Whether a serial line can be opened now: access denied (5) means
/// another program holds it; not found (2) means there is no such port.
pub fn serial_port_free(line: &str) -> io::Result<()> {
    let device = HSTRING::from(serial_device(line));
    let handle = unsafe {
        CreateFileW(
            &device,
            (GENERIC_READ | GENERIC_WRITE).0,
            windows::Win32::Storage::FileSystem::FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    };
    match handle {
        Ok(handle) => {
            drop(OwnedHandle(handle));
            Ok(())
        }
        Err(e) => Err(io::Error::from_raw_os_error(e.code().0 & 0xFFFF)),
    }
}

/// `MIB_TCP_STATE` values.
pub const TCP_ESTABLISHED: i32 = 5;
pub const TCP_CLOSE_WAIT: i32 = 8;

/// States of all IPv4 and IPv6 TCP connections owned by `pid`.
pub fn tcp_states(pid: u32) -> Vec<i32> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6TABLE_OWNER_PID, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    let mut states = Vec::new();
    for af in [u32::from(AF_INET.0), u32::from(AF_INET6.0)] {
        let mut size = 0u32;
        unsafe {
            let _ = GetExtendedTcpTable(None, &mut size, false, af, TCP_TABLE_OWNER_PID_ALL, 0);
        }
        if size == 0 {
            continue;
        }
        // room for connections opened in between; u32s keep it aligned
        let mut buffer = vec![0u32; (size as usize + 1024) / 4 + 1];
        let mut size = (buffer.len() * 4) as u32;
        let status = unsafe {
            GetExtendedTcpTable(Some(buffer.as_mut_ptr().cast()), &mut size, false, af, TCP_TABLE_OWNER_PID_ALL, 0)
        };
        if status != 0 {
            continue;
        }
        unsafe {
            if af == u32::from(AF_INET.0) {
                let table = &*(buffer.as_ptr() as *const MIB_TCPTABLE_OWNER_PID);
                let rows = std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
                states.extend(rows.iter().filter(|r| r.dwOwningPid == pid).map(|r| r.dwState as i32));
            } else {
                let table = &*(buffer.as_ptr() as *const MIB_TCP6TABLE_OWNER_PID);
                let rows = std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
                states.extend(rows.iter().filter(|r| r.dwOwningPid == pid).map(|r| r.dwState as i32));
            }
        }
    }
    states
}

/// End a process (a half-closed plink).
pub fn terminate(pid: u32) {
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    if let Ok(handle) = unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) } {
        let handle = OwnedHandle(handle);
        unsafe {
            let _ = TerminateProcess(handle.0, 1);
        }
    }
}

/// A fingerprint of what the console shows around the cursor (its
/// position and the text of its line and the one above): it changes when
/// anything is written. `None` without a console.
pub fn screen_fingerprint() -> Option<u64> {
    use std::hash::{Hash, Hasher};
    use windows::Win32::System::Console::{GetConsoleScreenBufferInfo, ReadConsoleOutputCharacterW, CONSOLE_SCREEN_BUFFER_INFO, COORD};
    let output = open_console(w!("CONOUT$")).ok()?;
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    unsafe { GetConsoleScreenBufferInfo(output.0, &mut info) }.ok()?;
    let width = info.dwSize.X.max(1) as usize;
    let cursor = info.dwCursorPosition;
    let first = cursor.Y.saturating_sub(1);
    let mut text = vec![0u16; width * (cursor.Y - first + 1) as usize];
    let mut read = 0u32;
    unsafe { ReadConsoleOutputCharacterW(output.0, &mut text, COORD { X: 0, Y: first }, &mut read) }.ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (cursor.X, cursor.Y, &text[..read as usize]).hash(&mut hasher);
    Some(hasher.finish())
}

/// Ctrl+C as a key (^C for the remote side) rather than a signal: takes
/// "processed input" off the console while it reads key by key. Line
/// input keeps it (Backspace and Enter are handled through it there).
/// Returns whether it changed the mode.
pub fn keep_ctrl_c_as_input() -> bool {
    use windows::Win32::System::Console::{ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT};
    let Ok(input) = open_console_input() else { return false };
    let mut mode = CONSOLE_MODE::default();
    if unsafe { GetConsoleMode(input.0, &mut mode) }.is_err() {
        return false;
    }
    if !mode.contains(ENABLE_PROCESSED_INPUT) || mode.contains(ENABLE_LINE_INPUT) {
        return false;
    }
    unsafe { SetConsoleMode(input.0, CONSOLE_MODE(mode.0 & !ENABLE_PROCESSED_INPUT.0)) }.is_ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn serial_device_paths() {
        assert_eq!(super::serial_device("COM30"), r"\\.\COM30");
        let missing = super::serial_port_free("COM250").unwrap_err();
        assert_eq!(missing.raw_os_error(), Some(2), "no such port");
    }
}
