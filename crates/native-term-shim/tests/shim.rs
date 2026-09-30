#![cfg(windows)]
// Against fake_ssh (a Windows console program) and Windows Terminal's
// consoles: the Unix shim gets its own tests on a Unix machine.
//! End to end: a real shim process against a test pipe server, with a fake
//! ssh (examples/fake_ssh.rs), no network.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use native_term_session::pipe::{PipeConnection, PipeListener};
use native_term_session::protocol::{AppMessage, PasswordAnswer, Role, ShimMessage};

const WAIT: Duration = Duration::from_secs(10);

fn shim_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_nativeterm-shim"))
}

fn fake_ssh() -> PathBuf {
    let path = shim_exe().parent().unwrap().join("examples").join("fake_ssh.exe");
    assert!(path.exists(), "build the example first: {}", path.display());
    path
}

fn pipe_name(tag: &str) -> String {
    // (one of its own each time: a test may serve several at once)
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(r"\\.\pipe\nativeterm-shimtest-{tag}-{}-{n}", std::process::id())
}

fn spawn_shim(pipe: &str, args: &[&str], envs: &[(&str, &str)]) -> Child {
    let mut command = Command::new(shim_exe());
    command
        .args(args)
        .env("NATIVETERM_PIPE", pipe)
        .env("NATIVETERM_SSH", fake_ssh())
        .env("WT_SESSION", "6e7a0000-0000-4000-8000-00000000c0de")
        .env("NATIVETERM_START_APP", "0")
        .env("NATIVETERM_LANG", "en")
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    for (k, v) in envs {
        command.env(k, v);
    }
    command.spawn().unwrap()
}

/// The console's code pages are one setting shared by every test in this
/// binary, and a shim sets them for the session it runs. The tests whose
/// shims do that take this in turn.
static CONSOLE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn console() -> std::sync::MutexGuard<'static, ()> {
    CONSOLE.lock().unwrap_or_else(|e| e.into_inner())
}

fn expect(conn: &PipeConnection) -> ShimMessage {
    conn.recv::<ShimMessage>(WAIT).unwrap().expect("message within timeout")
}

/// The next message that isn't the shim's own login report.
fn expect_skipping_login(conn: &PipeConnection) -> ShimMessage {
    loop {
        match expect(conn) {
            ShimMessage::Authenticated => continue,
            other => return other,
        }
    }
}

fn wait_exit(child: &mut Child) -> i32 {
    for _ in 0..100 {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code().unwrap();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    panic!("shim did not exit");
}

#[test]
fn login_disconnect_reconnect_close() {
    let name = pipe_name("lifecycle");
    let mut listener = PipeListener::bind(&name).unwrap();
    let said = "SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.19";
    let mut shim = spawn_shim(
        &name,
        &["--session", "s-1", "web01"],
        &[
            ("FAKE_SSH_LOGIN", "1"),
            ("FAKE_SSH_CODE", "255"),
            ("FAKE_SSH_SAID", said),
            // (what another server said, left in the environment, is not this one's)
            ("NATIVETERM_SERVER_VERSION", "SSH-2.0-OpenSSH_for_Windows_9.5"),
        ],
    );

    let conn = listener.accept().unwrap();
    assert_eq!(conn.client_pid().unwrap(), shim.id());
    match expect(&conn) {
        ShimMessage::Hello { role, wt_session, session, alias, .. } => {
            assert_eq!(role, Role::Shim);
            assert_eq!(wt_session.as_deref(), Some("6e7a0000-0000-4000-8000-00000000c0de"));
            assert_eq!(session.as_deref(), Some("s-1"));
            assert_eq!(alias.as_deref(), Some("web01"));
        }
        other => panic!("{other:?}"),
    }
    conn.send(&AppMessage::Welcome { protocol: 1, data_dir: None }).unwrap();
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });

    // the LocalCommand helper connects separately
    let helper = listener.accept().unwrap();
    match expect(&helper) {
        ShimMessage::Hello { role, wt_session, .. } => {
            assert_eq!(role, Role::AuthSignal);
            assert_eq!(
                wt_session.as_deref(),
                Some("6e7a0000-0000-4000-8000-00000000c0de"),
                "inherited through ssh and cmd"
            );
        }
        other => panic!("{other:?}"),
    }
    // what the server said it is, as ssh told, and that the login is done
    assert_eq!(expect(&helper), ShimMessage::Server { version: said.into() });
    assert_eq!(expect(&helper), ShimMessage::Authenticated);

    assert_eq!(expect_skipping_login(&conn), ShimMessage::Exited { code: 255 });

    // reconnect on command
    conn.send(&AppMessage::Connect).unwrap();
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 2 });
    let _second_helper = listener.accept().unwrap();
    assert_eq!(expect_skipping_login(&conn), ShimMessage::Exited { code: 255 });

    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0, "exit 0 closes the tab");

    let output = shim.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(text.matches("Disconnected (exit code 255)").count(), 2, "login seen, so not a login failure:\n{text}");
}

#[test]
fn state_is_replayed_in_order_after_nativeterm_restarts() {
    let name = pipe_name("replay");
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut shim = spawn_shim(
        &name,
        &["--session", "s-7", "web01"],
        &[("FAKE_SSH_LOGIN", "1"), ("FAKE_SSH_CODE", "255"), ("FAKE_SSH_MS", "800")],
    );
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
    let _helper = listener.accept().unwrap();
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });
    assert_eq!(expect(&conn), ShimMessage::Authenticated, "the shim reports the login itself");
    assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 });
    // NativeTerm restarts
    drop(conn);
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { session: Some(s), .. } if s == "s-7"));
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });
    assert_eq!(expect(&conn), ShimMessage::Authenticated);
    assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 }, "so it reads as disconnected, not connected");
    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0);
}

#[test]
fn login_failure_is_reported_as_such() {
    let name = pipe_name("loginfail");
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut shim = spawn_shim(&name, &["web01"], &[("FAKE_SSH_CODE", "255")]);
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });
    assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 });
    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0);
    let text = String::from_utf8_lossy(&shim.wait_with_output().unwrap().stdout).to_string();
    assert!(text.contains("Login failed or cancelled"), "{text}");
}

/// A persistent host (the folder's default): ssh gets a terminal and a
/// `RemoteCommand` that attaches to (or creates) the tab's own tmux
/// session, the same name on every attempt; a host set to `off` doesn't.
#[test]
fn persistent_hosts_run_inside_tmux() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join(".ssh");
    std::fs::create_dir_all(ssh.join("config.d")).unwrap();
    std::fs::write(ssh.join("config"), format!("Include {}/config.d/*.conf\n", ssh.display())).unwrap();
    std::fs::write(
        ssh.join("config.d").join("lab.conf"),
        "Host __nativeterm_folder__\n    NativeTermPersistent tmux\n\nHost web01\n    HostName 10.0.0.1\n\n\
         Host db01\n    HostName 10.0.0.2\n    NativeTermPersistent off\n",
    )
    .unwrap();
    let log = dir.path().join("ssh.log");
    let run = |alias: &str, session: &str| {
        let name = pipe_name(&format!("persist-{alias}"));
        let mut listener = PipeListener::bind(&name).unwrap();
        let mut shim = spawn_shim(
            &name,
            &["--ssh-dir", ssh.to_str().unwrap(), "--session", session, alias],
            &[("FAKE_SSH_LOG", log.to_str().unwrap()), ("FAKE_SSH_CODE", "255")],
        );
        let conn = listener.accept().unwrap();
        assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
        assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });
        assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 });
        conn.send(&AppMessage::Connect).unwrap();
        assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 2 });
        assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 });
        conn.send(&AppMessage::Close).unwrap();
        assert_eq!(wait_exit(&mut shim), 0);
        String::from_utf8_lossy(&shim.wait_with_output().unwrap().stdout).to_string()
    };

    let text = run("web01", "0f3a9c21-7d4e-4b8a-9c1d-2e3f4a5b6c7d");
    assert!(text.contains("Attaching to tmux session nt-web01-0f3a9c21 on the server"), "{text}");
    let lines = std::fs::read_to_string(&log).unwrap();
    let connects: Vec<&str> = lines.lines().filter(|l| !l.contains("| -G |")).collect();
    assert_eq!(connects.len(), 2, "{lines}");
    for line in &connects {
        assert!(line.contains("-o | RequestTTY=yes | -o | RemoteCommand=sh -c '"), "{line}");
        assert!(line.contains("exec tmux attach-session -t =nt-web01-0f3a9c21;"), "the same session each time: {line}");
        assert!(line.ends_with("| -- | web01"), "{line}");
    }

    std::fs::remove_file(&log).unwrap();
    let text = run("db01", "11111111-2222-3333-4444-555555555555");
    assert!(!text.contains("Attaching to"), "{text}");
    let lines = std::fs::read_to_string(&log).unwrap();
    assert!(!lines.contains("RemoteCommand"), "{lines}");
}

/// A saved password (a test entry in Credential Manager): the shim's
/// helper answers the account's password prompt, ssh tries it once; a
/// refused one is marked and not used again.
#[test]
fn saved_passwords_are_given_once_and_marked_when_refused() {
    use native_term_win::credentials::{self, Saved};
    let prefix = format!("NativeTerm-Tests-shim-{}", std::process::id());
    let target = format!("{prefix}:tester@web01:22");
    let g = "user tester;hostname web01;port 22;proxyjump bastion";
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("ssh.log");
    let run = |password: &str, attempts: usize| {
        let name = pipe_name("saved");
        let mut listener = PipeListener::bind(&name).unwrap();
        let mut shim = spawn_shim(
            &name,
            &["--session", "s-pw", "web01"],
            &[
                ("NATIVETERM_CRED_PREFIX", &prefix),
                ("FAKE_SSH_G", g),
                ("FAKE_SSH_PASSWORD", password),
                ("FAKE_SSH_LOG", log.to_str().unwrap()),
                ("FAKE_SSH_LOGIN", "1"),
            ],
        );
        let conn = listener.accept().unwrap();
        assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
        // NativeTerm's window, asked where the saved password was refused:
        // the new one is given, to be kept
        let asked =
            answer_windows(listener, vec![PasswordAnswer::Given { secret: password.into(), save: true, user: None }]);
        let mut seen = Vec::new();
        for attempt in 1..=attempts {
            if attempt > 1 {
                conn.send(&AppMessage::Connect).unwrap();
            }
            loop {
                let m = expect(&conn);
                let done = matches!(m, ShimMessage::Exited { .. });
                seen.push(m);
                if done {
                    break;
                }
            }
        }
        conn.send(&AppMessage::Close).unwrap();
        assert_eq!(wait_exit(&mut shim), 0);
        let asked = asked.lock().unwrap().clone();
        (seen, String::from_utf8_lossy(&shim.wait_with_output().unwrap().stdout).to_string(), asked)
    };

    credentials::write(&target, &Saved { user: "tester".into(), secret: "s3cret".into(), comment: String::new() })
        .unwrap();
    let (seen, text, asked) = run("s3cret", 1);
    assert_eq!(
        seen,
        [ShimMessage::Connecting { attempt: 1 }, ShimMessage::Authenticated, ShimMessage::Exited { code: 0 }],
        "{text}"
    );
    assert!(asked.is_empty(), "a saved password is given, nothing asked: {asked:?}");
    let lines = std::fs::read_to_string(&log).unwrap();
    assert!(lines.contains("-o | NumberOfPasswordPrompts=1"), "{lines}");
    assert!(!text.contains("s3cret"), "the password is never shown: {text}");

    // the server's password changed: refused once, marked, not given
    // again; the next attempt asks in NativeTerm's window, saying so, and
    // the new one is kept once it logged in
    let (seen, text, asked) = run("changed", 2);
    assert_eq!(
        seen,
        [
            ShimMessage::Connecting { attempt: 1 },
            ShimMessage::PasswordRefused,
            ShimMessage::Exited { code: 255 },
            ShimMessage::Connecting { attempt: 2 },
            ShimMessage::Authenticated,
            ShimMessage::Exited { code: 0 },
        ],
        "{text}"
    );
    assert!(text.contains("The saved password was refused"), "{text}");
    assert!(!text.contains("changed"), "the password is never shown: {text}");
    assert_eq!(asked, [("tester".to_string(), "web01".to_string(), false, true)], "who, again, refused");
    let saved = credentials::read(&target).unwrap().unwrap();
    credentials::delete(&target).unwrap();
    assert_eq!((saved.secret.as_str(), saved.comment.as_str()), ("changed", ""), "the new one, the mark gone");
}

/// What NativeTerm's window was asked: (user, host, retry, refused).
type Asked = std::sync::Arc<std::sync::Mutex<Vec<(String, String, bool, bool)>>>;

/// NativeTerm's window, as a test plays it: each question on the pipe
/// (`AskPassword`, from a `Request` helper) answered with the next of
/// `answers`; what was asked: (user, host, retry, refused).
fn answer_windows(mut listener: PipeListener, answers: Vec<PasswordAnswer>) -> Asked {
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = std::sync::Arc::clone(&asked);
    std::thread::spawn(move || {
        let mut answers = answers.into_iter();
        while let Ok(conn) = listener.accept() {
            let Ok(Some(ShimMessage::Hello { role: Role::Request, .. })) = conn.recv::<ShimMessage>(WAIT) else {
                continue;
            };
            let Ok(Some(ShimMessage::AskPassword { user, host, retry, refused, .. })) = conn.recv::<ShimMessage>(WAIT)
            else {
                continue;
            };
            seen.lock().unwrap().push((user, host, retry, refused));
            let answer = answers.next().unwrap_or(PasswordAnswer::Cancel);
            let _ = conn.send(&AppMessage::Welcome { protocol: 1, data_dir: None });
            let _ = conn.send(&AppMessage::Password { answer });
        }
    });
    asked
}

/// No password saved: NativeTerm's window asks (as SecureCRT does). A
/// wrong one is asked for again, saying so; the right one is kept once
/// logged in, and only then; Cancel gives the login up and keeps
/// nothing; and the next time the saved one is given, nothing asked.
#[test]
fn a_password_asked_in_the_window_is_kept_once_the_login_worked() {
    use native_term_win::credentials;
    let prefix = format!("NativeTerm-Tests-shimask-{}", std::process::id());
    let target = format!("{prefix}:tester@web01:22");
    let run = |answers: Vec<PasswordAnswer>| {
        let name = pipe_name("ask");
        let mut listener = PipeListener::bind(&name).unwrap();
        let mut shim = spawn_shim(
            &name,
            &["--session", "s-ask", "web01"],
            &[
                ("NATIVETERM_CRED_PREFIX", &prefix),
                ("FAKE_SSH_G", "user tester;hostname web01;port 22"),
                ("FAKE_SSH_PASSWORD", "right"),
                ("FAKE_SSH_TRIES", "3"),
                ("FAKE_SSH_LOGIN", "1"),
            ],
        );
        let conn = listener.accept().unwrap();
        assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
        let asked = answer_windows(listener, answers);
        let mut seen = Vec::new();
        loop {
            let m = expect(&conn);
            let done = matches!(m, ShimMessage::Exited { .. });
            seen.push(m);
            if done {
                break;
            }
        }
        conn.send(&AppMessage::Close).unwrap();
        assert_eq!(wait_exit(&mut shim), 0);
        let asked = asked.lock().unwrap().clone();
        (seen, String::from_utf8_lossy(&shim.wait_with_output().unwrap().stdout).to_string(), asked)
    };
    let given = |secret: &str, save: bool| PasswordAnswer::Given { secret: secret.into(), save, user: None };
    let cleanup = || {
        let _ = credentials::delete(&target);
    };
    let result = std::panic::catch_unwind(|| {
        // Cancel: given up, nothing kept
        let (seen, _, asked) = run(vec![PasswordAnswer::Cancel]);
        // (ssh is ended: given nothing it would try an empty password)
        assert!(matches!(seen.last(), Some(ShimMessage::Exited { code }) if *code != 0), "{seen:?}");
        assert_eq!(asked.len(), 1);
        assert_eq!(credentials::read(&target).unwrap(), None);
        // wrong, then right: asked again saying so; kept once logged in
        let (seen, text, asked) = run(vec![given("wrong", true), given("right", true)]);
        assert_eq!(
            seen,
            [ShimMessage::Connecting { attempt: 1 }, ShimMessage::Authenticated, ShimMessage::Exited { code: 0 }],
            "{text}"
        );
        let who = |retry| ("tester".to_string(), "web01".to_string(), retry, false);
        assert_eq!(asked, [who(false), who(true)]);
        let saved = credentials::read(&target).unwrap().unwrap();
        assert_eq!((saved.user.as_str(), saved.secret.as_str(), saved.comment.as_str()), ("tester", "right", ""));
        assert!(!text.contains("right") && !text.contains("wrong"), "no password in the tab: {text}");
        // saved now: given, nothing asked
        let (seen, _, asked) = run(vec![]);
        assert_eq!(seen[1], ShimMessage::Authenticated);
        assert!(asked.is_empty(), "{asked:?}");
        // another user name: connected again as that user, the password
        // given to its first prompt, kept under that account, and the
        // host's user to be that one
        credentials::delete(&target).unwrap();
        let other = format!("{prefix}:admin@web01:22");
        let (seen, text, asked) =
            run(vec![PasswordAnswer::Given { secret: "right".into(), save: true, user: Some("admin".into()) }]);
        let kept = credentials::read(&other).ok().flatten();
        let _ = credentials::delete(&other);
        assert_eq!(kept.map(|k| (k.user, k.secret)), Some(("admin".to_string(), "right".to_string())), "under admin");
        assert_eq!(credentials::read(&target).unwrap(), None, "nothing under the old user");
        assert_eq!(asked.len(), 1, "asked once: {asked:?}");
        assert!(seen.contains(&ShimMessage::UserChanged { user: "admin".into() }), "{seen:?}");
        assert_eq!(
            seen[seen.len() - 3..],
            [
                ShimMessage::Authenticated,
                ShimMessage::UserChanged { user: "admin".into() },
                ShimMessage::Exited { code: 0 }
            ],
            "{seen:?}"
        );
        assert!(text.contains("admin"), "said in the tab: {text}");
        // "Save password" off: logged in, nothing kept
        let (seen, _, _) = run(vec![given("right", false)]);
        assert_eq!(seen[1], ShimMessage::Authenticated);
        assert_eq!(credentials::read(&target).unwrap(), None);
    });
    cleanup();
    result.unwrap();
}

/// A folder's credential set (`NativeTermCredential`): its password is
/// given, not the account's own entry; a refusal marks the set and names
/// it.
#[test]
fn a_folders_credential_set_is_used_and_marked() {
    use native_term_win::credentials::{self, Saved};
    let prefix = format!("NativeTerm-Tests-shimset-{}", std::process::id());
    let set = format!("{prefix}/cred/机房");
    let own = format!("{prefix}:tester@web01:22");
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join(".ssh");
    std::fs::create_dir_all(ssh.join("config.d")).unwrap();
    std::fs::write(ssh.join("config"), format!("Include {}/config.d/*.conf\n", ssh.display())).unwrap();
    std::fs::write(
        ssh.join("config.d").join("lab.conf"),
        "Host __nativeterm_folder__\n    NativeTermCredential 机房\n\nHost web01\n    HostName web01\n",
    )
    .unwrap();
    let log = dir.path().join("ssh.log");
    let run = |password: &str| {
        let name = pipe_name("set");
        let mut listener = PipeListener::bind(&name).unwrap();
        let mut shim = spawn_shim(
            &name,
            &["--ssh-dir", ssh.to_str().unwrap(), "--session", "s-set", "web01"],
            &[
                ("NATIVETERM_CRED_PREFIX", &prefix),
                ("FAKE_SSH_G", "user tester;hostname web01;port 22;proxyjump bastion"),
                ("FAKE_SSH_PASSWORD", password),
                ("FAKE_SSH_LOG", log.to_str().unwrap()),
            ],
        );
        let conn = listener.accept().unwrap();
        assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
        let mut seen = Vec::new();
        loop {
            let m = expect(&conn);
            let done = matches!(m, ShimMessage::Exited { .. });
            seen.push(m);
            if done {
                break;
            }
        }
        conn.send(&AppMessage::Close).unwrap();
        assert_eq!(wait_exit(&mut shim), 0);
        (seen, String::from_utf8_lossy(&shim.wait_with_output().unwrap().stdout).to_string())
    };
    let saved = |secret: &str| Saved { user: String::new(), secret: secret.into(), comment: String::new() };
    credentials::write(&set, &saved("shared")).unwrap();
    credentials::write(&own, &saved("own-wrong")).unwrap();

    let (seen, text) = run("shared");
    let cleanup = || {
        let _ = credentials::delete(&set);
        let _ = credentials::delete(&own);
    };
    if seen != [ShimMessage::Connecting { attempt: 1 }, ShimMessage::Exited { code: 0 }] {
        cleanup();
        panic!("the set's password logs in: {seen:?} {text}");
    }
    let (seen, text) = run("changed");
    let marked = credentials::read(&set).unwrap().unwrap().comment;
    let own_note = credentials::read(&own).unwrap().unwrap().comment;
    cleanup();
    assert!(seen.contains(&ShimMessage::PasswordRefused), "{seen:?}");
    assert!(text.contains("credential set \"机房\" was refused"), "{text}");
    assert_eq!(marked, native_term_config::password::REFUSED, "the set is marked");
    assert_eq!(own_note, "", "the account's own entry is left alone");
}

/// A direct connection (no proxy in `ssh -G`) that never got a TCP
/// connection up: "could not connect", not a failed login.
#[test]
fn a_server_never_reached_is_reported_as_unreachable() {
    let name = pipe_name("unreachable");
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut shim = spawn_shim(&name, &["web01"], &[("FAKE_SSH_CODE", "255"), ("FAKE_SSH_DIRECT", "1")]);
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });
    assert_eq!(expect(&conn), ShimMessage::Unreachable);
    assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 });
    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0);
    let text = String::from_utf8_lossy(&shim.wait_with_output().unwrap().stdout).to_string();
    assert!(text.contains("Could not connect to the server (exit code 255)"), "{text}");
}

#[test]
fn close_while_connected_ends_ssh() {
    let name = pipe_name("closelive");
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut shim = spawn_shim(&name, &["web01"], &[("FAKE_SSH_MS", "60000")]);
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });
    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0);
}

#[test]
fn placeholder_without_host_is_closed_by_nativeterm() {
    let name = pipe_name("placeholder");
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut shim = spawn_shim(&name, &[], &[]);
    let conn = listener.accept().unwrap();
    match expect(&conn) {
        ShimMessage::Hello { alias, session, .. } => {
            assert_eq!(alias, None);
            assert_eq!(session, None);
        }
        other => panic!("{other:?}"),
    }
    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0);
}

/// What the shim says its tab is, waiting to be told to connect.
fn said_terminal_session(tag: &str, args: &[&str]) -> Option<String> {
    let name = pipe_name(tag);
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut shim = spawn_shim(&name, args, &[("WEZTERM_PANE", "7")]);
    let conn = listener.accept().unwrap();
    let ShimMessage::Hello { wt_session, .. } = expect(&conn) else { panic!("Hello first") };
    assert_eq!(expect(&conn), ShimMessage::Waiting);
    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0);
    wt_session
}

#[test]
fn a_wezterm_pane_is_named_by_its_pane() {
    // (the environment has a `WT_SESSION` too, as when NativeTerm was
    // started from a Windows Terminal tab: NativeTerm looks the tab's
    // session up by what the shim says, and WezTerm asks by the pane)
    let pane = said_terminal_session("pane", &["--wezterm", "--session", "s-9", "--wait", "web01"]);
    assert_eq!(pane.as_deref(), Some("7"));
    let tab = said_terminal_session("tab", &["--session", "s-9", "--wait", "web01"]);
    assert_eq!(tab.as_deref(), Some("6e7a0000-0000-4000-8000-00000000c0de"), "Windows Terminal's tab");
}

#[test]
fn restored_session_waits_for_connect() {
    let name = pipe_name("wait");
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut shim = spawn_shim(&name, &["--session", "s-9", "--wait", "web01"], &[("FAKE_SSH_CODE", "255")]);
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
    assert_eq!(expect(&conn), ShimMessage::Waiting);
    assert_eq!(conn.recv::<ShimMessage>(Duration::from_millis(500)).unwrap(), None, "no connection yet");
    conn.send(&AppMessage::Connect).unwrap();
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });
    assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 });
    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0);
}

#[test]
fn held_placeholder_waits_to_be_closed() {
    let name = pipe_name("hold");
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut shim = spawn_shim(&name, &[], &[]);
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { alias: None, .. }));
    conn.send(&AppMessage::Hold).unwrap();
    // well past the usual 3 s grace period
    std::thread::sleep(Duration::from_secs(5));
    assert!(shim.try_wait().unwrap().is_none(), "still held");
    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0);
}

#[test]
fn placeholder_without_an_answer_becomes_a_local_shell() {
    let name = pipe_name("goeslocal");
    let mut listener = PipeListener::bind(&name).unwrap();
    // the local shell (cmd with no input) ends at once, and so does the shim
    let mut shim = spawn_shim(&name, &[], &[]);
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { alias: None, .. }));
    let started = std::time::Instant::now();
    assert_eq!(wait_exit(&mut shim), 0);
    assert!(started.elapsed() >= Duration::from_millis(2500), "waited for NativeTerm first");
    // and it has hung up instead of reconnecting
    assert_eq!(conn.recv::<ShimMessage>(WAIT).unwrap_err().kind(), std::io::ErrorKind::UnexpectedEof);
}

#[test]
fn placeholder_answered_and_hung_up_at_once() {
    // NativeTerm answers "local shell" and closes within milliseconds
    let name = pipe_name("quickanswer");
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut shim = spawn_shim(&name, &[], &[]);
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { alias: None, .. }));
    conn.send(&AppMessage::LocalShell).unwrap();
    drop(conn);
    let started = std::time::Instant::now();
    assert_eq!(wait_exit(&mut shim), 0, "the local shell (no input) ends at once");
    assert!(started.elapsed() < Duration::from_millis(1500), "took {:?}", started.elapsed());
}

#[test]
fn close_sent_right_before_nativeterm_disconnects() {
    let name = pipe_name("dropclose");
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut shim = spawn_shim(&name, &["web01"], &[("FAKE_SSH_CODE", "255")]);
    // first connection: wait until ssh has exited, then go away
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });
    assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 });
    drop(conn);
    // the shim reconnects; answer and hang up at once, without reading on
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
    conn.send(&AppMessage::Welcome { protocol: 1, data_dir: None }).unwrap();
    conn.send(&AppMessage::Close).unwrap();
    drop(conn);
    assert_eq!(wait_exit(&mut shim), 0);
}

#[test]
fn works_without_nativeterm() {
    // nobody serves the pipe: ssh still runs, and the shim waits at the
    // reconnect/close prompt like in a real tab
    use std::io::{BufRead, BufReader};
    let mut shim = spawn_shim(&pipe_name("absent"), &["web01"], &[("FAKE_SSH_CODE", "0")]);
    let stdout = shim.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let mut seen = Vec::new();
    while let Ok(line) = rx.recv_timeout(WAIT) {
        seen.push(line);
        if seen.iter().any(|l| l.contains("Press R to reconnect")) {
            break;
        }
    }
    let _ = shim.kill();
    let _ = shim.wait();
    let text = seen.join("\n");
    assert!(text.contains("Session ended (exit code 0)"), "{text}");
    assert!(text.contains("Press R to reconnect"), "{text}");
}

/// `--install-key` against a "host" that is Git's `sh` (a POSIX shell):
/// added once, reported as present the second time.
#[test]
fn install_key_posix_host() {
    let sh = PathBuf::from(r"C:\Program Files\Git\usr\bin\sh.exe");
    if !sh.exists() {
        eprintln!("skipped: no Git sh");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let key = home.path().join("id_test.pub");
    std::fs::write(&key, "ssh-ed25519 AAAAposixtest me@pc\n").unwrap();
    let run = || {
        let output = Command::new(shim_exe())
            .args(["--install-key", key.to_str().unwrap(), "posix-host"])
            .env("NATIVETERM_SSH", fake_ssh())
            .env("NATIVETERM_LANG", "en")
            .env("FAKE_SSH_SH", &sh)
            .env("HOME", home.path())
            .output()
            .unwrap();
        (output.status.code(), String::from_utf8_lossy(&output.stdout).to_string())
    };
    let (code, text) = run();
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("now accepts the key"), "{text}");
    let (code, text) = run();
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("already"), "{text}");
    let keys = std::fs::read_to_string(home.path().join(".ssh").join("authorized_keys")).unwrap();
    assert_eq!(keys, "ssh-ed25519 AAAAposixtest me@pc\n");
}

/// The same against a "Windows host": the fake ssh runs the remote command
/// with cmd.exe, which rejects the POSIX script; the shim notices and adds
/// the key with PowerShell (profile and ProgramData in a scratch folder).
#[test]
fn install_key_windows_host() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("id_test.pub");
    std::fs::write(&key, "ssh-ed25519 AAAAwindowstest me@pc\n").unwrap();
    let output = Command::new(shim_exe())
        .args(["--install-key", key.to_str().unwrap(), "windows-host"])
        .env("NATIVETERM_SSH", fake_ssh())
        .env("NATIVETERM_LANG", "en")
        .env("FAKE_SSH_WINDOWS", "1")
        .env("USERPROFILE", dir.path().join("profile"))
        .env("ProgramData", dir.path().join("programdata"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    assert_eq!(output.status.code(), Some(0), "{text}");
    assert!(text.contains("is a Windows host"), "{text}");
    assert!(text.contains("now accepts the key"), "{text}");
    let admin = dir.path().join("programdata").join("ssh").join("administrators_authorized_keys");
    let user = dir.path().join("profile").join(".ssh").join("authorized_keys");
    assert!(admin.exists() || user.exists());
}

/// `--install-key-batch`: the password is given once (here on stdin) and
/// each ssh gets it from the shim's askpass helper; a host with another
/// password fails alone.
#[test]
fn install_key_batch_with_one_password() {
    use std::io::Write;
    let sh = PathBuf::from(r"C:\Program Files\Git\usr\bin\sh.exe");
    if !sh.exists() {
        eprintln!("skipped: no Git sh");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let key = home.path().join("id_test.pub");
    std::fs::write(&key, "ssh-ed25519 AAAAbatchtest me@pc\n").unwrap();
    let mut child = Command::new(shim_exe())
        .args(["--install-key-batch", key.to_str().unwrap(), "host-a", "host-b", "other-c"])
        .env("NATIVETERM_SSH", fake_ssh())
        .env("NATIVETERM_LANG", "en")
        .env("FAKE_SSH_SH", &sh)
        .env("FAKE_SSH_PASSWORD", "batch-test-pw")
        .env("HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"batch-test-pw\n").unwrap();
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("1 added, 1 had it already, 1 failed"), "{text}");
    assert!(text.contains("Failed: other-c"), "{text}");
    assert!(text.contains("other-c refused the login"), "{text}");
    assert!(!text.contains("batch-test-pw"), "the password is never shown: {text}");
    let keys = std::fs::read_to_string(home.path().join(".ssh").join("authorized_keys")).unwrap();
    assert_eq!(keys, "ssh-ed25519 AAAAbatchtest me@pc\n");
}

/// A non-SSH session: the shim finds it in the folder's `.nt.toml`, runs
/// plink (here the fake) with the right arguments, passes the PuTTY-only
/// options in a temporary saved session that is gone afterwards, removes
/// temporary sessions left by shims that no longer run, and leaves other
/// saved sessions alone. A plink that never connected is reported as a
/// failed connection (255).
#[test]
fn plink_session_with_putty_options() {
    let _console = console();
    use native_term_win::registry::{self, RegValue};
    let base = format!(r"Software\NativeTerm-Tests-plink-{}", std::process::id());
    registry::write_user_values(&format!(r"{base}\NativeTerm-4000000000-1"), &[("Left", RegValue::Dword(1))]).unwrap();
    registry::write_user_values(&format!(r"{base}\MySwitch"), &[("HostName", RegValue::Str("10.0.0.1".into()))])
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join(".ssh");
    std::fs::create_dir_all(ssh.join("config.d")).unwrap();
    std::fs::write(ssh.join("config"), "Include config.d/*.conf\n").unwrap();
    std::fs::write(ssh.join("config.d").join("lab.conf"), "").unwrap();
    std::fs::write(
        ssh.join("config.d").join("lab.nt.toml"),
        "[[session]]\nname = \"sw\"\nprotocol = \"telnet\"\nhost = \"10.9.9.9\"\nport = 2300\n\n[session.putty]\nPassiveTelnet = 1\n",
    )
    .unwrap();
    let log = dir.path().join("plink.log");

    let name = pipe_name("plink");
    let mut listener = PipeListener::bind(&name).unwrap();
    let plink = fake_ssh();
    let mut shim = spawn_shim(
        &name,
        &["--ssh-dir", ssh.to_str().unwrap(), "--session", "s-p", "sw"],
        &[
            ("NATIVETERM_PLINK", plink.to_str().unwrap()),
            ("NATIVETERM_PUTTY_KEY", &base),
            ("FAKE_SSH_LOG", log.to_str().unwrap()),
            ("FAKE_SSH_MS", "1500"),
            ("FAKE_SSH_CODE", "0"),
        ],
    );
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
    conn.send(&AppMessage::Welcome { protocol: 1, data_dir: None }).unwrap();
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });

    // while plink runs: its session exists, the stale one is gone
    std::thread::sleep(Duration::from_millis(500));
    let temporary = format!("NativeTerm-{}-1", shim.id());
    let keys = registry::user_subkeys(&base).unwrap();
    assert!(keys.contains(&temporary), "{keys:?}");
    assert!(!keys.iter().any(|k| k == "NativeTerm-4000000000-1"), "stale: {keys:?}");
    let values = registry::user_values(&format!(r"{base}\{temporary}")).unwrap();
    assert_eq!(values[0], ("PassiveTelnet".to_string(), RegValue::Dword(1)));
    let rest: Vec<&str> = values[1..].iter().map(|(n, _)| n.as_str()).collect();
    assert!(rest.is_empty() || rest == ["TermWidth", "TermHeight"], "the tab's size, if any: {values:?}");

    assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 });
    let args = std::fs::read_to_string(&log).unwrap();
    assert_eq!(args.trim(), format!("-load | {temporary} | -telnet | -P | 2300 | 10.9.9.9"));
    let keys = registry::user_subkeys(&base).unwrap();
    assert_eq!(keys, ["MySwitch"], "only the user's own session is left");

    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0);
    let text = String::from_utf8_lossy(&shim.wait_with_output().unwrap().stdout).to_string();
    assert!(text.contains("Connecting to 10.9.9.9:2300 (telnet) with plink"), "{text}");
    assert!(text.contains("Could not connect"), "never connected: {text}");
    registry::delete_user_tree(&base).unwrap();
}

/// With ntplink there: the PuTTY options go on its command line and no
/// saved session is written, the tab learns which special commands the
/// connection takes, a Break from NativeTerm arrives on ntplink's control
/// pipe, and "connection lost" (3) is a connection-level end (255).
#[test]
fn ntplink_session_with_options_and_break() {
    let _console = console();
    use native_term_win::registry;
    let base = format!(r"Software\NativeTerm-Tests-ntplink-{}", std::process::id());
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join(".ssh");
    std::fs::create_dir_all(ssh.join("config.d")).unwrap();
    std::fs::write(ssh.join("config"), "Include config.d/*.conf\n").unwrap();
    std::fs::write(ssh.join("config.d").join("lab.conf"), "").unwrap();
    std::fs::write(
        ssh.join("config.d").join("lab.nt.toml"),
        "[[session]]\nname = \"sw\"\nprotocol = \"telnet\"\nhost = \"10.9.9.9\"\nport = 2300\nbackspace = \"^?\"\n\n[session.putty]\nPassiveTelnet = 1\nTerminalType = \"vt100\"\n",
    )
    .unwrap();
    let log = dir.path().join("ntplink.log");

    let name = pipe_name("ntplink");
    let mut listener = PipeListener::bind(&name).unwrap();
    let ntplink = fake_ssh();
    let mut shim = spawn_shim(
        &name,
        &["--ssh-dir", ssh.to_str().unwrap(), "--session", "s-n", "sw"],
        &[
            ("NATIVETERM_NTPLINK", ntplink.to_str().unwrap()),
            ("NATIVETERM_PUTTY_KEY", &base),
            ("FAKE_SSH_LOG", log.to_str().unwrap()),
            ("FAKE_SSH_MS", "1500"),
            ("FAKE_SSH_CODE", "3"),
        ],
    );
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
    conn.send(&AppMessage::Welcome { protocol: 1, data_dir: None }).unwrap();
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });
    let message = expect(&conn);
    let ShimMessage::Specials { names } = message else {
        conn.send(&AppMessage::Close).unwrap();
        let text = String::from_utf8_lossy(&shim.wait_with_output().unwrap().stdout).to_string();
        panic!("specials expected, got {message:?}: {text}")
    };
    assert!(names.contains(&"brk".to_string()) && names.contains(&"ayt".to_string()), "{names:?}");
    conn.send(&AppMessage::Special { name: "brk".into() }).unwrap();

    assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 });
    let text = std::fs::read_to_string(&log).unwrap();
    let mut lines = text.lines();
    let control = format!(r"\\.\pipe\nativeterm-control-{}-1", shim.id());
    assert_eq!(
        lines.next().unwrap(),
        format!(
            "-set | PassiveTelnet=1 | -set | TerminalType=vt100 | -nt-control | {control} | \
             -nt-backspace | ^? | -telnet | -P | 2300 | 10.9.9.9"
        )
    );
    assert_eq!(lines.collect::<Vec<_>>(), ["control: special brk"]);
    assert!(registry::user_subkeys(&base).unwrap_or_default().is_empty(), "no saved session");

    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0);
    let text = String::from_utf8_lossy(&shim.wait_with_output().unwrap().stdout).to_string();
    assert!(text.contains("Connecting to 10.9.9.9:2300 (telnet) with ntplink"), "{text}");
    let _ = registry::delete_user_tree(&base);
}

/// Without PuTTY options a session still gets a temporary saved session:
/// plink would otherwise start from PuTTY's "Default Settings". It holds
/// only the tab's size (when there is a console), and it is gone after.
#[test]
fn plink_always_loads_its_own_session() {
    let _console = console();
    use native_term_win::registry::{self, RegValue};
    let base = format!(r"Software\NativeTerm-Tests-plink-load-{}", std::process::id());
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join(".ssh");
    std::fs::create_dir_all(ssh.join("config.d")).unwrap();
    std::fs::write(ssh.join("config"), "Include config.d/*.conf\n").unwrap();
    std::fs::write(ssh.join("config.d").join("lab.conf"), "").unwrap();
    std::fs::write(ssh.join("config.d").join("lab.nt.toml"), "[[session]]\nname = \"sw\"\nhost = \"10.9.9.9\"\n")
        .unwrap();
    let log = dir.path().join("plink.log");

    let name = pipe_name("plink-load");
    let mut listener = PipeListener::bind(&name).unwrap();
    let plink = fake_ssh();
    let mut shim = spawn_shim(
        &name,
        &["--ssh-dir", ssh.to_str().unwrap(), "--session", "s-l", "sw"],
        &[
            ("NATIVETERM_PLINK", plink.to_str().unwrap()),
            ("NATIVETERM_PUTTY_KEY", &base),
            ("FAKE_SSH_LOG", log.to_str().unwrap()),
            ("FAKE_SSH_MS", "1500"),
        ],
    );
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
    conn.send(&AppMessage::Welcome { protocol: 1, data_dir: None }).unwrap();
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });
    std::thread::sleep(Duration::from_millis(500));
    let temporary = format!("NativeTerm-{}-1", shim.id());
    assert!(registry::user_subkeys(&base).unwrap().contains(&temporary));
    let values = registry::user_values(&format!(r"{base}\{temporary}")).unwrap();
    let names: Vec<&str> = values.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.is_empty() || names == ["TermWidth", "TermHeight"], "only the size: {values:?}");
    if let Some((_, RegValue::Dword(width))) = values.first() {
        assert!(*width > 0);
    }

    assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 });
    let args = std::fs::read_to_string(&log).unwrap();
    assert_eq!(args.trim(), format!("-load | {temporary} | -telnet | 10.9.9.9"));
    assert!(registry::user_subkeys(&base).unwrap().is_empty(), "deleted");
    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0);
    registry::delete_user_tree(&base).unwrap();
}

/// The ProxyCommand helper against a SOCKS5 proxy on this machine that
/// answers the connection by echoing in upper case: ssh's bytes go
/// through both ways, and a refusal comes out on stderr.
#[test]
fn proxy_helper_passes_bytes_both_ways() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        for refuse in [false, true] {
            let (mut s, _) = listener.accept().unwrap();
            let mut hello = [0u8; 3];
            s.read_exact(&mut hello).unwrap();
            s.write_all(&[5, 0]).unwrap();
            let mut head = [0u8; 5];
            s.read_exact(&mut head).unwrap();
            let mut name = vec![0u8; head[4] as usize + 2];
            s.read_exact(&mut name).unwrap();
            assert_eq!(&name[..name.len() - 2], b"target.lan");
            if refuse {
                s.write_all(&[5, 4, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
                continue;
            }
            s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
            let mut got = Vec::new();
            s.read_to_end(&mut got).unwrap();
            s.write_all(&got.to_ascii_uppercase()).unwrap();
        }
    });
    let url = format!("socks5://127.0.0.1:{port}");
    let run = |input: &[u8]| {
        let mut child = Command::new(shim_exe())
            .args(["--proxy", &url, "target.lan", "22"])
            .env("NATIVETERM_LANG", "en")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    };
    let out = run(b"ssh-2.0 hello\n");
    assert!(out.status.success());
    assert_eq!(out.stdout, b"SSH-2.0 HELLO\n");
    let refused = run(b"");
    assert!(!refused.status.success());
    let error = String::from_utf8_lossy(&refused.stderr);
    assert!(error.contains("host unreachable") && error.contains("target.lan:22"), "{error}");
    server.join().unwrap();
}

/// A SOCKS5 proxy wanting a login: the password comes from Credential
/// Manager (a test entry), a refusal marks it, and a marked password is
/// never sent again.
#[test]
fn proxy_login_from_credential_manager() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use native_term_win::credentials::{self, Saved};

    let prefix = format!("NativeTerm-Tests-proxy-{}", std::process::id());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("socks5://alice@127.0.0.1:{port}");
    let entry = format!("{prefix}/proxy/{url}");
    let saved = Saved { user: "alice".into(), secret: "s3cret".into(), comment: String::new() };
    credentials::write(&entry, &saved).unwrap();
    // accepts the right password once, then refuses
    let server = std::thread::spawn(move || {
        let mut logins = Vec::new();
        for accept in [true, false] {
            let (mut s, _) = listener.accept().unwrap();
            let mut offer = [0u8; 4];
            s.read_exact(&mut offer).unwrap();
            assert_eq!(offer, [5, 2, 0, 2]);
            s.write_all(&[5, 2]).unwrap();
            let mut head = [0u8; 2];
            s.read_exact(&mut head).unwrap();
            let mut user = vec![0u8; head[1] as usize];
            s.read_exact(&mut user).unwrap();
            let mut len = [0u8; 1];
            s.read_exact(&mut len).unwrap();
            let mut password = vec![0u8; len[0] as usize];
            s.read_exact(&mut password).unwrap();
            logins.push((String::from_utf8(user).unwrap(), String::from_utf8(password).unwrap()));
            if !accept {
                s.write_all(&[1, 1]).unwrap();
                continue;
            }
            s.write_all(&[1, 0]).unwrap();
            let mut request = [0u8; 10];
            s.read_exact(&mut request).unwrap();
            s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
            let mut got = Vec::new();
            s.read_to_end(&mut got).unwrap();
            s.write_all(&got.to_ascii_uppercase()).unwrap();
        }
        logins
    });
    let run = |input: &[u8]| {
        let mut child = Command::new(shim_exe())
            .args(["--proxy", &url, "10.0.0.9", "22"])
            .env("NATIVETERM_LANG", "en")
            .env("NATIVETERM_CRED_PREFIX", &prefix)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    };
    // now and then on this machine a fresh process's first connection to
    // the loopback listener is refused (10061) although it is listening;
    // such a run never reached the server, so trying again is transparent
    let ok = (0..3)
        .map(|_| run(b"ssh-2.0\n"))
        .find(|o| !String::from_utf8_lossy(&o.stderr).contains("Couldn't connect to the proxy"))
        .unwrap();
    let refused = run(b"");
    let again = run(b"");
    let marked = credentials::read(&entry).unwrap().unwrap();
    credentials::delete(&entry).unwrap();
    assert_eq!(ok.stdout, b"SSH-2.0\n", "{}", String::from_utf8_lossy(&ok.stderr));
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("refused the user name or password"));
    assert_eq!(marked.comment, native_term_config::password::REFUSED);
    assert_eq!(marked.secret, "s3cret", "kept, only marked");
    let error = String::from_utf8_lossy(&again.stderr);
    assert!(error.contains("was refused before"), "{error}");
    let logins = server.join().unwrap();
    let login = ("alice".to_string(), "s3cret".to_string());
    assert_eq!(logins, [login.clone(), login], "not sent a third time");
}

/// `NativeTermPreConnect`: a command run on this computer before every
/// attempt. A host takes its folder's, its own wins, `none` keeps it
/// away, and a failed command marked `!` stops the connection.
#[test]
fn a_pre_connect_command_runs_before_every_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join(".ssh");
    std::fs::create_dir_all(ssh.join("config.d")).unwrap();
    std::fs::write(ssh.join("config"), format!("Include {}/config.d/*.conf\n", ssh.display())).unwrap();
    let marks = dir.path().join("marks.txt");
    let mark = |what: &str| format!("cmd /c echo {what}>>{}", marks.display());
    std::fs::write(
        ssh.join("config.d").join("lab.conf"),
        format!(
            "Host __nativeterm_folder__\n    NativeTermPreConnect {}\n\n\
             Host web01\n    HostName 10.0.0.1\n\n\
             Host own\n    HostName 10.0.0.2\n    NativeTermPreConnect {}\n\n\
             Host quiet\n    HostName 10.0.0.3\n    NativeTermPreConnect none\n\n\
             Host gate\n    HostName 10.0.0.4\n    NativeTermPreConnect !cmd /c exit 3\n",
            mark("folder"),
            mark("own")
        ),
    )
    .unwrap();
    let run = |alias: &str, attempts: usize| {
        let name = pipe_name(&format!("pre-{alias}"));
        let mut listener = PipeListener::bind(&name).unwrap();
        let mut shim = spawn_shim(
            &name,
            &["--ssh-dir", ssh.to_str().unwrap(), "--session", "s-pre", alias],
            &[("FAKE_SSH_CODE", "255")],
        );
        let conn = listener.accept().unwrap();
        assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
        for attempt in 1..=attempts {
            match expect(&conn) {
                // the connection was stopped before ssh: no Connecting
                ShimMessage::Exited { code } => assert_eq!(code, -1, "{alias}"),
                ShimMessage::Connecting { attempt: n } => {
                    assert_eq!(n as usize, attempt, "{alias}");
                    assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 }, "{alias}");
                }
                other => panic!("{alias}: {other:?}"),
            }
            if attempt < attempts {
                conn.send(&AppMessage::Connect).unwrap();
            }
        }
        conn.send(&AppMessage::Close).unwrap();
        assert_eq!(wait_exit(&mut shim), 0);
        String::from_utf8_lossy(&shim.wait_with_output().unwrap().stdout).to_string()
    };

    // the folder's command, once per attempt
    let text = run("web01", 2);
    assert!(text.contains("Before connecting:"), "{text}");
    let written = std::fs::read_to_string(&marks).unwrap();
    assert_eq!(written.matches("folder").count(), 2, "once per attempt: {written:?}");

    // the host's own wins
    std::fs::write(&marks, "").unwrap();
    run("own", 1);
    let written = std::fs::read_to_string(&marks).unwrap();
    assert_eq!(written.matches("own").count(), 1, "{written:?}");
    assert!(!written.contains("folder"), "the host's own, not the folder's: {written:?}");

    // `none` on the host keeps the folder's away
    std::fs::write(&marks, "").unwrap();
    let text = run("quiet", 1);
    assert!(!text.contains("Before connecting:"), "{text}");
    assert_eq!(std::fs::read_to_string(&marks).unwrap().trim(), "");

    // `!` and a failure: ssh is never started
    let text = run("gate", 1);
    assert!(text.contains("not connecting"), "{text}");
    assert!(!text.contains("Disconnected"), "ssh never ran: {text}");
}
/// `NativeTermCharset`: a host whose output is in a legacy code page.
/// Nothing converts along the way, so the shim sets the console's code
/// page for the session. The console is shared with every other test
/// running at the same time (each plink test sets its own), so the
/// charset here is one nothing else uses: seeing it at all is what shows
/// this shim set it. That `utf-8` sets nothing is a unit test of
/// `native_term_config::charset`.
#[test]
fn a_hosts_charset_becomes_the_consoles_code_page() {
    let _console = console();
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join(".ssh");
    std::fs::create_dir_all(ssh.join("config.d")).unwrap();
    std::fs::write(
        ssh.join("config"),
        format!(
            "Include {}/config.d/*.conf
",
            ssh.display()
        ),
    )
    .unwrap();
    std::fs::write(
        ssh.join("config.d").join("lab.conf"),
        "Host __nativeterm_folder__
    NativeTermCharset big5

Host sw
    HostName 10.0.0.1
",
    )
    .unwrap();
    let log = dir.path().join("sw.log");
    let name = pipe_name("charset");
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut shim = spawn_shim(
        &name,
        &["--ssh-dir", ssh.to_str().unwrap(), "--session", "s-cs", "sw"],
        &[("FAKE_SSH_LOG", log.to_str().unwrap()), ("FAKE_SSH_CODEPAGE", "1"), ("FAKE_SSH_CODE", "255")],
    );
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });
    assert_eq!(expect_skipping_login(&conn), ShimMessage::Exited { code: 255 });
    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0);
    let text = std::fs::read_to_string(&log).unwrap();
    let pages: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("codepage ")).collect();
    assert!(pages.contains(&"950"), "the folder's charset (Big5) was set: {text}");
}

/// The session log from NativeTerm's menu: started (in the data folder
/// NativeTerm's welcome names, as no file is set), fed by the client,
/// stopped; NativeTerm told each time.
#[test]
fn the_session_log_starts_and_stops_from_nativeterm() {
    let data = tempfile::tempdir().unwrap();
    let name = pipe_name("log");
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut shim = spawn_shim(
        &name,
        &["web01"],
        &[
            ("FAKE_SSH_LOGIN", "1"),
            ("FAKE_SSH_FEED", "from the server"),
            ("FAKE_SSH_MS", "8000"),
            ("FAKE_SSH_LOG", &data.path().join("ssh.log").display().to_string()),
        ],
    );
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
    let data_dir = data.path().display().to_string();
    conn.send(&AppMessage::Welcome { protocol: 1, data_dir: Some(data_dir) }).unwrap();
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });
    assert_eq!(expect(&conn), ShimMessage::Authenticated);
    conn.send(&AppMessage::Log { on: true }).unwrap();
    let file = match expect_skipping_login(&conn) {
        ShimMessage::Logging { file: Some(file) } => PathBuf::from(file),
        other => panic!("{other:?}"),
    };
    assert!(file.starts_with(data.path().join("logs")), "{}", file.display());
    let fed = |at_least: usize| {
        let lines = std::fs::read_to_string(&file).unwrap_or_default();
        lines.lines().filter(|l| *l == "from the server").count() >= at_least
    };
    for _ in 0..100 {
        if fed(2) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    conn.send(&AppMessage::Log { on: false }).unwrap();
    assert_eq!(expect_skipping_login(&conn), ShimMessage::Logging { file: None });
    let text = std::fs::read_to_string(&file).unwrap();
    let ssh_log = || std::fs::read_to_string(data.path().join("ssh.log")).unwrap_or_default();
    assert!(text.lines().filter(|l| *l == "from the server").count() >= 2, "{text:?} / ssh: {}", ssh_log());
    // stopped: nothing more is written
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0);
}

/// Logon actions end to end (a console of its own for the shim: typing
/// needs one, the window shows briefly). The fake ssh "server" says
/// `login:`, `Password:` and a prompt, one after the other; the table
/// answers with the credential set's user, a hidden Send from the
/// password store and the set's password, skips a hidden Send this
/// computer doesn't have (telling NativeTerm), and types `exit`.
#[test]
fn logon_actions_answer_the_servers_prompts() {
    use native_term_win::credentials::{self, Saved};
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let prefix = format!("NativeTerm-Tests-logon-{}", std::process::id());
    let set = format!("{prefix}/cred/lab");
    let hidden = format!("{prefix}/logon/t1");
    credentials::write(&set, &Saved { user: "labuser".into(), secret: "labpw".into(), comment: String::new() })
        .unwrap();
    credentials::write(&hidden, &Saved { user: String::new(), secret: "h1dden".into(), comment: String::new() })
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ssh_dir = dir.path().join(".ssh");
    std::fs::create_dir_all(&ssh_dir).unwrap();
    std::fs::write(
        ssh_dir.join("config"),
        "IgnoreUnknown NativeTerm*\nHost web01\n    HostName web01\n    NativeTermLogon yes\n    \
         NativeTermLogonExpect1 login:\n    NativeTermLogonSend1 BSs\n    NativeTermLogonFlags1 cred=lab\n    \
         NativeTermLogonExpect2 Password:\n    NativeTermLogonSend2 secret:t1\n    NativeTermLogonFlags2 hide\n    \
         NativeTermLogonExpect3 $\n    NativeTermLogonSend3 BSw\n    NativeTermLogonFlags3 cred=lab\n    \
         NativeTermLogonSend4 secret:gone\n    NativeTermLogonFlags4 hide\n    \
         NativeTermLogonSend5 exit\n"
            .replace("BS", "\\")
            .as_str(),
    )
    .unwrap();
    let log = dir.path().join("ssh.log");
    let name = pipe_name("logon");
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut shim = Command::new(shim_exe())
        .arg("web01")
        .env("NATIVETERM_PIPE", &name)
        .env("NATIVETERM_SSH", fake_ssh())
        .env("WT_SESSION", "6e7a0000-0000-4000-8000-00000000c0de")
        .env("NATIVETERM_START_APP", "0")
        .env("NATIVETERM_LANG", "en")
        .env("NATIVETERM_SSH_DIR", &ssh_dir)
        .env("NATIVETERM_CRED_PREFIX", &prefix)
        .env("FAKE_SSH_G", "user tester;hostname web01;port 22")
        .env("FAKE_SSH_LOGIN", "1")
        .env("FAKE_SSH_ECHO", "1")
        .env("FAKE_SSH_FEED", "Welcome to web01\r\n|login: |Password: |web01$ ")
        .env("FAKE_SSH_MS", "8000")
        .env("FAKE_SSH_LOG", &log)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .unwrap();
    let conn = listener.accept().unwrap();
    assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
    let mut notes = Vec::new();
    loop {
        match expect_skipping_login(&conn) {
            ShimMessage::Exited { .. } => break,
            ShimMessage::LogonNote { text } => notes.push(text),
            _ => {}
        }
    }
    conn.send(&AppMessage::Close).unwrap();
    let _ = wait_exit(&mut shim);
    let _ = credentials::delete(&set);
    let _ = credentials::delete(&hidden);
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let typed: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("input: ")).collect();
    assert_eq!(typed, ["labuser", "h1dden", "labpw", "exit"], "{text}");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("Logon action 4"), "{notes:?}");
}
