//! Prototype: shim behavior inside a Windows Terminal tab.
//!
//! shim-probe exit <code> <secs>   log WT_SESSION, wait, exit with <code>
//! shim-probe child <secs>         run a short child, then ignore Ctrl+C and wait <secs>
//! shim-probe ctrlc <pid>          send CTRL_C_EVENT to the console of process <pid>
//! shim-probe ssh <host>           run ssh with the LocalCommand login signal, log its exit code
//! shim-probe authenticated        (used as LocalCommand) log the login signal, print nothing
//! shim-probe cred-store <target> <user>   store env NT_PROTO_PW in Credential Manager (never logged)
//! shim-probe cred-delete <target>         delete that credential
//! shim-probe ssh-askpass <host> <target> <user@host>
//!                                 run ssh with SSH_ASKPASS=this exe (force); the helper answers
//!                                 only "<user@host>'s password: " from <target>, and asks in the
//!                                 console for anything else
//! shim-probe inject <pid> <text>  write <text> + Enter into the console input of <pid>
//!                                 (key-down records, character only, as NativeTerm would)
//!
//! Everything is logged to %TEMP%\nativeterm-proto.log.

use std::io::Write;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use windows::Win32::Foundation::BOOL;
use windows::Win32::System::Console::{
    AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT,
    CTRL_C_EVENT,
};

fn log(msg: &str) {
    let path = std::env::temp_dir().join("nativeterm-proto.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[pid {}] {msg}", std::process::id());
    }
}

unsafe extern "system" fn handler(ctrl: u32) -> BOOL {
    match ctrl {
        CTRL_C_EVENT | CTRL_BREAK_EVENT => {
            log("received Ctrl+C/Break, ignoring");
            BOOL(1)
        }
        CTRL_CLOSE_EVENT => {
            log("received CTRL_CLOSE_EVENT");
            BOOL(0)
        }
        _ => BOOL(0),
    }
}

/// One key-down record per UTF-16 unit, character only (vk 0, scan 0), then '\r'.
unsafe fn inject(pid: u32, text: &str) -> windows::core::Result<u32> {
    use windows::core::w;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{
        WriteConsoleInputW, INPUT_RECORD, INPUT_RECORD_0, KEY_EVENT, KEY_EVENT_RECORD, KEY_EVENT_RECORD_0,
    };

    let _ = FreeConsole();
    AttachConsole(pid)?;
    let conin = CreateFileW(
        w!("CONIN$"),
        (GENERIC_READ | GENERIC_WRITE).0,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        None,
        OPEN_EXISTING,
        FILE_FLAGS_AND_ATTRIBUTES(0),
        None,
    )?;
    let records: Vec<INPUT_RECORD> = text
        .encode_utf16()
        .chain(std::iter::once(b'\r' as u16))
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
    let mut written = 0u32;
    let result = WriteConsoleInputW(conin, &records, &mut written);
    let _ = CloseHandle(conin);
    let _ = FreeConsole();
    result.map(|_| written)
}

fn arg(n: usize) -> Option<String> {
    std::env::args().nth(n)
}

/// TCP states (MIB_TCP_STATE: 5 established, 8 close-wait) of all IPv4 and
/// IPv6 connections owned by `pid`.
fn tcp_states(pid: u32) -> Vec<i32> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6TABLE_OWNER_PID, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    let mut states = Vec::new();
    for af in [AF_INET.0 as u32, AF_INET6.0 as u32] {
        let mut size = 0u32;
        unsafe {
            let _ = GetExtendedTcpTable(None, &mut size, false, af, TCP_TABLE_OWNER_PID_ALL, 0);
        }
        if size == 0 {
            continue;
        }
        let mut buf = vec![0u8; size as usize + 1024];
        let mut size = buf.len() as u32;
        let rc = unsafe {
            GetExtendedTcpTable(Some(buf.as_mut_ptr() as *mut _), &mut size, false, af, TCP_TABLE_OWNER_PID_ALL, 0)
        };
        if rc != 0 {
            continue;
        }
        unsafe {
            if af == AF_INET.0 as u32 {
                let table = &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID);
                let rows = std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
                states.extend(rows.iter().filter(|r| r.dwOwningPid == pid).map(|r| r.dwState as i32));
            } else {
                let table = &*(buf.as_ptr() as *const MIB_TCP6TABLE_OWNER_PID);
                let rows = std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
                states.extend(rows.iter().filter(|r| r.dwOwningPid == pid).map(|r| r.dwState as i32));
            }
        }
    }
    states
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn cred_store(target: &str, user: &str, password: &str) -> windows::core::Result<()> {
    use windows::core::PWSTR;
    use windows::Win32::Security::Credentials::{
        CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };
    let mut target_w = wide(target);
    let mut user_w = wide(user);
    let mut blob: Vec<u8> = password.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let cred = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target_w.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user_w.as_mut_ptr()),
        ..Default::default()
    };
    let result = unsafe { CredWriteW(&cred, 0) };
    blob.fill(0);
    result
}

fn cred_read(target: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC};
    let target_w = wide(target);
    let mut ptr: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        CredReadW(PCWSTR(target_w.as_ptr()), CRED_TYPE_GENERIC, 0, &mut ptr).ok()?;
        let c = &*ptr;
        let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
        let secret = String::from_utf16(&units).ok();
        CredFree(ptr as *const _);
        secret
    }
}

fn cred_delete(target: &str) -> windows::core::Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};
    let target_w = wide(target);
    unsafe { CredDeleteW(PCWSTR(target_w.as_ptr()), CRED_TYPE_GENERIC, 0) }
}

/// Ask in the tab's own console (echo off unless it's a yes/no confirmation).
fn console_prompt(prompt: &str, echo: bool) -> String {
    use windows::core::w;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{
        GetConsoleMode, ReadConsoleW, SetConsoleMode, WriteConsoleW, CONSOLE_MODE, ENABLE_ECHO_INPUT,
    };
    unsafe {
        let open = |name| {
            CreateFileW(
                name,
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        };
        let (Ok(conin), Ok(conout)) = (open(w!("CONIN$")), open(w!("CONOUT$"))) else {
            return String::new();
        };
        let text: Vec<u16> = prompt.encode_utf16().collect();
        let _ = WriteConsoleW(conout, &text, None, None);
        let mut mode = CONSOLE_MODE(0);
        let _ = GetConsoleMode(conin, &mut mode);
        if !echo {
            let _ = SetConsoleMode(conin, mode & !ENABLE_ECHO_INPUT);
        }
        let mut buf = [0u16; 512];
        let mut read = 0u32;
        let _ = ReadConsoleW(conin, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut read, None);
        let _ = SetConsoleMode(conin, mode);
        if !echo {
            let _ = WriteConsoleW(conout, &[b'\r' as u16, b'\n' as u16], None, None);
        }
        let _ = CloseHandle(conin);
        let _ = CloseHandle(conout);
        String::from_utf16_lossy(&buf[..read as usize]).trim_end_matches(['\r', '\n']).to_string()
    }
}

fn askpass_helper() -> ! {
    let prompt = arg(1).unwrap_or_default();
    let target = std::env::var("NT_PROTO_TARGET").unwrap_or_default();
    let user_host = std::env::var("NT_PROTO_USERHOST").unwrap_or_default();
    let confirm = std::env::var("SSH_ASKPASS_PROMPT").map(|v| v == "confirm").unwrap_or(false);
    let expected = format!("{user_host}'s password: ");
    let answer = if !confirm && prompt == expected {
        match cred_read(&target) {
            Some(p) => {
                log(&format!("askpass: answered {prompt:?} from Credential Manager"));
                p
            }
            None => {
                log("askpass: no stored credential, asking in console");
                console_prompt(&prompt, false)
            }
        }
    } else {
        log(&format!("askpass: not our password prompt ({prompt:?}, confirm={confirm}), asking in console"));
        console_prompt(&prompt, confirm)
    };
    println!("{answer}");
    std::process::exit(0);
}

fn main() {
    // ssh's LocalCommand (and ProxyCommand) children inherit the askpass
    // environment, so the variable alone doesn't mean "called as askpass":
    // ssh calls the helper with exactly one argument, the prompt.
    const SUBCOMMANDS: &[&str] = &["authenticated", "inject", "ssh", "ssh-askpass", "cred-store", "cred-delete"];
    if std::env::var("NT_PROTO_ASKPASS").is_ok()
        && std::env::args().len() == 2
        && !arg(1).is_some_and(|a| SUBCOMMANDS.contains(&a.as_str()))
    {
        askpass_helper();
    }
    let session = std::env::var("WT_SESSION").unwrap_or_default();
    match arg(1).as_deref() {
        Some("plink") => {
            // Run plink as a child and watch its TCP connections: plink -raw
            // doesn't notice a server close until the next write (CLOSE_WAIT).
            let plink_args: Vec<String> = std::env::args().skip(2).collect();
            let mut child = Command::new(r"C:\Program Files\PuTTY\plink.exe").args(&plink_args).spawn().expect("plink");
            log(&format!("plink mode: pid {} args {plink_args:?}", child.id()));
            let started = std::time::Instant::now();
            let mut seen_established = false;
            loop {
                if let Ok(Some(status)) = child.try_wait() {
                    log(&format!("plink exited by itself: {status:?} after {:?}", started.elapsed()));
                    println!("[probe] plink exited: {status:?}");
                    break;
                }
                let states = tcp_states(child.id());
                if !seen_established && states.contains(&5) {
                    seen_established = true;
                    log(&format!("connection established after {:?}", started.elapsed()));
                }
                if states.contains(&8) {
                    log(&format!(
                        "CLOSE_WAIT detected after {:?}: server closed; terminating plink",
                        started.elapsed()
                    ));
                    let _ = child.kill();
                    let _ = child.wait();
                    println!("\r\n[probe] disconnected: the server closed the connection");
                    break;
                }
                sleep(Duration::from_millis(300));
            }
        }
        Some("restored") => {
            // A shim started without a host: as if it asked NativeTerm for its
            // session by WT_SESSION, label the pane with a per-session title.
            let secs: u64 = arg(2).and_then(|s| s.parse().ok()).unwrap_or(900);
            let suffix = &session[session.len().saturating_sub(4)..];
            let title = format!("nt-restored-{suffix}");
            let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            let ok = unsafe { windows::Win32::System::Console::SetConsoleTitleW(windows::core::PCWSTR(wide.as_ptr())) };
            log(&format!("restored mode: WT_SESSION={session} set title {title:?}: {ok:?}"));
            println!("[probe] restored session {session}, title {title}");
            sleep(Duration::from_secs(secs));
        }
        Some("exit") => {
            let code: i32 = arg(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            let secs: u64 = arg(3).and_then(|s| s.parse().ok()).unwrap_or(3);
            log(&format!("exit mode: WT_SESSION={session} code={code} after {secs}s"));
            println!("exit mode: will exit with {code} in {secs}s");
            sleep(Duration::from_secs(secs));
            log(&format!("exiting with {code}"));
            std::process::exit(code);
        }
        Some("child") => {
            let secs: u64 = arg(2).and_then(|s| s.parse().ok()).unwrap_or(60);
            log(&format!("child mode: WT_SESSION={session}"));
            let status = Command::new("cmd").args(["/c", "echo child running & ping -n 2 127.0.0.1 >nul"]).status();
            log(&format!("child exited: {status:?}"));
            unsafe {
                let _ = SetConsoleCtrlHandler(Some(handler), true);
            }
            log("handler installed, waiting");
            println!("child done; ignoring Ctrl+C for {secs}s");
            sleep(Duration::from_secs(secs));
            log("wait over, exiting 0");
        }
        Some("ctrlc") => {
            let pid: u32 = arg(2).and_then(|s| s.parse().ok()).expect("pid");
            unsafe {
                let _ = FreeConsole();
                AttachConsole(pid).expect("AttachConsole");
                let _ = SetConsoleCtrlHandler(None, true);
                GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0).expect("GenerateConsoleCtrlEvent");
                sleep(Duration::from_millis(500));
                let _ = FreeConsole();
            }
            log(&format!("sent CTRL_C_EVENT to console of pid {pid}"));
        }
        Some("ssh") => {
            let host = arg(2).expect("host");
            let attempts: u32 = arg(3).and_then(|s| s.parse().ok()).unwrap_or(1);
            let me = std::env::current_exe().expect("exe");
            let known_hosts = std::env::temp_dir().join("nt-proto-known_hosts");
            // quoted so a helper path with spaces survives cmd.exe
            let local_command = format!("LocalCommand=\"{}\" authenticated", me.display());
            let args = [
                "-o".to_string(),
                format!("UserKnownHostsFile={}", known_hosts.display()),
                "-o".into(),
                "StrictHostKeyChecking=accept-new".into(),
                "-o".into(),
                "NumberOfPasswordPrompts=1".into(),
                "-o".into(),
                "PermitLocalCommand=yes".into(),
                "-o".into(),
                local_command,
                host.clone(),
            ];
            unsafe {
                let _ = SetConsoleCtrlHandler(Some(handler), true);
            }
            for attempt in 1..=attempts {
                println!("[probe] attempt {attempt}/{attempts}");
                log(&format!("ssh attempt {attempt}: WT_SESSION={session} args={args:?}"));
                let status = Command::new("ssh").args(&args).status();
                let code = status.as_ref().ok().and_then(|s| s.code());
                log(&format!("ssh attempt {attempt} exited: {status:?} code={code:?}"));
                println!("[probe] ssh exited with {code:?}");
            }
            println!("[probe] closing in 5s");
            sleep(Duration::from_secs(5));
        }
        // "spew-default" takes no numeric arguments: some launchers (Tabby's CLI)
        // turn numeric arguments into numbers and then fail to spawn.
        Some(mode @ ("spew" | "spew-default")) => {
            let (lines, secs): (usize, u64) = if mode == "spew-default" {
                (20000, 120)
            } else {
                (
                    arg(2).and_then(|s| s.parse().ok()).unwrap_or(20000),
                    arg(3).and_then(|s| s.parse().ok()).unwrap_or(60),
                )
            };
            let filler = "x".repeat(90);
            let mut out = std::io::BufWriter::new(std::io::stdout().lock());
            for i in 0..lines {
                let _ = writeln!(out, "{i:06} {filler}");
            }
            drop(out);
            sleep(Duration::from_secs(secs));
        }
        Some("cred-store") => {
            let target = arg(2).expect("target");
            let user = arg(3).expect("user");
            let pw = std::env::var("NT_PROTO_PW").expect("NT_PROTO_PW");
            let r = cred_store(&target, &user, &pw);
            log(&format!("cred-store {target} for {user}: {r:?}"));
            println!("cred-store: {r:?}");
        }
        Some("cred-delete") => {
            let target = arg(2).expect("target");
            let r = cred_delete(&target);
            log(&format!("cred-delete {target}: {r:?}"));
            println!("cred-delete: {r:?}");
        }
        Some("ssh-askpass") => {
            let host = arg(2).expect("host");
            let target = arg(3).expect("target");
            let user_host = arg(4).expect("user@host");
            let me = std::env::current_exe().expect("exe");
            let known_hosts = std::env::temp_dir().join("nt-proto-known_hosts");
            let args = [
                "-o".to_string(),
                format!("UserKnownHostsFile={}", known_hosts.display()),
                "-o".into(),
                // NT_PROTO_STRICT=ask forces a host-key question, i.e. a prompt the helper must not answer
                format!("StrictHostKeyChecking={}", std::env::var("NT_PROTO_STRICT").unwrap_or("accept-new".into())),
                "-o".into(),
                "NumberOfPasswordPrompts=1".into(),
                "-o".into(),
                "PermitLocalCommand=yes".into(),
                "-o".into(),
                format!("LocalCommand=\"{}\" authenticated", me.display()),
                host.clone(),
            ];
            unsafe {
                let _ = SetConsoleCtrlHandler(Some(handler), true);
            }
            log(&format!("ssh-askpass: WT_SESSION={session} host={host}"));
            let status = Command::new("ssh")
                .args(&args)
                .env("SSH_ASKPASS", &me)
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env("NT_PROTO_ASKPASS", "1")
                .env("NT_PROTO_TARGET", &target)
                .env("NT_PROTO_USERHOST", &user_host)
                .status();
            let code = status.as_ref().ok().and_then(|s| s.code());
            log(&format!("ssh-askpass exited: code={code:?}"));
            println!("[probe] ssh exited with {code:?}; closing in 5s");
            sleep(Duration::from_secs(5));
        }
        Some("authenticated") => {
            log(&format!("LocalCommand fired: WT_SESSION={session}"));
        }
        Some("inject") => {
            let pid: u32 = arg(2).and_then(|s| s.parse().ok()).expect("pid");
            let text = arg(3).unwrap_or_default();
            let result = unsafe { inject(pid, &text) };
            log(&format!("inject into console of pid {pid}: {text:?} -> {result:?}"));
        }
        _ => eprintln!("usage: shim-probe exit <code> <secs> | child <secs> | ctrlc <pid> | inject <pid> <text>"),
    }
}
