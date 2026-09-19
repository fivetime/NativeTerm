//! Live test against a portable Windows Terminal (never the user's own):
//!
//! ```text
//! set NATIVETERM_TEST_WT_DIR=C:\…\terminal-1.26.2581.0
//! cargo build -p native-term-shim
//! cargo test -p native-term-platform --test portable_terminal -- --ignored --test-threads=1
//! ```
//!
//! The Terminal's `settings.json` must contain the "NativeTerm SSH" profile
//! pointing at `target\debug\nativeterm-shim.exe`, and no NativeTerm may be
//! running (the test serves the real pipe). Tabs connect to an unresolvable
//! host, so ssh fails at once and the shims wait at their prompt.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use native_term_platform::windows_terminal::install::{Install, Kind};
use native_term_platform::windows_terminal::{command, launch, WindowsTerminal};
use native_term_platform::{TabSpec, Target};
use native_term_session::pipe::{self, PipeConnection, PipeListener};
use native_term_session::protocol::{AppMessage, Role, ShimMessage};

const HOST: &str = "nativeterm-test.invalid";
const WAIT: Duration = Duration::from_secs(20);

fn terminal() -> WindowsTerminal {
    let dir = std::env::var("NATIVETERM_TEST_WT_DIR").expect("set NATIVETERM_TEST_WT_DIR to a portable Terminal");
    let install = Install::from_dir(dir.as_ref()).unwrap();
    assert_eq!(install.kind, Kind::Portable, "only a portable Terminal is used for tests");
    let settings = std::fs::read_to_string(install.settings_json()).unwrap();
    assert!(
        settings.contains(command::PROFILE_NAME),
        "add the NativeTerm SSH profile to {}",
        install.settings_json().display()
    );
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf();
    let shim = root.join("target").join("debug").join("nativeterm-shim.exe");
    assert!(shim.exists(), "build the shim first: {}", shim.display());
    WindowsTerminal::new(install, &shim)
}

struct Shim {
    conn: Arc<PipeConnection>,
    wt_session: Option<String>,
    session: Option<String>,
    alias: Option<String>,
}

/// Serves the real pipe once per test process; every shim that says
/// hello is passed on. Tests run one at a time and share it.
fn serve() -> MutexGuard<'static, Receiver<Shim>> {
    static SERVER: OnceLock<Mutex<Receiver<Shim>>> = OnceLock::new();
    let server = SERVER.get_or_init(|| {
        let rx = start_server();
        // shims left by an earlier, failed run reconnect within 2 s: close them
        let mut strays = Vec::new();
        while let Ok(stray) = rx.recv_timeout(Duration::from_secs(3)) {
            println!("closing a stray test tab {:?}", stray.wt_session);
            let _ = stray.conn.send(&AppMessage::Close);
            strays.push(stray);
        }
        Mutex::new(rx)
    });
    let rx = server.lock().unwrap_or_else(|e| e.into_inner());
    while rx.try_recv().is_ok() {}
    rx
}

fn start_server() -> Receiver<Shim> {
    let mut listener = PipeListener::bind(&pipe::pipe_name().unwrap()).expect("is a NativeTerm running?");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || loop {
        let Ok(conn) = listener.accept() else { return };
        let conn = Arc::new(conn);
        let tx = tx.clone();
        std::thread::spawn(move || {
            if let Ok(Some(ShimMessage::Hello { role: Role::Shim, wt_session, session, alias, .. })) =
                conn.recv::<ShimMessage>(WAIT)
            {
                let _ = conn.send(&AppMessage::Welcome { protocol: 1 });
                let _ = tx.send(Shim { conn, wt_session, session, alias });
            }
        });
    });
    rx
}

/// The hellos of `tabs`' shims, by terminal session GUID.
fn hellos(rx: &Receiver<Shim>, tabs: &[TabSpec]) -> OpenTabs {
    let wanted: HashSet<&str> = tabs.iter().map(|t| t.terminal_session.as_str()).collect();
    let deadline = Instant::now() + WAIT;
    let mut out = HashMap::new();
    while out.len() < tabs.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        let shim =
            rx.recv_timeout(left).unwrap_or_else(|_| panic!("only {} of {} shims said hello", out.len(), tabs.len()));
        let key = shim.wt_session.clone().unwrap_or_default().to_lowercase();
        if wanted.contains(key.as_str()) {
            out.insert(key, shim);
        } else {
            println!("ignoring hello from {key}");
        }
    }
    OpenTabs(out)
}

fn guid(n: u32) -> String {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    format!("6e7a{:04x}-0000-4000-8000-{:012x}", n, stamp & 0xffff_ffff_ffff)
}

fn spec(n: u32, label: &str) -> TabSpec {
    TabSpec {
        terminal_session: guid(n),
        label: label.to_string(),
        session: format!("s-{n}"),
        alias: HOST.to_string(),
        wait: false,
        no_forwards: false,
        tab_color: None,
    }
}

/// Closes its tabs when dropped, also when an assertion failed.
struct OpenTabs(HashMap<String, Shim>);

impl Drop for OpenTabs {
    fn drop(&mut self) {
        for shim in self.0.values() {
            let _ = shim.conn.send(&AppMessage::Close);
        }
        // keep the connections until the shims have read it
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn wait_gone(wt: &WindowsTerminal, window: isize) {
    let deadline = Instant::now() + WAIT;
    while wt.windows().iter().any(|w| w.handle == window) {
        assert!(Instant::now() < deadline, "window {window} still open");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
#[ignore = "needs a portable Windows Terminal, see the file header"]
fn open_claim_select_close() {
    let wt = terminal();
    let rx = serve();

    // odd labels: a `;`, a leading `-`, CJK and spaces
    let tabs = vec![spec(1, "nt-it web01"), spec(2, "nt-it db; prod"), spec(3, "-nt-it 中文 主机")];
    let labels: HashSet<String> = tabs.iter().map(|t| t.label.clone()).collect();
    let expected: Vec<String> = tabs.iter().map(|t| t.label.clone()).collect();

    let started = Instant::now();
    let report = wt.open(&Target::NewWindow, &tabs).unwrap();
    assert_eq!(report.launched, 3);
    assert!(report.pending.is_empty());
    let window = report.window.expect("a new window");
    let (snapshot, missing) = wt.wait_for(&labels, &expected, WAIT);
    assert!(missing.is_empty(), "missing {missing:?} in {snapshot:#?}");
    println!("3 tabs claimed after {} ms", started.elapsed().as_millis());
    for label in &expected {
        let (w, tab) = snapshot.find(label).unwrap();
        assert_eq!(w.handle, window, "{label} is in the new window");
        assert_eq!(&tab.name, label, "title kept");
    }

    // the shims know their tab
    let shims = hellos(&rx, &tabs);
    for t in &tabs {
        let shim = shims.0.get(&t.terminal_session).unwrap_or_else(|| panic!("no shim for {}", t.terminal_session));
        assert_eq!(shim.session.as_deref(), Some(t.session.as_str()));
        assert_eq!(shim.alias.as_deref(), Some(HOST));
    }

    // select the first tab (the last opened is selected)
    let (w, first) = snapshot.find(&expected[0]).unwrap();
    assert!(!first.selected);
    assert!(wt.select(w.handle, first).unwrap());
    let (snapshot, _) = wt.wait_for(&labels, &expected, WAIT);
    assert!(snapshot.find(&expected[0]).unwrap().1.selected, "{snapshot:#?}");

    // a tab that changed since the snapshot isn't touched
    let mut stale = snapshot.find(&expected[1]).unwrap().1.clone();
    stale.name = "something else".into();
    assert!(!wt.select(window, &stale).unwrap());

    // a placeholder from the profile (restored/duplicated pane): no host
    launch::run(
        &wt.install().launcher,
        &["-w".into(), "0".into(), "new-tab".into(), "--profile".into(), command::PROFILE_NAME.into()],
    )
    .unwrap();
    let placeholder = loop {
        let shim = rx.recv_timeout(WAIT).expect("placeholder hello");
        if shim.alias.is_none() {
            break shim;
        }
    };
    // answer within the shim's grace period, or it starts a local shell
    placeholder.conn.send(&AppMessage::Close).unwrap();
    assert_eq!(placeholder.session, None);
    assert!(placeholder.wt_session.is_some());

    // closing through the shims closes the tabs and the window
    drop(shims);
    wait_gone(&wt, window);
    let snapshot = wt.snapshot(&labels);
    assert_eq!(snapshot.claimed().count(), 0);
    assert!(snapshot.complete);
    assert_eq!(wt.abandoned_workers(), 0);
}

#[test]
#[ignore = "needs a portable Windows Terminal, see the file header"]
fn batches_through_the_shell() {
    let wt = terminal();
    let rx = serve();
    // long labels force several `wt` calls; launched like an elevated NativeTerm would
    std::env::set_var(launch::VIA_SHELL_ENV, "1");
    let tabs: Vec<TabSpec> = (0..30).map(|n| spec(100 + n, &format!("nt-batch {n:02} {}", "x".repeat(900)))).collect();
    let calls = command::batches(&Target::NewWindow, &tabs, std::path::Path::new(r"C:\x"), &[]).len();
    assert!(calls >= 2, "{calls}");
    let labels: HashSet<String> = tabs.iter().map(|t| t.label.clone()).collect();
    let expected: Vec<String> = tabs.iter().map(|t| t.label.clone()).collect();

    let report = wt.open(&Target::NewWindow, &tabs).unwrap();
    std::env::remove_var(launch::VIA_SHELL_ENV);
    let window = report.window.expect("a new window");
    assert!(report.pending.is_empty(), "{} pending: was the new window left?", report.pending.len());
    let shims = hellos(&rx, &tabs);
    let (snapshot, missing) = wt.wait_for(&labels, &expected, WAIT);
    assert!(missing.is_empty(), "missing {} tabs", missing.len());
    let windows: HashSet<isize> = expected.iter().map(|l| snapshot.find(l).unwrap().0.handle).collect();
    assert_eq!(windows, HashSet::from([window]), "all batches in the new window");
    // strip order is the requested order
    let order: Vec<&str> =
        snapshot.windows.iter().find(|w| w.handle == window).unwrap().tabs.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(order, expected.iter().map(String::as_str).collect::<Vec<_>>());

    drop(shims);
    wait_gone(&wt, window);
}
