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
    Core::start(WindowsTerminal::new(install, &shim), None).expect("is a NativeTerm running?")
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
    let host = HostRequest::new("nativeterm-test.invalid", "nt-app 测试");
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
    // each new tab takes the focus; the batch ends on its first tab
    wait_until(&core, &ids, "the batch's first tab selected", |s| s[0].location.as_ref().is_some_and(|l| l.selected));

    // selected behind NativeTerm's back: the selection event updates it,
    // long before the 15 s fallback scan
    let labels = ["nt-app 测试".to_string(), "nt-app 测试 (2)".to_string()].into_iter().collect();
    let snapshot = core.terminal().snapshot(&labels);
    let (w, tab) = snapshot.find("nt-app 测试 (2)").unwrap();
    assert!(core.terminal().select(w.handle, tab).unwrap());
    let started = Instant::now();
    wait_until(&core, &ids, "second tab selected, noticed by event", |s| {
        s[1].location.as_ref().is_some_and(|l| l.selected)
    });
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert!(core.change_counts().1 > 0);

    core.focus(&ids[0]);
    wait_until(&core, &ids, "first tab selected again", |s| s[0].location.as_ref().is_some_and(|l| l.selected));

    // reconnect: the shim runs ssh again, which fails again
    let pid = first.shim_pid;
    core.connect(&ids[0]);
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

    // a bigger batch: the tabs wait, and the queue connects them one by one
    let host = HostRequest::new("nativeterm-test.invalid", "nt-queue");
    let started = Instant::now();
    let ids = core.open(&vec![host; 6], Target::NewWindow);
    let sessions = wait_until(&core, &ids, "all six connected through the queue and failed", |s| {
        s.len() == 6 && s.iter().all(|s| matches!(s.state, State::LoginFailed(255)) && s.attempt == 1)
    });
    // six starts, 200 ms apart
    assert!(started.elapsed() >= Duration::from_millis(1000), "{:?}", started.elapsed());
    let window = sessions[0].location.as_ref().unwrap().window;
    wait_until(&core, &ids, "the batch's first tab selected", |s| s[0].location.as_ref().is_some_and(|l| l.selected));

    // "connect all" goes through the same queue
    core.connect_all(ids.clone());
    wait_until(&core, &ids, "all reconnected once", |s| {
        s.iter().all(|s| s.attempt == 2 && matches!(s.state, State::LoginFailed(255)))
    });
    for id in &ids {
        core.close(id);
    }
    wait_until(&core, &ids, "all closed", |s| s.iter().all(|s| s.state == State::Closed));
    let started = Instant::now();
    while core.terminal().windows().iter().any(|w| w.handle == window) {
        assert!(started.elapsed() < WAIT, "window still open");
        std::thread::sleep(Duration::from_millis(200));
    }
}
