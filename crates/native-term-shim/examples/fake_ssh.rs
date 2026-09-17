//! A stand-in for `ssh` in the shim's integration tests.
//!
//! - `-G …`: prints nothing (no user keepalive settings), exit 0.
//! - Otherwise: runs the `LocalCommand` through `cmd.exe /c` like Windows
//!   OpenSSH if `FAKE_SSH_LOGIN=1`, sleeps `FAKE_SSH_MS`, and exits with
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
        let local = args.iter().find_map(|a| a.strip_prefix("LocalCommand="));
        if let Some(command) = local {
            let _ = Command::new("cmd.exe").arg("/c").raw_arg(command).status();
        }
    }
    std::thread::sleep(Duration::from_millis(env_num("FAKE_SSH_MS", 200) as u64));
    std::process::exit(env_num("FAKE_SSH_CODE", 0) as i32);
}
