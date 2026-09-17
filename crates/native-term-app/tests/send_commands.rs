//! Sending commands end to end: shims started directly, each with its own
//! console (injection needs one; the console windows show briefly), and a
//! fake ssh that logs what is typed. Needs no other NativeTerm running and
//! a portable Terminal folder for the core's setup.
//!
//! ```text
//! cargo build -p native-term-shim --examples
//! NATIVETERM_TEST_WT_DIR=<portable Terminal> cargo test -p native-term-app --test send_commands -- --ignored
//! ```

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use native_term_app::registry::Registry;
use native_term_app::{Core, State};
use native_term_platform::windows_terminal::install::Install;
use native_term_platform::windows_terminal::WindowsTerminal;

const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

fn target_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().join("target").join("debug")
}

struct Shim(Child);

impl Drop for Shim {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn spawn_shim(session: &str, guid: &str, log: &Path, login: bool) -> Shim {
    spawn_shim_with(session, guid, log, login, &[])
}

fn spawn_shim_with(session: &str, guid: &str, log: &Path, login: bool, envs: &[(&str, &str)]) -> Shim {
    let fake = target_dir().join("examples").join("fake_ssh.exe");
    assert!(fake.exists(), "cargo build -p native-term-shim --examples");
    let mut command = Command::new(target_dir().join("nativeterm-shim.exe"));
    command
        .args(["--session", session, "nativeterm-test.invalid"])
        .env("NATIVETERM_SSH", fake)
        .env("WT_SESSION", guid)
        .env("NATIVETERM_START_APP", "0")
        .env("NATIVETERM_LANG", "en")
        .env("FAKE_SSH_ECHO", "1")
        .env("FAKE_SSH_CODE", "0")
        .env("FAKE_SSH_LOG", log)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NEW_CONSOLE);
    if login {
        command.env("FAKE_SSH_LOGIN", "1");
    }
    for (k, v) in envs {
        command.env(k, v);
    }
    Shim(command.spawn().unwrap())
}

fn wait_until(what: &str, limit: Duration, mut check: impl FnMut() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what}");
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("{what}: {:.1} s", started.elapsed().as_secs_f64());
}

fn state(core: &Core, id: &str) -> Option<State> {
    core.sessions().into_iter().find(|s| s.id == id).map(|s| s.state)
}

fn typed(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix("input: ").map(str::to_string))
        .collect()
}

#[test]
#[ignore = "serves the user's NativeTerm pipe; opens console windows; needs a portable Terminal folder"]
fn commands_reach_logged_in_sessions_only() {
    let dir = std::env::var("NATIVETERM_TEST_WT_DIR").expect("set NATIVETERM_TEST_WT_DIR to a portable Terminal");
    let install = Install::from_dir(dir.as_ref()).unwrap();
    let shim = target_dir().join("nativeterm-shim.exe");
    let core = Core::start(WindowsTerminal::new(install, &shim), Some(Registry::in_memory().unwrap()))
        .expect("is a NativeTerm running?");
    let tmp = tempfile::tempdir().unwrap();
    core.set_audit_dir(tmp.path().join("audit"));
    let (log_in, log_out) = (tmp.path().join("in.log"), tmp.path().join("out.log"));

    let _in = spawn_shim("sc-in", "6e7a0000-0000-4000-8000-0000000b0001", &log_in, true);
    let _out = spawn_shim("sc-out", "6e7a0000-0000-4000-8000-0000000b0002", &log_out, false);
    let started = Instant::now();
    while !(state(&core, "sc-in") == Some(State::Connected) && state(&core, "sc-out") == Some(State::Connecting)) {
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "one logged in, one not: {:?} {:?}",
            state(&core, "sc-in"),
            state(&core, "sc-out")
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let ids = ["sc-in".to_string(), "sc-out".to_string()];
    let report = core.send_text(&ids, "uptime\necho 你好 😀", true);
    assert_eq!(report.sent.len(), 1, "{report:?}");
    assert_eq!(report.skipped.len(), 1, "{report:?}");
    let started = Instant::now();
    while typed(&log_in).len() < 2 {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "typed into the logged-in session: {:?}",
            std::fs::read_to_string(&log_in)
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(typed(&log_in), ["uptime", "echo 你好 😀"]);
    assert!(typed(&log_out).is_empty(), "nothing typed at a login prompt");

    let audit: Vec<_> = std::fs::read_dir(tmp.path().join("audit")).unwrap().filter_map(Result::ok).collect();
    assert_eq!(audit.len(), 1);
    let text = std::fs::read_to_string(audit[0].path()).unwrap();
    assert!(text.contains("uptime") && text.contains("echo 你好") && !text.contains("sc-out"), "{text}");

    // "exit" ends the fake session; the shim then waits for R / C
    core.send_text(&ids[..1], "exit", true);
    wait_until("the session ended", Duration::from_secs(10), || matches!(state(&core, "sc-in"), Some(State::Ended(0))));

    // a locked session isn't closed with the ended ones
    core.set_locked("sc-in", true);
    assert!(core.sessions().iter().any(|s| s.id == "sc-in" && s.locked));
    assert_eq!(core.close_ended(), 0);
    assert!(matches!(state(&core, "sc-in"), Some(State::Ended(0))));

    // a login command is typed after every login
    let log = tmp.path().join("login.log");
    let _login = spawn_shim_with("lc-1", "6e7a0000-0000-4000-8000-0000000b0003", &log, true, &[("FAKE_SSH_LOGIN_DELAY_MS", "1500")]);
    wait_until("the session is there, not logged in yet", Duration::from_secs(10), || {
        state(&core, "lc-1") == Some(State::Connecting)
    });
    core.set_login_command("lc-1", Some("sudo -i".into()));
    wait_until("typed after login", Duration::from_secs(10), || typed(&log) == ["sudo -i"]);
    core.send_text(&["lc-1".to_string()], "exit", true);
    wait_until("ended", Duration::from_secs(10), || matches!(state(&core, "lc-1"), Some(State::Ended(0))));
    core.connect("lc-1");
    wait_until("typed after the second login", Duration::from_secs(15), || typed(&log) == ["sudo -i", "exit", "sudo -i"]);

    // close everything through the shims, so their ssh processes end too
    for id in ["sc-in", "sc-out", "lc-1"] {
        core.close(id);
    }
    wait_until("all closed", Duration::from_secs(10), || {
        ["sc-in", "sc-out", "lc-1"].iter().all(|id| state(&core, id).is_none_or(|s| !s.is_open()))
    });
    std::thread::sleep(Duration::from_millis(500));
}

