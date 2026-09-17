//! A stand-in for `ssh` in the shim's integration tests.
//!
//! - `-G …`: prints nothing (no user keepalive settings), exit 0.
//! - Otherwise: runs the `LocalCommand` through `cmd.exe /c` like Windows
//!   OpenSSH if `FAKE_SSH_LOGIN=1`, with `FAKE_SSH_ECHO=1` logs typed lines
//!   (`input: …`) until `exit`, sleeps `FAKE_SSH_MS`, and exits with
//!   `FAKE_SSH_CODE`. Arguments are appended to `FAKE_SSH_LOG`.

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Ok(log) = std::env::var("FAKE_SSH_LOG") {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
            let _ = writeln!(f, "{}", args.join(" | "));
        }
    }
    if args.first().map(String::as_str) == Some("-G") {
        return;
    }
    let env_num = |key: &str, default: i64| std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    if std::env::var("FAKE_SSH_LOGIN").as_deref() == Ok("1") {
        std::thread::sleep(Duration::from_millis(env_num("FAKE_SSH_LOGIN_DELAY_MS", 0) as u64));
        let local = args.iter().find_map(|a| a.strip_prefix("LocalCommand="));
        if let Some(command) = local {
            let _ = Command::new("cmd.exe").arg("/c").raw_arg(command).status();
        }
    }
    if std::env::var("FAKE_SSH_ECHO").as_deref() == Ok("1") {
        // like Windows OpenSSH: key records straight from the console input
        // buffer, a line per Enter, until "exit"
        for text in console_lines() {
            if let Ok(log) = std::env::var("FAKE_SSH_LOG") {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
                    let _ = writeln!(f, "input: {text}");
                }
            }
            if text == "exit" {
                break;
            }
        }
    }
    std::thread::sleep(Duration::from_millis(env_num("FAKE_SSH_MS", 200) as u64));
    std::process::exit(env_num("FAKE_SSH_CODE", 0) as i32);
}

fn console_lines() -> impl Iterator<Item = String> {
    use windows::core::w;
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows::Win32::System::Console::{ReadConsoleInputW, INPUT_RECORD, KEY_EVENT};
    // the console itself: in the tests stdin is the test runner's
    let input = unsafe {
        CreateFileW(
            w!("CONIN$"),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .ok();
    let mut units: Vec<u16> = Vec::new();
    let mut lines = std::collections::VecDeque::new();
    std::iter::from_fn(move || {
        let input = input?;
        loop {
            if let Some(line) = lines.pop_front() {
                return Some(line);
            }
            let mut records = [INPUT_RECORD::default(); 64];
            let mut read = 0u32;
            unsafe { ReadConsoleInputW(input, &mut records, &mut read) }.ok()?;
            for record in &records[..read as usize] {
                if u32::from(record.EventType) != KEY_EVENT {
                    continue;
                }
                let key = unsafe { record.Event.KeyEvent };
                if !key.bKeyDown.as_bool() {
                    continue;
                }
                match unsafe { key.uChar.UnicodeChar } {
                    0 => {}
                    13 => {
                        lines.push_back(String::from_utf16_lossy(&units));
                        units.clear();
                    }
                    unit => units.push(unit),
                }
            }
        }
    })
}
