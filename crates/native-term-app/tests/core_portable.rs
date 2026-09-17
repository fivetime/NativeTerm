//! NativeTerm's core against a portable Windows Terminal (see
//! `native-term-platform/tests/portable_terminal.rs` for the setup):
//!
//! ```text
//! set NATIVETERM_TEST_WT_DIR=C:\…\terminal-1.26.2581.0
//! cargo build -p native-term-shim
//! cargo test -p native-term-app --test core_portable -- --ignored
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use native_term_app::{Core, HostRequest, SessionView, State};
use native_term_platform::windows_terminal::install::{Install, Kind};
use native_term_platform::windows_terminal::WindowsTerminal;
use native_term_platform::Target;

const WAIT: Duration = Duration::from_secs(30);

fn core() -> Core {
    let dir = std::env::var("NATIVETERM_TEST_WT_DIR").expect("set NATIVETERM_TEST_WT_DIR to a portable Terminal");
    let install = Install::from_dir(dir.as_ref()).unwrap();
    assert_eq!(install.kind, Kind::Portable, "only a portable Terminal is used for tests");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf();
    let shim = root.join("target").join("debug").join("nativeterm-shim.exe");
    assert!(shim.exists(), "build the shim first");
    Core::start(WindowsTerminal::new(install, &shim), || {}).expect("is a NativeTerm running?")
}

/// Only this test's sessions (a core adopts tabs left by earlier runs).
fn wait_until(core: &Core, ids: &[String], what: &str, check: impl Fn(&[SessionView]) -> bool) -> Vec<SessionView> {
    let started = Instant::now();
    loop {
        let sessions: Vec<SessionView> = core.sessions().into_iter().filter(|s| ids.contains(&s.id)).collect();
        if check(&sessions) {
            println!("{what}: {} ms", started.elapsed().as_millis());
            return sessions;
        }
        assert!(started.elapsed() < WAIT, "{what}: {sessions:#?}\nnotices: {:?}", core.take_notices());
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "needs a portable Windows Terminal"]
fn open_track_reconnect_close() {
    let core = core();
    let host = HostRequest { alias: "nativeterm-test.invalid".into(), label: "nt-app 测试".into() };
    let ids = core.open(&[host.clone(), host], Target::NewWindow);

    // ssh can't resolve the host: exit 255 before any login
    let sessions = wait_until(&core, &ids, "both failed to log in and were located", |s| {
        s.iter().all(|s| matches!(s.state, State::LoginFailed(255)) && s.location.is_some() && s.linked)
    });
    let labels: Vec<&str> = sessions.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["nt-app 测试", "nt-app 测试 (2)"]);
    let first = &sessions[0];
    let second = &sessions[1];
    let window = first.location.as_ref().unwrap().window;
    assert_eq!(second.location.as_ref().unwrap().window, window, "same new window");
    assert!(second.location.as_ref().unwrap().selected, "the last opened tab is selected");

    core.focus(&ids[0]);
    wait_until(&core, &ids, "first tab selected", |s| s[0].location.as_ref().is_some_and(|l| l.selected));

    // reconnect: the shim runs ssh again, which fails again
    let pid = first.shim_pid;
    core.reconnect(&ids[0]);
    wait_until(&core, &ids, "reconnected and failed again", |s| {
        s[0].attempt == 2 && matches!(s[0].state, State::LoginFailed(255)) && s[0].shim_pid == pid
    });
    let sessions = wait_until(&core, &ids, "the other one untouched", |s| s[1].attempt == 1);
    assert_eq!(sessions.len(), 2);

    core.close(&ids[0]);
    core.close(&ids[1]);
    wait_until(&core, &ids, "both closed", |s| s.iter().all(|s| s.state == State::Closed));
    let started = Instant::now();
    while core.terminal().windows().iter().any(|w| w.handle == window) {
        assert!(started.elapsed() < WAIT, "window still open");
        std::thread::sleep(Duration::from_millis(200));
    }
    wait_until(&core, &ids, "locations cleared", |s| s.iter().all(|s| s.location.is_none()));
    core.clear_finished();
    assert!(core.sessions().iter().all(|s| !ids.contains(&s.id)));
    assert!(core.take_notices().is_empty());
}
