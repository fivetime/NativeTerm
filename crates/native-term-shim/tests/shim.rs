//! End to end: a real shim process against a test pipe server, with a fake
//! ssh (examples/fake_ssh.rs), no network.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use native_term_session::pipe::{PipeConnection, PipeListener};
use native_term_session::protocol::{AppMessage, Role, ShimMessage};

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
    format!(r"\\.\pipe\nativeterm-shimtest-{tag}-{}", std::process::id())
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
    let mut shim = spawn_shim(&name, &["--session", "s-1", "web01"], &[("FAKE_SSH_LOGIN", "1"), ("FAKE_SSH_CODE", "255")]);

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
    conn.send(&AppMessage::Welcome { protocol: 1 }).unwrap();
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 1 });

    // the LocalCommand helper connects separately
    let helper = listener.accept().unwrap();
    match expect(&helper) {
        ShimMessage::Hello { role, wt_session, .. } => {
            assert_eq!(role, Role::AuthSignal);
            assert_eq!(wt_session.as_deref(), Some("6e7a0000-0000-4000-8000-00000000c0de"), "inherited through ssh and cmd");
        }
        other => panic!("{other:?}"),
    }
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
    conn.send(&AppMessage::Welcome { protocol: 1 }).unwrap();
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
