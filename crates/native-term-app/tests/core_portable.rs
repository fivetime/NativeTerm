#![cfg(windows)]
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
use native_term_platform::windows_terminal::command;
use native_term_platform::windows_terminal::install::{Install, Kind};
use native_term_platform::windows_terminal::WindowsTerminal;
use native_term_platform::{Target, WindowId};

const WAIT: Duration = Duration::from_secs(30);

/// The test host does not resolve, so the shim reports a server it never
/// reached (`Unreachable`), not a login that failed.
const FAILED: State = State::Unreachable(255);

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
        s.iter().all(|s| matches!(s.state, FAILED) && s.location.is_some() && s.linked)
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
        s[0].attempt == 2 && matches!(s[0].state, FAILED) && s[0].shim_pid == pid
    });
    let sessions = wait_until(&core, &ids, "the other one untouched", |s| s[1].attempt == 1);
    assert_eq!(sessions.len(), 2);

    core.close(&ids[0]);
    core.close(&ids[1]);
    wait_until(&core, &ids, "both closed", |s| s.iter().all(|s| s.state == State::Closed));
    let started = Instant::now();
    while core.terminal().window_ids().contains(&window) {
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
        s.len() == 6 && s.iter().all(|s| matches!(s.state, FAILED) && s.attempt == 1)
    });
    // six starts, 200 ms apart
    assert!(started.elapsed() >= Duration::from_millis(1000), "{:?}", started.elapsed());
    let window = sessions[0].location.as_ref().unwrap().window;
    wait_until(&core, &ids, "the batch's first tab selected", |s| s[0].location.as_ref().is_some_and(|l| l.selected));

    // "connect all" goes through the same queue
    core.connect_all(ids.clone());
    wait_until(&core, &ids, "all reconnected once", |s| s.iter().all(|s| s.attempt == 2 && matches!(s.state, FAILED)));
    for id in &ids {
        core.close(id);
    }
    wait_until(&core, &ids, "all closed", |s| s.iter().all(|s| s.state == State::Closed));
    let started = Instant::now();
    while core.terminal().window_ids().contains(&window) {
        assert!(started.elapsed() < WAIT, "window still open");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A tab that never turns up is asked for once more, under a new terminal
/// GUID: the same session, the same label, one tab in the end. A test hook
/// drops the first `wt` command for the label (`SWALLOW_ENV`), as a busy
/// Terminal that swallows a command would.
#[test]
#[ignore = "needs a portable Windows Terminal"]
fn a_tab_that_never_appeared_is_asked_for_again() {
    let core = core();
    // a window to open into: `-w 0` needs one
    let host = HostRequest::new("nativeterm-test.invalid", "nt-lost window");
    let first = core.open(&[host], Target::NewWindow);
    let opened = wait_until(&core, &first, "the window is there", |s| s.iter().all(|s| s.location.is_some()));
    let window = opened[0].location.as_ref().unwrap().window;
    let guid_before = core.sessions().into_iter().find(|s| s.id == first[0]).map(|s| s.id.clone());
    assert!(guid_before.is_some());

    std::env::set_var(command::SWALLOW_ENV, "nt-lost tab");
    let ids = core.open(&[HostRequest::new("nativeterm-test.invalid", "nt-lost tab")], Target::Recent);
    // the first try is swallowed; the second one opens it (CONFIRM apart)
    let sessions = wait_until(&core, &ids, "the tab arrived on the second try", |s| {
        s.len() == 1 && matches!(s[0].state, FAILED) && s[0].location.is_some() && s[0].linked
    });
    std::env::remove_var(command::SWALLOW_ENV);
    assert_eq!(sessions[0].location.as_ref().unwrap().window, window, "the same window, not one of its own");
    let names = tab_names(&core, window);
    assert_eq!(names.iter().filter(|n| *n == "nt-lost tab").count(), 1, "one tab, not two: {names:?}");
    let notices = core.take_notices();
    assert!(notices.iter().any(|n| n.contains("nt-lost tab")), "said once: {notices:?}");

    for id in first.iter().chain(ids.iter()) {
        core.close(id);
    }
    let all: Vec<String> = first.iter().chain(ids.iter()).cloned().collect();
    wait_until(&core, &all, "both closed", |s| s.iter().all(|s| s.state == State::Closed));
}

/// Only NativeTerm's own helper may speak on the pipe: this test program
/// is not it, so its Hello is refused (see "Named pipe access").
#[test]
#[ignore = "needs a portable Windows Terminal"]
fn a_program_that_is_not_the_helper_is_refused() {
    let core = core();
    let name = native_term_session::pipe::pipe_name().unwrap();
    let conn = native_term_session::pipe::connect(&name, Duration::from_secs(2)).expect("the pipe is there");
    let hello = native_term_session::protocol::ShimMessage::Hello {
        protocol: native_term_session::PROTOCOL_VERSION,
        role: native_term_session::protocol::Role::Shim,
        pid: std::process::id(),
        wt_session: None,
        session: Some("nt-not-a-shim".to_string()),
        alias: Some("nativeterm-test.invalid".to_string()),
        terminal_window: None,
    };
    let _ = conn.send(&hello);
    let answer = conn.recv::<native_term_session::protocol::AppMessage>(Duration::from_secs(2));
    assert!(matches!(answer, Ok(None) | Err(_)), "a stranger was welcomed onto the pipe: {answer:?}");
    assert!(!core.sessions().iter().any(|s| s.id == "nt-not-a-shim"), "and no session was made for it");
    let started = Instant::now();
    let told = loop {
        let notices = core.take_notices();
        if notices.iter().any(|n| n.contains("core_portable")) {
            break true;
        }
        if started.elapsed() > Duration::from_secs(5) {
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(told, "the user is told which program it was");
}

/// Tab titles of a window, now.
fn tab_names(core: &Core, window: WindowId) -> Vec<String> {
    let snapshot = core.terminal().snapshot(&Default::default());
    snapshot
        .windows
        .iter()
        .find(|w| w.handle == window)
        .map(|w| w.tabs.iter().map(|t| t.name.clone()).collect())
        .unwrap_or_default()
}

/// "Close NativeTerm's tabs when it exits": only unlocked NativeTerm
/// sessions are told to close; a tab NativeTerm doesn't manage in the same
/// window, and the window itself, stay.
#[test]
#[ignore = "needs a portable Windows Terminal"]
fn close_all_spares_locked_and_foreign_tabs() {
    let core = core();
    // a tab that isn't NativeTerm's, in a window of its own
    let dir = PathBuf::from(std::env::var("NATIVETERM_TEST_WT_DIR").unwrap());
    let window_name = format!("nt-closeall-{}", std::process::id());
    let foreign = format!("nt-foreign-{}", std::process::id());
    std::process::Command::new(dir.join("wt.exe"))
        .args(["-w", &window_name, "new-tab", "--title", &foreign, "--suppressApplicationTitle", "cmd.exe", "/k"])
        .status()
        .unwrap();
    let started = Instant::now();
    let window = loop {
        let found = core.terminal().window_ids().into_iter().find(|w| tab_names(&core, *w).contains(&foreign));
        if let Some(w) = found {
            break w;
        }
        assert!(started.elapsed() < WAIT, "the foreign tab didn't appear");
        std::thread::sleep(Duration::from_millis(200));
    };
    // two sessions in the same named window
    let host = HostRequest::new("nativeterm-test.invalid", "nt-closeall");
    let ids = core.open(&[host.clone(), host], Target::Named(window_name.clone()));
    let sessions = wait_until(&core, &ids, "both failed to log in and were located", |s| {
        s.len() == 2 && s.iter().all(|s| matches!(s.state, FAILED) && s.location.is_some() && s.linked)
    });
    assert!(sessions.iter().all(|s| s.location.as_ref().unwrap().window == window), "in the foreign tab's window");
    core.set_locked(&ids[1], true);

    let told = core.close_all();
    assert!(told >= 1, "{told}");
    let started = Instant::now();
    while tab_names(&core, window).contains(&"nt-closeall".to_string()) {
        assert!(started.elapsed() < WAIT, "the unlocked tab is still open: {:?}", tab_names(&core, window));
        std::thread::sleep(Duration::from_millis(200));
    }
    let names = tab_names(&core, window);
    assert!(names.contains(&"nt-closeall (2)".to_string()), "the locked one stays: {names:?}");
    assert!(names.contains(&foreign), "the foreign tab stays: {names:?}");
    assert!(core.terminal().window_ids().contains(&window), "the window stays");

    // clean up: the locked session, then the foreign tab (this test's own)
    core.set_locked(&ids[1], false);
    core.close(&ids[1]);
    wait_until(&core, &ids, "the locked one closed after unlocking", |s| s.iter().all(|s| !s.state.is_open()));
    let snapshot = core.terminal().snapshot(&Default::default());
    if let Some(w) = snapshot.windows.iter().find(|w| w.handle == window) {
        if let Some(tab) = w.tabs.iter().find(|t| t.name == foreign) {
            let _ = core.terminal().close(window, tab);
        }
    }
}
