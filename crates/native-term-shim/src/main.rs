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
mod askpass;
mod debug;
mod i18n;
mod keys;
mod link;
mod look;
mod persistent;
mod plink;
mod proxy;
mod saved;
mod ssh;
mod win;
mod zmodem;

use std::os::windows::io::AsRawHandle;

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use native_term_session::protocol::{AppMessage, Role, ShimMessage};
use native_term_session::{classify_exit, pipe, SessionEnd, PROTOCOL_VERSION};

use args::Mode;
use link::Link;

const POLL: Duration = Duration::from_millis(100);

fn main() {
    // ssh running the shim as its askpass helper in a key batch: the prompt
    // is the only argument
    if let Ok(pipe) = std::env::var(askpass::PIPE_VAR) {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if let [prompt] = args.as_slice() {
            if !prompt.starts_with("--") {
                std::process::exit(askpass::answer(&pipe, prompt));
            }
        }
    }
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
        Mode::InstallKey { key, alias } => {
            let code = keys::install(&key, &alias);
            wait_for_any_key();
            std::process::exit(code);
        }
        Mode::InstallKeys { key, aliases } => {
            let code = keys::install_batch(&key, &aliases);
            wait_for_any_key();
            std::process::exit(code);
        }
        Mode::AddKeys => {
            let code = keys::add_to_agent();
            wait_for_any_key();
            std::process::exit(code);
        }
        // ssh's ProxyCommand: stdin and stdout are ssh's connection, so
        // nothing else may be printed or waited for
        Mode::Proxy { url, host, port } => std::process::exit(proxy::run(&url, &host, &port)),
        // rz / sz: stdin and stdout are the session's data
        Mode::Zmodem { mode, escape, files } => std::process::exit(zmodem::run(&mode, escape, files)),
        Mode::CreateKey { path } => {
            let code = keys::create(&path);
            wait_for_any_key();
            std::process::exit(code);
        }
        Mode::Shim { session, alias, flags, ssh_dir } => {
            if let Some(dir) = ssh_dir {
                plink::set_ssh_dir(dir.into());
            }
            let code = run(session, alias, flags);
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

fn run(session: Option<String>, alias: Option<String>, flags: args::Flags) -> i32 {
    debug::log(format!(
        "start session={session:?} alias={alias:?} {flags:?} wt_session={:?} pipe={:?}",
        wt_session(),
        pipe_name()
    ));
    let hello = ShimMessage::Hello {
        protocol: PROTOCOL_VERSION,
        role: Role::Shim,
        pid: std::process::id(),
        wt_session: wt_session(),
        session: session.clone(),
        alias: alias.clone(),
        terminal_window: win::terminal_window(),
    };
    let link = pipe_name().map(|name| Link::start(name, hello));
    if let Some(link) = &link {
        let closing = link.sender();
        win::install_ctrl_handler(move || {
            closing.send(ShimMessage::Closing);
            std::thread::sleep(Duration::from_millis(300));
        });
    } else {
        win::install_ctrl_handler(|| {});
    }

    match alias {
        Some(alias) => run_host(&alias, session.as_deref(), link.as_ref(), flags),
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
                    println!("{}", t!("reopening"));
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

fn run_host(alias: &str, session: Option<&str>, link: Option<&Link>, flags: args::Flags) -> i32 {
    if plink::lookup(alias).is_some() {
        return plink::run(alias, link, flags);
    }
    let send = |m: ShimMessage| {
        if let Some(link) = link {
            link.send(m);
        }
    };
    let ssh_path = native_term_session::ssh_program();
    let shim_exe = std::env::current_exe().unwrap_or_default();
    let pid = std::process::id();
    let auth = win::AuthEvent::create(pid).ok();
    let mut attempt = 0;

    if flags.wait {
        send(ShimMessage::Waiting);
        println!("{}", t!("restored", alias = alias));
        if let Next::Close = after_exit(link) {
            return 0;
        }
    }

    // another folder than ~/.ssh (--ssh-dir): ssh reads its config
    let config = plink::custom_ssh_dir().map(|dir| dir.join("config"));
    loop {
        attempt += 1;
        look::apply(alias);
        let effective =
            native_term_config::effective::effective_with(&ssh_path, config.as_deref(), alias).unwrap_or_default();
        // read again on every attempt: an edit applies at the next connect
        let remote = persistent::remote_command(alias, session, &effective);
        let mut arguments =
            ssh::arguments(alias, &shim_exe, pid, &effective, flags.no_forwards, config.as_deref(), remote.as_deref());
        if let Some(event) = &auth {
            event.reset();
        }
        send(ShimMessage::Connecting { attempt });
        // put back after ssh: Windows 10's OpenSSH 8.1 leaves the console
        // without "processed output" (line breaks shown as ♪◙)
        let modes = win::ConsoleModes::save();
        let direct = ssh::is_direct(&effective);
        let mut command = Command::new(&ssh_path);
        // NativeTerm's ssh hands rz / sz to the shim (`--zmodem`); any
        // other ssh ignores it
        command.env("NATIVETERM_ZMODEM", &shim_exe);
        // a saved password: the shim answers ssh's password prompt
        let set = saved::credential_set(alias);
        let saved =
            saved::Attempt::find(&effective, set.as_deref()).filter(|s| s.configure(&mut command, &mut arguments));
        let mut child = match command.args(&arguments).spawn() {
            Ok(child) => child,
            Err(e) => {
                println!("{}", t!("ssh-not-started", path = ssh_path.display().to_string(), error = e.to_string()));
                send(ShimMessage::Exited { code: -1 });
                match after_exit(link) {
                    Next::Reconnect => continue,
                    Next::Close => return 0,
                }
            }
        };

        let reached = direct.then(|| ssh::Reached::watch(child.id()));
        if let Some(saved) = &saved {
            saved.started(child.id());
        }
        let code = match supervise(&mut child, link, auth.as_ref(), None) {
            Supervised::Exited(code) => code,
            Supervised::Close => return 0,
        };
        drop(modes);
        let authenticated = auth.as_ref().is_some_and(|e| e.is_set());
        let end = classify_exit(code, authenticated);
        // only a direct connection shows whether the server was reached
        let unreachable = end == SessionEnd::LoginFailed && reached.is_some_and(|r| !r.stop());
        // the saved password was given and the login failed anyway
        let refused = saved.as_ref().is_some_and(|s| s.finish()) && end == SessionEnd::LoginFailed && !unreachable;
        if let (true, Some(saved)) = (refused, &saved) {
            saved.refused();
            send(ShimMessage::PasswordRefused);
        }
        if unreachable {
            send(ShimMessage::Unreachable);
        }
        send(ShimMessage::Exited { code });
        let text = if unreachable { t!("unreachable", code = code) } else { describe(end, code) };
        println!("\r\n{text}");
        if refused {
            match saved.as_ref().and_then(|s| s.set()) {
                Some(set) => println!("{}", t!("saved-password-refused-set", set = set)),
                None => println!("{}", t!("saved-password-refused")),
            }
        }
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
/// can replay it. Sleeps on handles: ssh's process, the link's arrivals and
/// the login event.
/// `control`: where `AppMessage::Special` goes (ntplink's control pipe).
fn supervise(
    child: &mut Child,
    link: Option<&Link>,
    auth: Option<&win::AuthEvent>,
    control: Option<&win::ControlPipe>,
) -> Supervised {
    let mut reported = false;
    let process = windows::Win32::Foundation::HANDLE(child.as_raw_handle() as _);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Supervised::Exited(status.code().unwrap_or(-1));
        }
        let mut handles = vec![process];
        if let Some(link) = link {
            handles.push(link.arrival_event().handle());
            if let (false, Some(auth)) = (reported, auth) {
                handles.push(auth.handle());
            }
        }
        win::wait_any(&handles, None);
        if let (false, Some(link), Some(auth)) = (reported, link, auth) {
            if auth.is_set() {
                link.send(ShimMessage::Authenticated);
                reported = true;
            }
        }
        for message in link.map(Link::drain).unwrap_or_default() {
            match message {
                AppMessage::Close => {
                    end(child);
                    return Supervised::Close;
                }
                AppMessage::Disconnect => end(child),
                AppMessage::SendText { text, enter } => {
                    let _ = win::inject(&text, enter);
                }
                AppMessage::ClearScreen => {
                    if auth.is_some_and(|a| a.is_set()) {
                        // clear here first, then let the remote side redraw
                        // (Ctrl+L) on the empty screen
                        write_console(CLEAR_ALL);
                        let _ = win::inject("\u{c}", false);
                    } else {
                        // a password prompt may be showing: keep the screen,
                        // and don't type into it
                        write_console(CLEAR_SCROLLBACK);
                    }
                }
                AppMessage::Special { name } => {
                    if let Some(control) = control {
                        control.send(&format!("special {name}"));
                    }
                }
                _ => {}
            }
        }
    }
}

/// Windows Terminal drops the scrollback on `ESC [3J` and keeps the screen
/// (verified through ConPTY on 1.26, see PROTOTYPES.md).
const CLEAR_SCROLLBACK: &[u8] = b"\x1b[3J";
/// `ESC [2J` moves the screen into the scrollback in Windows Terminal, so
/// the scrollback is cleared after it. A remote shell answering Ctrl+L
/// with its own `ESC [2J` then finds an empty screen and adds nothing;
/// clearing only the scrollback before Ctrl+L left the old screen behind.
const CLEAR_ALL: &[u8] = b"\x1b[H\x1b[2J\x1b[3J";

fn write_console(bytes: &[u8]) {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(bytes);
    let _ = out.flush();
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
    println!("{}", t!("reconnect-or-close"));
    let keys = win::KeyReader::open().ok();
    loop {
        let mut handles = Vec::new();
        if let Some(keys) = &keys {
            handles.push(keys.handle());
        }
        if let Some(link) = link {
            handles.push(link.arrival_event().handle());
        }
        if handles.is_empty() {
            // no console input and no NativeTerm: nothing can ever arrive
            return Next::Close;
        }
        win::wait_any(&handles, None);
        if let Some(keys) = &keys {
            // every record read clears the signal; non-key records too
            while let Ok(Some(key)) = keys.read_key(Duration::ZERO) {
                match key {
                    'r' | 'R' | '\r' => return Next::Reconnect,
                    'c' | 'C' => return Next::Close,
                    _ => {}
                }
            }
        }
        for message in link.map(Link::drain).unwrap_or_default() {
            match message {
                AppMessage::Connect => return Next::Reconnect,
                AppMessage::Close => return Next::Close,
                AppMessage::SendText { text, enter } => {
                    let _ = win::inject(&text, enter);
                }
                AppMessage::ClearScreen => {
                    // nothing runs here: clear everything, keep the keys hint
                    write_console(CLEAR_ALL);
                    println!("{}", t!("reconnect-or-close"));
                }
                _ => {}
            }
        }
    }
}

fn describe(end: SessionEnd, code: i32) -> String {
    match end {
        SessionEnd::LoginFailed => t!("login-failed", code = code),
        SessionEnd::Disconnected => t!("disconnected", code = code),
        SessionEnd::Closed(c) => t!("ended", code = c),
    }
}

/// Keep a tool's tab open until a key is pressed, so its result can be
/// read. Only in a tab: with output going to a pipe (tests, scripts)
/// nobody is there to press a key.
fn wait_for_any_key() {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return;
    }
    if let Ok(keys) = win::KeyReader::open() {
        println!("{}", t!("any-key"));
        while !matches!(keys.read_key(Duration::from_secs(3600)), Ok(Some(_))) {}
    }
}
