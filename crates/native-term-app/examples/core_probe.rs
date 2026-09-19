//! Test driver: one NativeTerm core run against a portable Terminal, with
//! a `state.db`, doing one thing and printing the sessions. Several runs in
//! a row are NativeTerm restarts (see `tests/restore_portable.rs`).
//!
//! core_probe --terminal-dir <dir> --data-dir <dir> <command>
//!   open [--new-window] <label>...  open tabs to an unresolvable host
//!   watch <secs>                    just run (restored tabs get replaced)
//!   connect <label>                 connect a waiting/ended session
//!   close-all                       close every session with a tab
//!   exit-closing                    exit like NativeTerm with "close
//!                                   tabs on exit" (on by default)
//!   restore-terminal                start the Terminal without arguments

use std::path::PathBuf;
use std::time::{Duration, Instant};

use native_term_app::registry::Registry;
use native_term_app::{Core, HostRequest, SessionView, State};
use native_term_platform::windows_terminal::install::Install;
use native_term_platform::windows_terminal::{launch, WindowsTerminal};
use native_term_platform::Target;

const HOST: &str = "nativeterm-test.invalid";

fn wait_until(core: &Core, secs: u64, check: impl Fn(&[SessionView]) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if check(&core.sessions()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn print(core: &Core) {
    for s in core.sessions() {
        let (window, tab) = s.location.as_ref().map_or((0, -1), |l| (l.window_number, l.tab_index as i64));
        println!(
            "SESSION label={:?} state={:?} linked={} window={} tab={} attempt={}",
            s.label, s.state, s.linked, window, tab, s.attempt
        );
    }
    for w in core.snapshot().windows {
        let names: Vec<&str> = w.tabs.iter().map(|t| t.name.as_str()).collect();
        println!("WINDOW {} tabs={:?}", w.handle, names);
    }
    let (windows, tabs) = core.change_counts();
    println!("CHANGES windows={windows} tabs={tabs} scans={}", core.scan_count());
    for n in core.take_notices() {
        println!("NOTICE {n}");
    }
}

fn main() {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut take = |flag: &str| {
        let i = args.iter().position(|a| a == flag)?;
        args.remove(i);
        Some(args.remove(i))
    };
    let terminal_dir = PathBuf::from(take("--terminal-dir").expect("--terminal-dir"));
    let data_dir = PathBuf::from(take("--data-dir").expect("--data-dir"));
    let install = Install::from_dir(&terminal_dir).expect("terminal");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf();
    let shim = root.join("target").join("debug").join("nativeterm-shim.exe");

    if args.first().map(String::as_str) == Some("restore-terminal") {
        launch::run(&install.launcher, &[]).expect("start Terminal");
        return;
    }

    std::fs::create_dir_all(&data_dir).unwrap();
    let registry = Registry::open(&data_dir.join("state.db")).expect("state.db");
    let core = Core::start(WindowsTerminal::new(install, &shim), Some(registry)).expect("pipe (NativeTerm running?)");
    let settled = |s: &[SessionView]| {
        s.iter().all(|s| !matches!(s.state, State::Opening | State::Detached | State::Connecting))
            && s.iter().filter(|s| s.state.is_open()).all(|s| s.location.is_some() && s.linked)
    };

    match args.first().map(String::as_str) {
        Some("open") => {
            let new_window = args.iter().any(|a| a == "--new-window");
            let hosts: Vec<HostRequest> =
                args[1..].iter().filter(|a| !a.starts_with("--")).map(|l| HostRequest::new(HOST, l.clone())).collect();
            core.open(&hosts, if new_window { Target::NewWindow } else { Target::Recent });
            let ok = wait_until(&core, 30, |s| {
                s.len() >= hosts.len() && settled(s) && s.iter().all(|s| matches!(s.state, State::LoginFailed(_)))
            });
            println!("SETTLED {ok}");
        }
        Some("watch") => {
            let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
            std::thread::sleep(Duration::from_secs(secs));
            let ok = wait_until(&core, 20, settled);
            println!("SETTLED {ok}");
        }
        Some("connect") => {
            let label = args.get(1).expect("label").clone();
            let found = wait_until(&core, 20, |s| s.iter().any(|s| s.label == label && s.linked));
            let id = core.sessions().into_iter().find(|s| s.label == label).map(|s| s.id);
            if let (true, Some(id)) = (found, id) {
                let before = core.sessions().into_iter().find(|s| s.id == id).map_or(0, |s| s.attempt);
                core.connect(&id);
                let ok = wait_until(&core, 20, |s| {
                    s.iter().any(|s| s.id == id && s.attempt > before && matches!(s.state, State::LoginFailed(_)))
                });
                println!("CONNECTED {ok}");
            } else {
                println!("CONNECTED false (no such linked session)");
            }
        }
        Some("exit-closing") => {
            wait_until(&core, 20, settled);
            println!("TOLD {}", core.close_all());
            print(&core);
            // gone at once, like the real window closing
            std::process::exit(0);
        }
        Some("close-all") => {
            wait_until(&core, 20, settled);
            for s in core.sessions().into_iter().filter(|s| s.state.is_open()) {
                core.close(&s.id);
            }
            let ok = wait_until(&core, 20, |s| s.iter().all(|s| !s.state.is_open()));
            std::thread::sleep(Duration::from_secs(3));
            println!("CLOSED {ok}");
        }
        other => panic!("unknown command {other:?}"),
    }
    print(&core);
}
