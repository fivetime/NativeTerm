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

    assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 });

    // reconnect on command
    conn.send(&AppMessage::Connect).unwrap();
    assert_eq!(expect(&conn), ShimMessage::Connecting { attempt: 2 });
    let _second_helper = listener.accept().unwrap();
    assert_eq!(expect(&conn), ShimMessage::Exited { code: 255 });

    conn.send(&AppMessage::Close).unwrap();
    assert_eq!(wait_exit(&mut shim), 0, "exit 0 closes the tab");

    let output = shim.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(text.matches("Disconnected (exit code 255)").count(), 2, "login seen, so not a login failure:\n{text}");
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
