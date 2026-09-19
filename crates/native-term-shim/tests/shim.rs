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
    let mut shim =
        spawn_shim(&name, &["--session", "s-1", "web01"], &[("FAKE_SSH_LOGIN", "1"), ("FAKE_SSH_CODE", "255")]);

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
            assert_eq!(
                wt_session.as_deref(),
                Some("6e7a0000-0000-4000-8000-00000000c0de"),
                "inherited through ssh and cmd"
            );
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
            ],
        );
        let conn = listener.accept().unwrap();
        assert!(matches!(expect(&conn), ShimMessage::Hello { .. }));
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
        (seen, String::from_utf8_lossy(&shim.wait_with_output().unwrap().stdout).to_string())
    };

    credentials::write(&target, &Saved { user: "tester".into(), secret: "s3cret".into(), comment: String::new() })
        .unwrap();
    let (seen, text) = run("s3cret", 1);
    assert_eq!(seen, [ShimMessage::Connecting { attempt: 1 }, ShimMessage::Exited { code: 0 }], "{text}");
    let lines = std::fs::read_to_string(&log).unwrap();
    assert!(lines.contains("-o | NumberOfPasswordPrompts=1"), "{lines}");
    assert!(!text.contains("s3cret"), "the password is never shown: {text}");

    // the server's password changed: refused once, marked, not used again
    let (seen, text) = run("changed", 2);
    assert_eq!(
        seen,
        [
            ShimMessage::Connecting { attempt: 1 },
            ShimMessage::PasswordRefused,
            ShimMessage::Exited { code: 255 },
            ShimMessage::Connecting { attempt: 2 },
            ShimMessage::Exited { code: 255 },
        ],
        "{text}"
    );
    assert!(text.contains("The saved password was refused"), "{text}");
    let saved = credentials::read(&target).unwrap().unwrap();
    assert_eq!(saved.comment, native_term_config::password::REFUSED);
    assert_eq!(saved.secret, "s3cret", "kept, for the user to replace");
    credentials::delete(&target).unwrap();
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

/// A non-SSH session: the shim finds it in the folder's `.nt.toml`, runs
/// plink (here the fake) with the right arguments, passes the PuTTY-only
/// options in a temporary saved session that is gone afterwards, removes
/// temporary sessions left by shims that no longer run, and leaves other
/// saved sessions alone. A plink that never connected is reported as a
/// failed connection (255).
#[test]
fn plink_session_with_putty_options() {
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
    conn.send(&AppMessage::Welcome { protocol: 1 }).unwrap();
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
    use native_term_win::registry;
    let base = format!(r"Software\NativeTerm-Tests-ntplink-{}", std::process::id());
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join(".ssh");
    std::fs::create_dir_all(ssh.join("config.d")).unwrap();
    std::fs::write(ssh.join("config"), "Include config.d/*.conf\n").unwrap();
    std::fs::write(ssh.join("config.d").join("lab.conf"), "").unwrap();
    std::fs::write(
        ssh.join("config.d").join("lab.nt.toml"),
        "[[session]]\nname = \"sw\"\nprotocol = \"telnet\"\nhost = \"10.9.9.9\"\nport = 2300\n\n[session.putty]\nPassiveTelnet = 1\nTerminalType = \"vt100\"\n",
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
    conn.send(&AppMessage::Welcome { protocol: 1 }).unwrap();
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
        format!("-set | PassiveTelnet=1 | -set | TerminalType=vt100 | -nt-control | {control} | -telnet | -P | 2300 | 10.9.9.9")
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
    conn.send(&AppMessage::Welcome { protocol: 1 }).unwrap();
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
