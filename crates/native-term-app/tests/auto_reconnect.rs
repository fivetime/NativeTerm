#![cfg(windows)]
//! Automatic reconnects, with shims started directly (not in a Terminal
//! tab) and a fake ssh. Needs no other NativeTerm running (it serves the
//! user's pipe) and a portable Terminal folder for the core's setup.
//!
//! ```text
//! cargo build -p native-term-shim --examples
//! NATIVETERM_TEST_WT_DIR=<portable Terminal> cargo test -p native-term-app --test auto_reconnect <name> -- --ignored
//! ```
//!
//! One test per run: a test's core keeps the pipe until the process ends.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use native_term_app::registry::Registry;
use native_term_app::{Core, SessionView, State};
use native_term_platform::windows_terminal::install::Install;
use native_term_platform::windows_terminal::WindowsTerminal;

fn target_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().join("target").join("debug")
}

fn core() -> Core {
    let dir = std::env::var("NATIVETERM_TEST_WT_DIR").expect("set NATIVETERM_TEST_WT_DIR to a portable Terminal");
    let install = Install::from_dir(dir.as_ref()).unwrap();
    let shim = target_dir().join("nativeterm-shim.exe");
    Core::start(WindowsTerminal::new(install, &shim), Some(Registry::in_memory().unwrap()))
        .expect("is a NativeTerm running?")
}

struct Shim(Child);

impl Drop for Shim {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn spawn_shim(session: &str, guid: &str, envs: &[(&str, &str)]) -> Shim {
    let fake = target_dir().join("examples").join("fake_ssh.exe");
    assert!(fake.exists(), "cargo build -p native-term-shim --examples");
    let mut command = Command::new(target_dir().join("nativeterm-shim.exe"));
    command
        .args(["--session", session, "nativeterm-test.invalid"])
        .env("NATIVETERM_SSH", fake)
        .env("WT_SESSION", guid)
        .env("NATIVETERM_START_APP", "0")
        .env("FAKE_SSH_MS", "300")
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    for (k, v) in envs {
        command.env(k, v);
    }
    Shim(command.spawn().unwrap())
}

fn session(core: &Core, id: &str) -> Option<SessionView> {
    core.sessions().into_iter().find(|s| s.id == id)
}

fn wait_until(what: &str, limit: Duration, mut check: impl FnMut() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what}");
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("{what}: {:.1} s", started.elapsed().as_secs_f64());
}

/// The host goes away after the first connection (network down): the
/// automatic reconnects never reach it, and they go on, counting up,
/// instead of stopping at the first failure. (The fake ssh never opens a
/// connection; with `FAKE_SSH_DIRECT` the shim sees a direct one, so each
/// failed try is "could not connect".)
#[test]
#[ignore = "serves the user's NativeTerm pipe; needs a portable Terminal folder"]
fn failed_reconnects_keep_retrying() {
    let core = core();
    core.set_auto_reconnect(true);
    let marker = std::env::temp_dir().join(format!("nativeterm-ar-once-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let _shim = spawn_shim(
        "ar-away",
        "6e7a0000-0000-4000-8000-0000000a0003",
        &[("FAKE_SSH_LOGIN_ONCE", marker.to_str().unwrap()), ("FAKE_SSH_CODE", "255"), ("FAKE_SSH_DIRECT", "1")],
    );
    wait_until("dropped after the first login", Duration::from_secs(15), || {
        session(&core, "ar-away").is_some_and(|s| s.attempt == 1 && matches!(s.state, State::Disconnected(_)))
    });
    // retry 1 after ~3 s fails before a login; retry 2 follows ~10 s later
    wait_until("a failed reconnect was retried", Duration::from_secs(30), || {
        session(&core, "ar-away").is_some_and(|s| s.attempt >= 3)
    });
    wait_until("shown as reconnecting after a failure", Duration::from_secs(15), || {
        session(&core, "ar-away")
            .is_some_and(|s| matches!(s.state, State::Unreachable(_)) && s.auto_retry.is_some_and(|n| n >= 2))
    });
    core.set_auto_reconnect(false);
    core.close("ar-away");
    wait_until("closed", Duration::from_secs(10), || session(&core, "ar-away").is_none_or(|s| !s.state.is_open()));
    let _ = std::fs::remove_file(&marker);
}

#[test]
#[ignore = "serves the user's NativeTerm pipe; needs a portable Terminal folder"]
fn dropped_sessions_reconnect_failed_logins_do_not() {
    let core = core();
    core.set_auto_reconnect(true);
    assert!(core.auto_reconnect());

    // logs in, then the connection drops (255 after login)
    let _dropped = spawn_shim(
        "ar-dropped",
        "6e7a0000-0000-4000-8000-0000000a0001",
        &[("FAKE_SSH_LOGIN", "1"), ("FAKE_SSH_CODE", "255")],
    );
    // never logs in (255 before login)
    let _failed = spawn_shim("ar-failed", "6e7a0000-0000-4000-8000-0000000a0002", &[("FAKE_SSH_CODE", "255")]);

    wait_until("the failed login is reported", Duration::from_secs(15), || {
        session(&core, "ar-failed").is_some_and(|s| matches!(s.state, State::LoginFailed(_)))
    });
    // first retry after 3 s (+ up to 2 s spread), second after 10 s more:
    // it drops right after each login, so the count doesn't start over
    let started = Instant::now();
    wait_until("the dropped session was reconnected twice", Duration::from_secs(40), || {
        session(&core, "ar-dropped").is_some_and(|s| s.attempt >= 3)
    });
    assert!(started.elapsed() >= Duration::from_secs(12), "growing delays");
    wait_until("shown as reconnecting", Duration::from_secs(15), || {
        session(&core, "ar-dropped").is_some_and(|s| s.auto_retry.is_some_and(|n| n >= 2))
    });
    let failed = session(&core, "ar-failed").unwrap();
    assert_eq!(failed.attempt, 1, "a failed login is never retried: {failed:?}");
    assert!(matches!(failed.state, State::LoginFailed(_)));

    // off: the next drop stays dropped
    core.set_auto_reconnect(false);
    wait_until("dropped again", Duration::from_secs(15), || {
        session(&core, "ar-dropped").is_some_and(|s| matches!(s.state, State::Disconnected(_)))
    });
    let attempt = session(&core, "ar-dropped").unwrap().attempt;
    std::thread::sleep(Duration::from_secs(8));
    assert_eq!(session(&core, "ar-dropped").unwrap().attempt, attempt, "no reconnect while off");

    core.close("ar-dropped");
    core.close("ar-failed");
    wait_until("both closed", Duration::from_secs(10), || {
        ["ar-dropped", "ar-failed"].iter().all(|id| session(&core, id).is_none_or(|s| !s.state.is_open()))
    });
}
