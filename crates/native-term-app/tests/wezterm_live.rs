#![cfg(unix)]
//! `Core` with the WezTerm backend and real shims, on a Unix desktop:
//! tabs open in WezTerm running `nativeterm-shim`, the shims say hello
//! over the socket, ssh fails to resolve the test host, and the sessions
//! are found, focused, read and closed. Needs `wezterm` on `PATH`, the
//! desktop's `DISPLAY`, a built shim, and no NativeTerm running:
//!
//! ```text
//! cargo build -p native-term-shim
//! DISPLAY=:0 cargo test -p native-term-app --test wezterm_live -- --ignored --nocapture --exact <test>
//! ```
//!
//! One test per process: each starts a `Core` on the user's socket, and
//! the socket stays served until the process ends. The login test needs
//! `ssh 127.0.0.1` to log in with a key and no prompt (the user's own key
//! in their `authorized_keys`).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use native_term_app::{Core, HostRequest, SessionView, State};
use native_term_platform::{Target, WindowId};
use native_term_wezterm::{cli, WezTerm};

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

/// An ssh folder of the test's own: one host, this machine, with the
/// NativeTerm keys ssh is told to ignore.
fn local_ssh_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nativeterm-live-ssh-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let user = std::env::var("USER").unwrap_or_default();
    let config = format!(
        "IgnoreUnknown NativeTerm*\n\nHost nt-local\n    HostName 127.0.0.1\n    User {user}\n    \
         StrictHostKeyChecking accept-new\n    NativeTermLabel local\n"
    );
    std::fs::write(dir.join("config"), config).unwrap();
    dir
}

/// The pane of the tab at `index` in `window`, straight from wezterm.
fn pane_of(window: WindowId, index: usize) -> u64 {
    let out = std::process::Command::new("wezterm").args(cli::list_args()).output().unwrap();
    let windows = cli::parse_list(&String::from_utf8_lossy(&out.stdout)).unwrap();
    let w = windows.iter().find(|w| w.window_id == window.0).expect("the window is listed");
    w.tabs[index].active_pane().expect("a pane")
}

/// A session that logs in: the login signal, text typed through the
/// terminal, disconnect and reconnect, and the tab closed from the
/// terminal's side.
#[test]
#[ignore = "needs WezTerm on a desktop, a built shim, and ssh to 127.0.0.1 with a key"]
fn login_type_reconnect_and_close_from_the_terminal() {
    let shim = built_shim();
    assert!(shim.exists(), "build the shim first: {}", shim.display());
    let ssh_dir = local_ssh_dir();
    let backend = WezTerm::new(None, &shim).with_ssh_dir(&ssh_dir);
    let core = Core::start(backend, None).expect("is a NativeTerm running?");
    let ids = core.open(&[HostRequest::new("nt-local", "local")], Target::NewWindow);

    // the LocalCommand helper reports the login through the FIFO
    let sessions = wait_until(&core, &ids, "logged in and located", |s| {
        s.len() == 1 && s[0].linked && s[0].state == State::Connected && s[0].location.is_some()
    });
    let window = sessions[0].location.as_ref().unwrap().window;
    assert_eq!(sessions[0].attempt, 1);

    // typed through wezterm, run by the shell on the other side
    let report = core.send_text(&ids, "echo nt-live-marker-$((40+2))", true);
    assert_eq!(report.sent, ["local"], "{report:?}");
    let started = Instant::now();
    loop {
        let lines = core.terminal().screen_text(window, 80).unwrap_or_default();
        if lines.iter().any(|l| l.trim() == "nt-live-marker-42") {
            break;
        }
        assert!(started.elapsed() < WAIT, "the marker never showed: {lines:?}");
        std::thread::sleep(Duration::from_millis(200));
    }

    // disconnect (ssh ends, the shim stays), then connect again
    core.disconnect(&ids[0]);
    wait_until(&core, &ids, "disconnected", |s| s[0].state.can_connect());
    core.connect(&ids[0]);
    wait_until(&core, &ids, "logged in again", |s| s[0].state == State::Connected && s[0].attempt == 2);

    // the user closes the tab in wezterm: the shim gets SIGHUP and says so
    let index = core.sessions().into_iter().find(|s| s.id == ids[0]).unwrap().location.unwrap().tab_index;
    let pane = pane_of(window, index);
    let killed = std::process::Command::new("wezterm").args(cli::kill_pane_args(pane)).status().unwrap();
    assert!(killed.success());
    wait_until(&core, &ids, "closed with its tab", |s| s[0].state == State::Closed);
    let started = Instant::now();
    while core.terminal().window_ids().contains(&window) {
        assert!(started.elapsed() < WAIT, "the window stays");
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = std::fs::remove_dir_all(&ssh_dir);
}
