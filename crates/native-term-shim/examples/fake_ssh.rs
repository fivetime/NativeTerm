//! A stand-in for `ssh` in the shim's integration tests.
//!
//! - `-G …`: prints nothing (no user keepalive settings), exit 0; with
//!   `FAKE_SSH_DIRECT=1` a host name, so the shim takes the connection for
//!   a direct one (and, as the fake never connects, for unreachable);
//!   with `FAKE_SSH_G=<k v;k v…>` those lines instead.
//! - Otherwise: runs the `LocalCommand` through `cmd.exe /c` like Windows
//!   OpenSSH if `FAKE_SSH_LOGIN=1`, with `FAKE_SSH_ECHO=1` logs typed lines
//!   (`input: …`) until `exit`, sleeps `FAKE_SSH_MS`, and exits with
//!   `FAKE_SSH_CODE`. Arguments are appended to `FAKE_SSH_LOG`.
//! - `FAKE_SSH_SAID=<s>`: the `LocalCommand` has `<s>` as what the server
//!   said it is (`NATIVETERM_SERVER_VERSION`), like NativeTerm's ssh.
//! - `FAKE_SSH_WINDOWS=1`: runs the remote command with `cmd.exe /c`, like
//!   a Windows sshd.
//! - `FAKE_SSH_PASSWORD=<p>`: logs in only if the forced `SSH_ASKPASS`
//!   helper answers `<p>` (`<p>-other` for hosts named `other…`), asked
//!   up to `FAKE_SSH_TRIES` times (1; once where the arguments say
//!   `NumberOfPasswordPrompts=1`), before the `LocalCommand` runs.
//! - `FAKE_SSH_INTERACTIVE=<bash>`: after the login, runs that shell
//!   interactively (a local stand-in for the remote side).
//! - As ntplink: with `-nt-control <pipe>`, opens that pipe and logs each
//!   command line received (`control: …`) while it runs.
//! - `FAKE_SSH_CODEPAGE=1`: logs the console's output code page, to show
//!   what character set the session runs in.
//! - `FAKE_SSH_LOGIN_ONCE=<file>`: logs in (as `FAKE_SSH_LOGIN=1`) only
//!   while `<file>` doesn't exist, and creates it: the host "goes away"
//!   after the first connection.
//! - `FAKE_SSH_FEED=<text>`: while it runs, writes `<text>` and a line end
//!   every 100 ms into the session log's pipe (`NATIVETERM_LOG`), as an
//!   output record, like NativeTerm's ssh. Parts split by `|` are written
//!   one after the other, 300 ms apart and without a line end (a prompt),
//!   then the last one again. With `FAKE_SSH_ECHO=1` it feeds while it
//!   logs what is typed.

#[cfg(windows)]
mod on_windows {

    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use std::time::Duration;

    pub fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if let Ok(log) = std::env::var("FAKE_SSH_LOG") {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
                let _ = writeln!(f, "{}", args.join(" | "));
                if std::env::var("FAKE_SSH_CODEPAGE").as_deref() == Ok("1") {
                    // SAFETY: a read of the console this process is attached to
                    let page = unsafe { windows::Win32::System::Console::GetConsoleOutputCP() };
                    let _ = writeln!(f, "codepage {page}");
                }
            }
        }
        if args.iter().any(|a| a == "-G") {
            if let Ok(lines) = std::env::var("FAKE_SSH_G") {
                for line in lines.split(';') {
                    println!("{line}");
                }
            } else if std::env::var("FAKE_SSH_DIRECT").as_deref() == Ok("1") {
                println!("hostname {}", args.last().cloned().unwrap_or_default());
            }
            return;
        }
        if let Some(pipe) = args.iter().position(|a| a == "-nt-control").and_then(|i| args.get(i + 1)) {
            control_log(pipe);
        }
        let env_num = |key: &str, default: i64| std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
        // like ssh with SSH_ASKPASS_REQUIRE=force: the password from the
        // helper, as many times as FAKE_SSH_TRIES says (ssh's
        // NumberOfPasswordPrompts: 1 where the arguments say so)
        if let Ok(expected) = std::env::var("FAKE_SSH_PASSWORD") {
            let host = args.iter().skip_while(|a| *a != "--").nth(1).cloned().unwrap_or_default();
            // hosts named "other…" have a different password
            let expected = if host.starts_with("other") { format!("{expected}-other") } else { expected };
            let forced = std::env::var("SSH_ASKPASS_REQUIRE").as_deref() == Ok("force");
            let once = args.iter().any(|a| a == "NumberOfPasswordPrompts=1");
            let tries = if once { 1 } else { env_num("FAKE_SSH_TRIES", 1) };
            let mut right = false;
            for _ in 0..tries {
                // (the user: -l's, else the config's in these tests)
                let user = args.iter().position(|a| a == "-l").and_then(|i| args.get(i + 1)).map_or("tester", |u| u);
                let given = std::env::var_os("SSH_ASKPASS").filter(|_| forced).and_then(|helper| {
                    let output = Command::new(helper).arg(format!("{user}@{host}'s password: ")).output().ok()?;
                    // (a helper that fails is ssh's "cancelled": no more asking)
                    output
                        .status
                        .success()
                        .then(|| String::from_utf8_lossy(&output.stdout).trim_end_matches(['\r', '\n']).to_string())
                });
                match given {
                    Some(given) if given == expected => {
                        right = true;
                        break;
                    }
                    Some(_) => continue,
                    None => break,
                }
            }
            if !right {
                eprintln!("tester@{host}: Permission denied (password).");
                std::process::exit(255);
            }
        }
        // logged in: the LocalCommand
        let once = std::env::var("FAKE_SSH_LOGIN_ONCE").ok().filter(|f| std::fs::File::create_new(f).is_ok());
        if std::env::var("FAKE_SSH_LOGIN").as_deref() == Ok("1") || once.is_some() {
            std::thread::sleep(Duration::from_millis(env_num("FAKE_SSH_LOGIN_DELAY_MS", 0) as u64));
            let local = args.iter().find_map(|a| a.strip_prefix("LocalCommand="));
            if let Some(command) = local {
                let mut local = Command::new("cmd.exe");
                if let Ok(said) = std::env::var("FAKE_SSH_SAID") {
                    local.env("NATIVETERM_SERVER_VERSION", said);
                }
                let _ = local.arg("/c").raw_arg(command).status();
            }
        }
        // run the remote command with a local POSIX shell (key installation)
        if let Ok(sh) = std::env::var("FAKE_SSH_SH") {
            let script = args.last().cloned().unwrap_or_default();
            let code = Command::new(sh).arg("-c").arg(script).status().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
            std::process::exit(code);
        }
        // the remote command through cmd.exe, like a Windows sshd's default shell
        if std::env::var("FAKE_SSH_WINDOWS").as_deref() == Ok("1") {
            let command = args.last().cloned().unwrap_or_default();
            let code = Command::new("cmd.exe")
                .arg("/c")
                .raw_arg(command)
                .status()
                .map(|s| s.code().unwrap_or(-1))
                .unwrap_or(-1);
            std::process::exit(code);
        }
        // an interactive shell as the "remote side" (manual checks in a tab)
        if let Ok(shell) = std::env::var("FAKE_SSH_INTERACTIVE") {
            let code = Command::new(shell)
                .args(["--norc", "--noprofile", "-i"])
                .status()
                .map(|s| s.code().unwrap_or(-1))
                .unwrap_or(-1);
            std::process::exit(code);
        }
        let ms = env_num("FAKE_SSH_MS", 200) as u64;
        let echo = std::env::var("FAKE_SSH_ECHO").as_deref() == Ok("1");
        if let (true, Ok(text), Ok(handle)) = (echo, std::env::var("FAKE_SSH_FEED"), std::env::var("NATIVETERM_LOG")) {
            std::thread::spawn(move || feed(&text, &handle, Duration::from_millis(ms)));
        }
        if echo {
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
        match (echo, std::env::var("FAKE_SSH_FEED"), std::env::var("NATIVETERM_LOG")) {
            (true, _, _) => {}
            (false, Ok(text), Ok(handle)) => feed(&text, &handle, Duration::from_millis(ms)),
            _ => std::thread::sleep(Duration::from_millis(ms)),
        }
        std::process::exit(env_num("FAKE_SSH_CODE", 0) as i32);
    }

    /// `text` as output records into the inherited pipe, for `how_long`.
    fn feed(text: &str, handle: &str, how_long: Duration) {
        use std::os::windows::io::FromRawHandle;
        let Ok(raw) = handle.parse::<usize>() else { return };
        // SAFETY: the handle is the pipe's write end this process
        // inherited, open for its whole life and used only here.
        let mut pipe = unsafe { std::fs::File::from_raw_handle(raw as *mut std::ffi::c_void) };
        let record = |body: &str| {
            let mut record = vec![b'O'];
            record.extend_from_slice(&(body.len() as u32).to_le_bytes());
            record.extend_from_slice(body.as_bytes());
            record
        };
        let parts: Vec<&str> = text.split('|').collect();
        let (bodies, pause) = match parts.len() {
            1 => (vec![format!("{text}\r\n")], Duration::from_millis(100)),
            _ => (parts.iter().map(|p| p.to_string()).collect(), Duration::from_millis(300)),
        };
        let until = std::time::Instant::now() + how_long;
        let mut result = Ok(());
        let mut next = 0;
        while std::time::Instant::now() < until {
            result = result.and(pipe.write_all(&record(&bodies[next])));
            next = (next + 1).min(bodies.len() - 1);
            std::thread::sleep(pause);
        }
        if let Ok(log) = std::env::var("FAKE_SSH_LOG") {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
                let _ = writeln!(f, "feed {handle}: {result:?}");
            }
        }
        std::mem::forget(pipe);
    }

    /// Like ntplink: reads NativeTerm's commands from its control pipe.
    fn control_log(pipe: &str) {
        use std::io::BufRead;
        let Ok(file) = std::fs::File::open(pipe) else { return };
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
                if let Ok(log) = std::env::var("FAKE_SSH_LOG") {
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
                        let _ = writeln!(f, "control: {line}");
                    }
                }
            }
        });
    }

    fn console_lines() -> impl Iterator<Item = String> {
        use windows::core::w;
        use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
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
}

fn main() {
    #[cfg(windows)]
    on_windows::main();
}
