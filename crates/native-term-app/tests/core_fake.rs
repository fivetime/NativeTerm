//! `Core` against the fake terminal: no Windows Terminal, no shims, just
//! what the program asks of a terminal and what it makes of the answers.
//! Each test serves a pipe of its own, so they run side by side and
//! beside a real NativeTerm.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use native_term_app::registry::Registry;
use native_term_app::{tab_menu, Core, HostRequest, SessionView, State, DETACHED_GRACE};
use native_term_platform::{Change, FakeBackend, Target, TerminalBackend};
use native_term_session::pipe;
use native_term_session::protocol::{AppMessage, Role, ShimMessage};

const WAIT: Duration = Duration::from_secs(10);
const HOST: &str = "nativeterm-test.invalid";

static PIPES: AtomicU32 = AtomicU32::new(0);

fn pipe_name() -> String {
    let n = PIPES.fetch_add(1, Ordering::Relaxed);
    format!("{}fake-{}-{n}", native_term_session::PIPE_NAME_PREFIX, std::process::id())
}

fn start(registry: Option<Registry>) -> (Core, FakeBackend) {
    let fake = FakeBackend::new("nativeterm-shim");
    let core = Core::start_with_pipe(fake.clone(), registry, &pipe_name()).expect("a pipe of our own");
    (core, fake)
}

/// Only these sessions, once `check` holds for them.
fn wait_until(core: &Core, ids: &[String], what: &str, check: impl Fn(&[SessionView]) -> bool) -> Vec<SessionView> {
    let started = Instant::now();
    loop {
        let sessions: Vec<SessionView> = core.sessions().into_iter().filter(|s| ids.contains(&s.id)).collect();
        if check(&sessions) {
            return sessions;
        }
        assert!(started.elapsed() < WAIT, "{what}: {sessions:#?}\nnotices: {:?}", core.take_notices());
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for(what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < WAIT, "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn located(s: &[SessionView]) -> bool {
    !s.is_empty() && s.iter().all(|s| s.location.is_some())
}

#[test]
fn opens_tabs_and_finds_them() {
    let (core, fake) = start(None);
    let host = HostRequest::new(HOST, "web01");
    let ids = core.open(&[host.clone(), host], Target::NewWindow);
    let sessions = wait_until(&core, &ids, "both located", located);
    let labels: Vec<&str> = sessions.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["web01", "web01 (2)"], "a second tab of the same host gets a unique label");
    assert!(sessions.iter().all(|s| s.state == State::Opening && !s.linked), "no shim has said hello");
    let first = sessions[0].location.as_ref().unwrap();
    let second = sessions[1].location.as_ref().unwrap();
    assert_eq!((first.window_number, first.tab_index), (1, 0));
    assert_eq!((second.window_number, second.tab_index), (1, 1));
    assert_eq!(first.window, second.window, "one new window for the batch");
    assert_eq!(core.window_number(first.window), Some(1));
    // each new tab took the selection; the batch ends on its first tab
    let sessions = wait_until(&core, &ids, "the batch's first tab selected", |s| {
        located(s) && s[0].location.as_ref().is_some_and(|l| l.selected)
    });
    assert!(!sessions[1].location.as_ref().unwrap().selected);
    assert_eq!(fake.calls().selected, [(first.window, 0)]);

    let windows = fake.windows();
    assert_eq!(windows.len(), 1);
    let titles: Vec<&str> = windows[0].tabs.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["web01", "web01 (2)"]);
    let calls = fake.calls();
    assert_eq!(calls.opened.len(), 1);
    assert_eq!(calls.opened[0].0, Target::NewWindow);
    assert_eq!(calls.opened[0].1[0].alias, HOST);
    assert!(core.take_notices().is_empty(), "nothing to complain about");
}

#[test]
fn focus_selects_the_tab() {
    let (core, fake) = start(None);
    let host = HostRequest::new(HOST, "db");
    let ids = core.open(&[host.clone(), host], Target::NewWindow);
    wait_until(&core, &ids, "both located", located);

    let sessions = wait_until(&core, &ids, "the batch's first tab selected", |s| {
        located(s) && s[0].location.as_ref().is_some_and(|l| l.selected)
    });
    let window = sessions[0].location.as_ref().unwrap().window;

    core.focus(&ids[1]);
    let sessions = wait_until(&core, &ids, "second tab selected", |s| {
        located(s) && s[1].location.as_ref().is_some_and(|l| l.selected)
    });
    assert_eq!(fake.calls().selected, [(window, 0), (window, 1)]);
    assert!(!sessions[0].location.as_ref().unwrap().selected);
}

#[test]
fn closing_a_session_without_a_shim_closes_its_tab() {
    let (core, fake) = start(None);
    let host = HostRequest::new(HOST, "app");
    let ids = core.open(&[host.clone(), host], Target::NewWindow);
    let sessions = wait_until(&core, &ids, "both located", located);
    let window = sessions[0].location.as_ref().unwrap().window;

    core.close(&ids[0]);
    wait_until(&core, &ids, "first closed", |s| s[0].state == State::Closed);
    wait_for("its tab is gone", || fake.windows()[0].tabs.len() == 1);
    assert_eq!(fake.calls().closed, [(window, 0)]);
    // the other one moved up and is found there
    let sessions =
        wait_until(&core, &ids, "second at index 0", |s| s[1].location.as_ref().is_some_and(|l| l.tab_index == 0));
    assert!(sessions[0].location.is_none(), "a closed session has no place");

    core.close(&ids[1]);
    wait_until(&core, &ids, "second closed", |s| s[1].state == State::Closed);
    wait_for("the empty window is gone", || fake.window_ids().is_empty());
}

#[test]
fn a_tab_that_never_appears_is_asked_for_again() {
    let (core, fake) = start(None);
    fake.drop_next(1);
    let host = HostRequest::new(HOST, "lost");
    let ids = core.open(&[host.clone(), host], Target::NewWindow);
    let sessions = wait_until(&core, &ids, "both located after the resend", located);

    let calls = fake.calls();
    assert_eq!(calls.opened.len(), 2, "one resend");
    let (first_target, first_specs) = &calls.opened[0];
    let (again_target, again_specs) = &calls.opened[1];
    assert_eq!(*first_target, Target::NewWindow);
    assert_eq!(*again_target, Target::Recent, "into the window the first try made");
    assert_eq!(again_specs.len(), 1);
    assert_eq!(again_specs[0].label, "lost", "the dropped one, the first of the batch");
    assert_eq!(again_specs[0].session, first_specs[0].session, "the same session");
    assert_ne!(again_specs[0].terminal_session, first_specs[0].terminal_session, "a fresh terminal GUID");
    let window = sessions[0].location.as_ref().unwrap().window;
    assert_eq!(calls.activated, [window], "the new window was brought forward first");
    assert!(sessions.iter().all(|s| s.location.as_ref().unwrap().window == window));
    let titles: Vec<String> = fake.windows()[0].tabs.iter().map(|t| t.title.clone()).collect();
    assert_eq!(titles, ["lost (2)", "lost"], "the resent tab comes last");
    assert_eq!(core.take_notices().len(), 1, "one line about the resend");
}

#[test]
fn a_change_notification_makes_it_look_again() {
    let (core, fake) = start(None);
    let ids = core.open(&[HostRequest::new(HOST, "watch")], Target::NewWindow);
    let sessions = wait_until(&core, &ids, "located", located);
    let window = sessions[0].location.as_ref().unwrap().window;
    let scans = core.scan_count();

    // the user opens a tab of their own: not noticed until Terminal says so
    fake.add_tab(window, "pwsh");
    fake.emit(Change::Tabs);
    wait_for("the new tab is in the snapshot", || {
        core.snapshot().windows.first().is_some_and(|w| w.tabs.iter().any(|t| t.name == "pwsh"))
    });
    assert!(core.scan_count() > scans);
    assert_eq!(core.change_counts(), (0, 1));
    let sessions = wait_until(&core, &ids, "still located", located);
    assert!(!sessions[0].location.as_ref().unwrap().selected, "the user's tab took the selection");

    // the user selects the session's tab again
    fake.select_tab(window, 0);
    fake.emit(Change::Content);
    wait_until(&core, &ids, "selected again", |s| s[0].location.as_ref().is_some_and(|l| l.selected));
    assert_eq!(core.change_counts(), (0, 2));
}

#[test]
fn the_active_session_follows_the_foreground_window() {
    let (core, fake) = start(None);
    let a = core.open(&[HostRequest::new(HOST, "a")], Target::NewWindow);
    wait_until(&core, &a, "a located", located);
    let b = core.open(&[HostRequest::new(HOST, "b")], Target::NewWindow);
    wait_until(&core, &b, "b located", located);
    let windows = fake.window_ids();
    assert_eq!(windows.len(), 2);

    fake.set_foreground(Some(windows[1]));
    fake.emit(Change::Foreground);
    wait_for("b is the active session", || core.active_session().is_some_and(|s| s.id == b[0]));
    fake.set_foreground(Some(windows[0]));
    fake.emit(Change::Foreground);
    wait_for("a is the active session", || core.active_session().is_some_and(|s| s.id == a[0]));
    // something else in front: the last Terminal window still counts
    fake.set_foreground(None);
    fake.emit(Change::Foreground);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(core.active_session().map(|s| s.id), Some(a[0].clone()));
    assert_eq!(fake.subscribers(), 1);
}

#[test]
fn a_window_closed_by_the_user_loses_its_sessions_places() {
    let (core, fake) = start(None);
    let ids = core.open(&[HostRequest::new(HOST, "gone")], Target::NewWindow);
    let sessions = wait_until(&core, &ids, "located", located);
    let window = sessions[0].location.as_ref().unwrap().window;
    fake.close_window(window);
    fake.emit(Change::Windows);
    wait_until(&core, &ids, "no place any more", |s| s[0].location.is_none());
    assert_eq!(core.change_counts(), (1, 0));
    assert_eq!(core.window_number(window), None, "the window number is freed");
    assert_eq!(core.snapshot().windows.len(), 0);
}

#[test]
fn sessions_of_the_last_run_are_offered_back() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let (core, _fake) = start(Some(Registry::open(&db).unwrap()));
    let ids = core.open(&[HostRequest::new(HOST, "yesterday")], Target::NewWindow);
    wait_until(&core, &ids, "located", located);
    drop(core);

    // the next run: the record is there, its shim never says hello
    let (core, fake) = start(Some(Registry::open(&db).unwrap()));
    assert_eq!(fake.windows().len(), 0, "a fresh terminal, the tab is gone");
    let sessions = wait_until(&core, &ids, "known as detached", |s| s.len() == 1 && s[0].state == State::Detached);
    assert_eq!(sessions[0].label, "yesterday");
    assert!(core.lost_at_start().is_empty(), "given time to come back first");

    let started = Instant::now();
    while core.lost_at_start().is_empty() {
        assert!(started.elapsed() < DETACHED_GRACE + WAIT, "never given up");
        std::thread::sleep(Duration::from_millis(200));
    }
    let lost = core.lost_at_start();
    assert_eq!(lost.len(), 1);
    assert_eq!((lost[0].alias.as_str(), lost[0].label.as_str()), (HOST, "yesterday"));
    assert!(started.elapsed() >= DETACHED_GRACE - Duration::from_secs(1));
    let sessions = wait_until(&core, &ids, "gone", |s| s[0].state == State::Gone);
    assert!(!sessions[0].state.is_open());

    core.forget_lost(true);
    assert!(core.lost_at_start().is_empty());
    assert!(core.sessions().is_empty(), "cleared with the finished ones");
}

/// A terminal that shows NativeTerm's tab menu itself (WezTerm's picker)
/// asks over the pipe what is on it, and reports the choice the same
/// way: the shim's `--tab-menu` helper, as a `Request` connection.
#[test]
fn a_terminal_that_shows_the_menu_itself_gets_it_over_the_pipe() {
    let name = pipe_name();
    // only the terminal's own shim may speak on the pipe: here, this test
    let fake = FakeBackend::new(std::env::current_exe().unwrap());
    let core = Core::start_with_pipe(fake.clone(), None, &name).expect("a pipe of our own");
    core.start_tab_menu(|_| {}).unwrap();
    let ids = core.open(&[HostRequest::new(HOST, "menu")], Target::NewWindow);
    wait_until(&core, &ids, "located", located);
    // the tab's terminal session id: what the shim in it would report
    let pane = fake.calls().opened[0].1[0].terminal_session.clone();

    let helper = |request: ShimMessage| {
        let conn = pipe::connect(&name, Duration::from_secs(2)).expect("the pipe answers");
        let hello = ShimMessage::Hello {
            protocol: native_term_session::PROTOCOL_VERSION,
            role: Role::Request,
            pid: std::process::id(),
            wt_session: Some(pane.clone()),
            session: None,
            alias: None,
            terminal_window: None,
        };
        conn.send(&hello).unwrap();
        conn.send(&request).unwrap();
        conn
    };
    let conn = helper(ShimMessage::TabMenu);
    assert!(matches!(conn.recv::<AppMessage>(Duration::from_secs(2)), Ok(Some(AppMessage::Welcome { .. }))));
    let Ok(Some(AppMessage::TabMenu { items })) = conn.recv::<AppMessage>(Duration::from_secs(2)) else {
        panic!("no menu came back");
    };
    assert_eq!((items[0].id, items[0].text.as_str()), (0, "menu"), "headed by the session's label: {items:?}");
    assert!(items.iter().any(|i| i.id == tab_menu::CLOSE), "closing is always offered: {items:?}");
    assert!(items.iter().all(|i| i.separator || !i.text.is_empty()));
    // icons for WezTerm to draw, separators only between items
    assert!(items.iter().filter(|i| i.id != 0 && !i.separator).all(|i| i.icon.is_some()), "{items:?}");
    assert!(!items.last().unwrap().separator && !items[1].separator, "{items:?}");
    assert!(items.windows(2).all(|w| !(w[0].separator && w[1].separator)), "{items:?}");

    // a tab nobody claims gets no menu
    let conn = pipe::connect(&name, Duration::from_secs(2)).unwrap();
    conn.send(&ShimMessage::Hello {
        protocol: native_term_session::PROTOCOL_VERSION,
        role: Role::Request,
        pid: std::process::id(),
        wt_session: Some("no-such-pane".into()),
        session: None,
        alias: None,
        terminal_window: None,
    })
    .unwrap();
    conn.send(&ShimMessage::TabMenu).unwrap();
    let _ = conn.recv::<AppMessage>(Duration::from_secs(2));
    assert!(
        matches!(conn.recv::<AppMessage>(Duration::from_secs(2)), Ok(Some(AppMessage::TabMenu { items })) if items.is_empty())
    );

    // the choice closes the session, and its tab with it
    let _conn = helper(ShimMessage::TabAction { id: tab_menu::CLOSE });
    wait_until(&core, &ids, "closed from the menu", |s| s[0].state == State::Closed);
    wait_for("its tab is gone", || fake.window_ids().is_empty());
}
