//! First process of every NativeTerm tab.
//!
//! `nativeterm-shim [--session <id>] [<host-alias>]`
//!
//! - Announces itself to NativeTerm (session GUID from `WT_SESSION`).
//! - Runs ssh in the tab and reports connecting / exited; the
//!   `LocalCommand` helper (`--authenticated <shim-pid>`) reports login.
//! - Stays after ssh exits and offers reconnect or close; exits with 0
//!   only to close the tab.
//! - Executes NativeTerm's commands: connect, disconnect, close, type text.
//! - Without a host (restored layout, duplicated pane): asks NativeTerm
//!   (starting it if it isn't running), which closes placeholders it
//!   replaces; otherwise a local shell.
//! - `--wait`: a restored session; connects only when told to.
//!
//! Test hooks: `NATIVETERM_PIPE` (pipe name), `NATIVETERM_SSH` (ssh path),
//! `NATIVETERM_START_APP=0` (never start NativeTerm).

mod args;
mod debug;
mod link;
mod ssh;
mod win;

use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use native_term_session::protocol::{AppMessage, Role, ShimMessage};
use native_term_session::{classify_exit, pipe, SessionEnd, PROTOCOL_VERSION};

use args::Mode;
use link::Link;

const POLL: Duration = Duration::from_millis(100);

fn main() {
    let mode = match args::parse(std::env::args().skip(1)) {
        Ok(mode) => mode,
        Err(e) => {
            eprintln!("[NativeTerm] {e}");
            wait_for_any_key();
            return;
        }
    };
    match mode {
        Mode::Authenticated { shim_pid } => authenticated(shim_pid),
        Mode::Shim { session, alias, wait } => {
            let code = run(session, alias, wait);
            std::process::exit(code);
        }
    }
}

fn pipe_name() -> Option<String> {
    std::env::var("NATIVETERM_PIPE").ok().or_else(|| pipe::pipe_name().ok())
}

fn wt_session() -> Option<String> {
    std::env::var("WT_SESSION").ok().filter(|s| !s.is_empty())
}

/// The `LocalCommand` helper: runs synchronously inside ssh, so it must be
/// quick and print nothing.
fn authenticated(shim_pid: u32) {
    win::signal_authenticated(shim_pid);
    let Some(name) = pipe_name() else { return };
    if let Ok(conn) = pipe::connect(&name, Duration::from_millis(300)) {
        let _ = conn.send(&ShimMessage::Hello {
            protocol: PROTOCOL_VERSION,
            role: Role::AuthSignal,
            pid: std::process::id(),
            wt_session: wt_session(),
            session: None,
            alias: None,
            terminal_window: None,
        });
        let _ = conn.send(&ShimMessage::Authenticated);
    }
}

fn run(session: Option<String>, alias: Option<String>, wait: bool) -> i32 {
    debug::log(format!("start session={session:?} alias={alias:?} wait={wait} wt_session={:?} pipe={:?}", wt_session(), pipe_name()));
    let hello = ShimMessage::Hello {
        protocol: PROTOCOL_VERSION,
        role: Role::Shim,
        pid: std::process::id(),
        wt_session: wt_session(),
        session,
        alias: alias.clone(),
        terminal_window: win::terminal_window(),
    };
    let link = pipe_name().map(|name| Link::start(name, hello));
    if let Some(link) = &link {
        let closing = link.sender();
        win::install_ctrl_handler(move || {
            let _ = closing.send(ShimMessage::Closing);
            std::thread::sleep(Duration::from_millis(300));
        });
    } else {
        win::install_ctrl_handler(|| {});
    }

    match alias {
        Some(alias) => run_host(&alias, link.as_ref(), wait),
        None => run_without_host(link),
    }
}

/// Restored or duplicated pane: NativeTerm decides.
fn run_without_host(link: Option<Link>) -> i32 {
    let connected = link.as_ref().is_some_and(|l| {
        l.wait_reached(Duration::from_secs(1))
            || (start_app() && l.wait_reached(Duration::from_secs(15)))
            || l.wait_reached(Duration::from_secs(1))
    });
    debug::log(format!("placeholder connected={connected}"));
    if let (true, Some(link)) = (connected, link.as_ref()) {
        let mut deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let message = link.recv(POLL);
            if message.is_some() {
                debug::log(format!("placeholder got {message:?}"));
            }
            match message {
                Some(AppMessage::Close) => return 0,
                Some(AppMessage::LocalShell) => break,
                Some(AppMessage::Hold) => {
                    println!("[NativeTerm] Reopening this restored session in a new tab…");
                    deadline = Instant::now() + Duration::from_secs(120);
                }
                _ => {}
            }
        }
    }
    // a local shell makes this the user's own tab: leave NativeTerm alone
    drop(link);
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    let _ = Command::new(shell).status();
    0
}

/// Restored tabs run the shim before NativeTerm may be running: start it
/// (it exits quietly if another copy already serves the pipe).
fn start_app() -> bool {
    if std::env::var("NATIVETERM_START_APP").as_deref() == Ok("0") {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let app = exe.with_file_name("nativeterm.exe");
    let started = app.exists() && Command::new(&app).arg("--from-shim").spawn().is_ok();
    debug::log(format!("started {}: {started}", app.display()));
    started
}

fn run_host(alias: &str, link: Option<&Link>, wait: bool) -> i32 {
    let send = |m: ShimMessage| {
        if let Some(link) = link {
            link.send(m);
        }
    };
    let ssh_path = std::env::var_os("NATIVETERM_SSH").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("ssh"));
    let shim_exe = std::env::current_exe().unwrap_or_default();
    let pid = std::process::id();
    let auth = win::AuthEvent::create(pid).ok();
    let mut attempt = 0;

    if wait {
        send(ShimMessage::Waiting);
        println!("[NativeTerm] {alias}: restored, not connected yet.");
        if let Next::Close = after_exit(link) {
            return 0;
        }
    }

    loop {
        attempt += 1;
        let effective = native_term_config::effective::effective(&ssh_path, alias).unwrap_or_default();
        let arguments = ssh::arguments(alias, &shim_exe, pid, &effective);
        if let Some(event) = &auth {
            event.reset();
        }
        send(ShimMessage::Connecting { attempt });
        let mut child = match Command::new(&ssh_path).args(&arguments).spawn() {
            Ok(child) => child,
            Err(e) => {
                println!("[NativeTerm] Could not start ssh ({}): {e}", ssh_path.display());
                send(ShimMessage::Exited { code: -1 });
                match after_exit(link) {
                    Next::Reconnect => continue,
                    Next::Close => return 0,
                }
            }
        };

        let code = match supervise(&mut child, link, auth.as_ref()) {
            Supervised::Exited(code) => code,
            Supervised::Close => return 0,
        };
        send(ShimMessage::Exited { code });
        let authenticated = auth.as_ref().is_some_and(|e| e.is_set());
        println!("\r\n{}", describe(classify_exit(code, authenticated), code));
        match after_exit(link) {
            Next::Reconnect => continue,
            Next::Close => return 0,
        }
    }
}

enum Supervised {
    Exited(i32),
    Close,
}

/// Wait for ssh while serving NativeTerm's commands. The login is also
/// reported from here (the helper reports it too), so a reconnecting link
/// can replay it.
fn supervise(child: &mut Child, link: Option<&Link>, auth: Option<&win::AuthEvent>) -> Supervised {
    let mut reported = false;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Supervised::Exited(status.code().unwrap_or(-1));
        }
        if let (false, Some(link), Some(auth)) = (reported, link, auth) {
            if auth.is_set() {
                link.send(ShimMessage::Authenticated);
                reported = true;
            }
        }
        let Some(link) = link else {
            std::thread::sleep(POLL);
            continue;
        };
        match link.recv(POLL) {
            Some(AppMessage::Close) => {
                end(child);
                return Supervised::Close;
            }
            Some(AppMessage::Disconnect) => end(child),
            Some(AppMessage::SendText { text, enter }) => {
                let _ = win::inject(&text, enter);
            }
            _ => {}
        }
    }
}

fn end(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

enum Next {
    Reconnect,
    Close,
}

/// ssh has exited: keep the tab, offer reconnect/close on the keyboard,
/// and follow NativeTerm's commands.
fn after_exit(link: Option<&Link>) -> Next {
    println!("[NativeTerm] Press R to reconnect, C to close this tab.");
    let keys = win::KeyReader::open().ok();
    loop {
        if let Some(keys) = &keys {
            match keys.read_key(POLL) {
                Ok(Some('r' | 'R' | '\r')) => return Next::Reconnect,
                Ok(Some('c' | 'C')) => return Next::Close,
                _ => {}
            }
        } else if link.is_none() {
            // no console input and no NativeTerm: nothing can ever arrive
            return Next::Close;
        }
        if let Some(link) = link {
            match link.recv(if keys.is_some() { Duration::ZERO } else { POLL }) {
                Some(AppMessage::Connect) => return Next::Reconnect,
                Some(AppMessage::Close) => return Next::Close,
                Some(AppMessage::SendText { text, enter }) => {
                    let _ = win::inject(&text, enter);
                }
                _ => {}
            }
        }
    }
}

fn describe(end: SessionEnd, code: i32) -> String {
    match end {
        SessionEnd::LoginFailed => format!("[NativeTerm] Login failed or cancelled (exit code {code})."),
        SessionEnd::Disconnected => format!("[NativeTerm] Disconnected (exit code {code})."),
        SessionEnd::Closed(c) => format!("[NativeTerm] Session ended (exit code {c})."),
    }
}

fn wait_for_any_key() {
    if let Ok(keys) = win::KeyReader::open() {
        println!("[NativeTerm] Press any key to close.");
        while !matches!(keys.read_key(Duration::from_secs(3600)), Ok(Some(_))) {}
    }
}
