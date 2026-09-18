//! NativeTerm restarts and Windows Terminal's session restore, against a
//! portable Terminal whose settings have the "NativeTerm SSH" profile,
//! `"firstWindowPreference": "persistedLayout"` and
//! `"warning.confirmOnClose": "never"`:
//!
//! ```text
//! set NATIVETERM_TEST_WT_DIR=C:\…\terminal-1.26.2581.0
//! cargo build -p native-term-shim -p native-term-app --examples
//! cargo test -p native-term-app --test restore_portable -- --ignored --nocapture
//! ```
//!
//! Every step is a separate `core_probe` process, i.e. a NativeTerm run.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use native_term_platform::windows_terminal::install::Install;
use native_term_platform::windows_terminal::window;
use native_term_platform::windows_terminal::WindowsTerminal;

struct Env {
    terminal: PathBuf,
    data: PathBuf,
    probe: PathBuf,
}

#[derive(Debug, Default)]
struct Output {
    text: String,
}

impl Output {
    fn flag(&self, name: &str) -> bool {
        self.text.lines().any(|l| l.starts_with(&format!("{name} true")))
    }

    fn sessions(&self) -> Vec<String> {
        self.text.lines().filter(|l| l.starts_with("SESSION ")).map(String::from).collect()
    }

    fn session(&self, label: &str) -> String {
        self.sessions()
            .into_iter()
            .rev()
            .find(|l| l.contains(&format!("label={label:?}")))
            .unwrap_or_else(|| panic!("no session {label}:\n{}", self.text))
    }

    fn windows(&self) -> Vec<(isize, String)> {
        self.text
            .lines()
            .filter_map(|l| l.strip_prefix("WINDOW "))
            .map(|l| {
                let (handle, tabs) = l.split_once(' ').unwrap();
                (handle.parse().unwrap(), tabs.to_string())
            })
            .collect()
    }
}

impl Env {
    fn new() -> Env {
        let terminal = PathBuf::from(std::env::var("NATIVETERM_TEST_WT_DIR").expect("NATIVETERM_TEST_WT_DIR"));
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf();
        let probe = root.join("target").join("debug").join("examples").join("core_probe.exe");
        assert!(probe.exists(), "build the example first");
        let data = std::env::temp_dir().join(format!("nativeterm-restore-test-{}", std::process::id()));
        Env { terminal, data, probe }
    }

    fn spawn(&self, args: &[&str]) -> Child {
        Command::new(&self.probe)
            .arg("--terminal-dir")
            .arg(&self.terminal)
            .arg("--data-dir")
            .arg(&self.data)
            .args(args)
            .env("NATIVETERM_DEBUG", "1")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        finish(self.spawn(args), args)
    }

    fn terminal_windows(&self) -> Vec<isize> {
        let install = Install::from_dir(&self.terminal).unwrap();
        window::terminal_windows(&install).iter().map(|w| w.handle).collect()
    }

    fn close_all_windows(&self) {
        for w in self.terminal_windows() {
            native_term_win::desktop::close_window(w);
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.terminal_windows().is_empty() {
            assert!(Instant::now() < deadline, "Terminal windows still open");
            std::thread::sleep(Duration::from_millis(200));
        }
        // the Terminal process writes its state and exits
        std::thread::sleep(Duration::from_secs(3));
    }
}

fn finish(child: Child, args: &[&str]) -> Output {
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    println!("--- core_probe {}\n{text}", args.join(" "));
    assert!(out.status.success(), "core_probe failed");
    Output { text }
}

fn assert_waiting_and_located(out: &Output, labels: &[&str]) {
    for label in labels {
        let line = out.session(label);
        assert!(line.contains("state=Waiting") && line.contains("linked=true"), "{line}");
        assert!(!line.contains("tab=-1"), "located: {line}");
    }
    // the placeholders are gone: only the replacement tabs are left
    let windows = out.windows();
    assert_eq!(windows.len(), 1, "{windows:?}");
    for label in labels {
        assert!(windows[0].1.contains(&format!("{label:?}")), "{windows:?}");
    }
    assert!(!windows[0].1.contains("NativeTerm SSH"), "placeholder left: {windows:?}");
}

fn restore(env: &Env, labels: &[&str]) {
    let watch = env.spawn(&["watch", "8"]);
    std::thread::sleep(Duration::from_millis(500));
    env.run(&["restore-terminal"]);
    let out = finish(watch, &["watch", "8"]);
    assert!(out.flag("SETTLED"), "{}", out.text);
    assert_waiting_and_located(&out, labels);
}

fn cleanup(data: &Path) {
    let _ = std::fs::remove_dir_all(data);
}

#[test]
#[ignore = "needs a portable Windows Terminal, see the file header"]
fn restart_and_session_restore() {
    let env = Env::new();
    // a previous test's window may still be closing
    let deadline = Instant::now() + Duration::from_secs(15);
    while !env.terminal_windows().is_empty() {
        assert!(Instant::now() < deadline, "close the test Terminal's windows first");
        std::thread::sleep(Duration::from_millis(250));
    }
    std::thread::sleep(Duration::from_secs(2));
    let labels = ["nt-r a", "nt-r 中文 b"];

    // open, then NativeTerm restarts and finds its tabs again
    let out = env.run(&["open", "--new-window", labels[0], labels[1]]);
    assert!(out.flag("SETTLED"), "{}", out.text);
    let out = env.run(&["watch", "2"]);
    assert!(out.flag("SETTLED"), "{}", out.text);
    for label in labels {
        let line = out.session(label);
        assert!(line.contains("state=LoginFailed(255)") && line.contains("linked=true"), "re-attached: {line}");
    }

    // the window closes while NativeTerm runs: the shims report closing,
    // NativeTerm notices the window went with them
    let watch = env.spawn(&["watch", "14"]);
    std::thread::sleep(Duration::from_secs(3));
    env.close_all_windows();
    let out = finish(watch, &["watch", "14"]);
    for label in labels {
        assert!(out.session(label).contains("state=Closed"), "{}", out.text);
    }
    // Terminal restores the layout: placeholders are replaced by waiting tabs
    restore(&env, &labels);

    let out = env.run(&["connect", labels[1]]);
    assert!(out.flag("CONNECTED"), "{}", out.text);
    assert!(out.session(labels[1]).contains("attempt=1"), "{}", out.text);

    // the window closes while NativeTerm isn't running at all
    env.close_all_windows();
    restore(&env, &labels);

    let out = env.run(&["close-all"]);
    assert!(out.flag("CLOSED"), "{}", out.text);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !env.terminal_windows().is_empty() {
        assert!(Instant::now() < deadline, "window still open");
        std::thread::sleep(Duration::from_millis(200));
    }
    // nothing left to find
    let out = env.run(&["watch", "1"]);
    assert!(out.sessions().is_empty(), "{}", out.text);
    cleanup(&env.data);
}

/// A tab closed while NativeTerm isn't running (or gone with a shutdown or
/// sign-out): its shim no longer runs, so the next start knows the tab is
/// gone right away, without looking for it or reporting it lost. Other
/// Terminal windows are left alone (a new window is used).
#[test]
#[ignore = "needs a portable Windows Terminal, see the file header"]
fn tab_closed_while_nativeterm_was_not_running() {
    let env = Env::new();
    let labels = ["nt-g a", "nt-g b"];
    let out = env.run(&["open", "--new-window", labels[0], labels[1]]);
    assert!(out.flag("SETTLED"), "{}", out.text);

    // NativeTerm isn't running: close one tab with its close button
    let install = Install::from_dir(&env.terminal).unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf();
    let terminal = WindowsTerminal::new(install, &root.join("target").join("debug").join("nativeterm-shim.exe"));
    let snapshot = terminal.snapshot(&Default::default());
    let (window, tab) = snapshot
        .windows
        .iter()
        .find_map(|w| w.tabs.iter().find(|t| t.name == labels[0]).map(|t| (w.handle, t.clone())))
        .expect("the tab");
    assert!(terminal.close(window, &tab).unwrap());
    std::thread::sleep(Duration::from_secs(2));

    let started = Instant::now();
    let out = env.run(&["watch", "1"]);
    assert!(started.elapsed() < Duration::from_secs(10), "no 12 s wait for a lost tab: {:?}", started.elapsed());
    assert!(out.flag("SETTLED"), "{}", out.text);
    assert!(!out.text.contains(&format!("label={:?}", labels[0])), "known to be closed: {}", out.text);
    let line = out.session(labels[1]);
    assert!(line.contains("state=LoginFailed(255)") && line.contains("linked=true"), "the other one found: {line}");
    assert!(!out.text.contains("NOTICE"), "nothing reported lost: {}", out.text);

    let out = env.run(&["close-all"]);
    assert!(out.flag("CLOSED"), "{}", out.text);
    cleanup(&env.data);
}
