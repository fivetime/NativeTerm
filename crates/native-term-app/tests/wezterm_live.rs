#![cfg(unix)]
//! `Core` with the WezTerm backend and real shims, on a Unix desktop:
//! tabs open in WezTerm running `nativeterm-shim`, the shims say hello
//! over the socket, ssh fails to resolve the test host, and the sessions
//! are found, focused, read and closed. Needs `wezterm` on `PATH`, the
//! desktop's `DISPLAY`, a built shim, and no NativeTerm running:
//!
//! ```text
//! cargo build -p native-term-shim
//! DISPLAY=:0 cargo test -p native-term-app --test wezterm_live -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use native_term_app::{Core, HostRequest, SessionView, State};
use native_term_platform::Target;
use native_term_wezterm::WezTerm;

const WAIT: Duration = Duration::from_secs(30);
const HOST: &str = "nativeterm-test.invalid";

fn ended(state: &State) -> bool {
    matches!(state, State::Unreachable(255) | State::LoginFailed(255))
}

fn built_shim() -> PathBuf {
    let exe = std::env::current_exe().expect("this test binary");
    let debug = exe.parent().and_then(std::path::Path::parent).expect("target/debug/deps/<test>");
    debug.join("nativeterm-shim")
}

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
#[ignore = "needs WezTerm on a desktop and a built shim"]
fn open_find_focus_read_close() {
    let shim = built_shim();
    assert!(shim.exists(), "build the shim first: {}", shim.display());
    let backend = WezTerm::new(None, &shim);
    assert!(backend.available(), "no wezterm on PATH");
    let core = Core::start(backend, None).expect("is a NativeTerm running?");
    let host = HostRequest::new(HOST, "nt-wez 测试");
    let ids = core.open(&[host.clone(), host], Target::NewWindow);

    // the shims say hello over the socket; ssh can't resolve the host
    let sessions = wait_until(&core, &ids, "both linked, failed and located", |s| {
        s.len() == 2 && s.iter().all(|s| s.linked && ended(&s.state) && s.location.is_some())
    });
    let labels: Vec<&str> = sessions.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["nt-wez 测试", "nt-wez 测试 (2)"]);
    let window = sessions[0].location.as_ref().unwrap().window;
    assert_eq!(sessions[1].location.as_ref().unwrap().window, window, "one new window");
    assert!(sessions.iter().all(|s| s.shim_pid.is_some()));

    // the batch ends on its first tab; focus picks the second
    wait_until(&core, &ids, "first tab selected", |s| s[0].location.as_ref().is_some_and(|l| l.selected));
    core.focus(&ids[1]);
    wait_until(&core, &ids, "second tab selected", |s| s[1].location.as_ref().is_some_and(|l| l.selected));

    // the tab's screen, read through wezterm: the shim's own words are on it
    let lines = core.terminal().screen_text(window, 50).expect("the screen");
    assert!(lines.iter().any(|l| l.contains("NativeTerm")), "{lines:?}");

    core.close(&ids[0]);
    core.close(&ids[1]);
    wait_until(&core, &ids, "both closed", |s| s.iter().all(|s| s.state == State::Closed));
    let started = Instant::now();
    while core.terminal().window_ids().contains(&window) {
        assert!(started.elapsed() < WAIT, "the window stays");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(core.take_notices().is_empty(), "nothing to complain about");
}
