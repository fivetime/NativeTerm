//! NativeTerm's core, without UI: the open-session registry, the pipe
//! server the shims talk to, and the Terminal backend doing the work on
//! background threads. The GUI (`main.rs`) only reads views and sends
//! commands.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use native_term_platform::windows_terminal::WindowsTerminal;
use native_term_platform::{Snapshot, TabSpec, Target};
use native_term_session::pipe::{self, PipeConnection, PipeListener};
use native_term_session::protocol::{AppMessage, Role, ShimMessage};
use native_term_session::{classify_exit, SessionEnd, PROTOCOL_VERSION};

/// How often tab positions are refreshed while sessions are open.
const REFRESH: Duration = Duration::from_secs(2);
const CONFIRM: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// `wt` was asked; the shim hasn't said hello yet.
    Opening,
    Connecting,
    Connected,
    LoginFailed(i32),
    Disconnected(i32),
    Ended(i32),
    /// The tab never appeared, or wasn't sent.
    Failed(String),
    /// The shim went away without saying why (tab or window closed, crash).
    Gone,
    Closed,
}

impl State {
    pub fn is_open(&self) -> bool {
        !matches!(self, State::Failed(_) | State::Gone | State::Closed)
    }

    pub fn describe(&self) -> String {
        match self {
            State::Opening => "opening".into(),
            State::Connecting => "connecting / waiting for login".into(),
            State::Connected => "connected".into(),
            State::LoginFailed(c) => format!("login failed ({c})"),
            State::Disconnected(c) => format!("disconnected ({c})"),
            State::Ended(c) => format!("ended ({c})"),
            State::Failed(why) => format!("failed: {why}"),
            State::Gone => "tab gone".into(),
            State::Closed => "closed".into(),
        }
    }
}

/// Where a session's tab is, as of the last refresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// Stable number of the window (order of first appearance).
    pub window_number: usize,
    pub window: isize,
    pub tab_index: usize,
    pub title: String,
    pub selected: bool,
    pub mixed: bool,
}

#[derive(Clone, Debug)]
pub struct SessionView {
    pub id: String,
    pub label: String,
    pub alias: String,
    pub state: State,
    pub shim_pid: Option<u32>,
    /// Connection attempts of the shim (1 = first connect).
    pub attempt: u32,
    pub linked: bool,
    pub location: Option<Location>,
}

struct Session {
    id: String,
    /// The GUID NativeTerm assigned; the shim's current one may differ
    /// after "Restart connection".
    terminal_session: String,
    current_terminal_session: Option<String>,
    label: String,
    alias: String,
    state: State,
    authenticated: bool,
    attempt: u32,
    shim_pid: Option<u32>,
    link: Option<Arc<PipeConnection>>,
    location: Option<Location>,
}

impl Session {
    fn view(&self) -> SessionView {
        SessionView {
            id: self.id.clone(),
            label: self.label.clone(),
            alias: self.alias.clone(),
            state: self.state.clone(),
            shim_pid: self.shim_pid,
            attempt: self.attempt,
            linked: self.link.is_some(),
            location: self.location.clone(),
        }
    }

    fn matches_terminal_session(&self, guid: &str) -> bool {
        let guid = guid.trim_matches(['{', '}']);
        self.terminal_session.eq_ignore_ascii_case(guid)
            || self.current_terminal_session.as_deref().is_some_and(|g| g.eq_ignore_ascii_case(guid))
    }
}

struct Shared {
    terminal: WindowsTerminal,
    sessions: Mutex<Vec<Session>>,
    notices: Mutex<Vec<String>>,
    snapshot: Mutex<Snapshot>,
    /// Terminal windows in the order NativeTerm first saw them, for
    /// stable window numbers (Z order changes with every activation).
    window_order: Mutex<Vec<isize>>,
    repaint: Box<dyn Fn() + Send + Sync>,
    wake: Mutex<Sender<()>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn changed(&self) {
        (self.repaint)();
    }

    fn notice(&self, text: String) {
        lock(&self.notices).push(text);
        self.changed();
    }

    fn refresh_soon(&self) {
        let _ = lock(&self.wake).send(());
    }

    fn labels(&self) -> HashSet<String> {
        lock(&self.sessions).iter().filter(|s| s.state.is_open()).map(|s| s.label.clone()).collect()
    }

    fn update<R>(&self, id: &str, f: impl FnOnce(&mut Session) -> R) -> Option<R> {
        let result = lock(&self.sessions).iter_mut().find(|s| s.id == id).map(f);
        self.changed();
        result
    }
}

#[derive(Clone)]
pub struct Core {
    shared: Arc<Shared>,
}

/// A host to open: its alias and the label it is shown with.
#[derive(Clone, Debug)]
pub struct HostRequest {
    pub alias: String,
    pub label: String,
}

impl Core {
    /// Serve the pipe and start the background threads. Fails with
    /// `AddrInUse` if another NativeTerm is running.
    pub fn start(terminal: WindowsTerminal, repaint: impl Fn() + Send + Sync + 'static) -> io::Result<Core> {
        let listener = PipeListener::bind(&pipe::pipe_name()?)?;
        let (wake, woken) = mpsc::channel();
        let shared = Arc::new(Shared {
            terminal,
            sessions: Mutex::new(Vec::new()),
            notices: Mutex::new(Vec::new()),
            snapshot: Mutex::new(Snapshot::default()),
            window_order: Mutex::new(Vec::new()),
            repaint: Box::new(repaint),
            wake: Mutex::new(wake),
        });
        let server = Arc::clone(&shared);
        std::thread::Builder::new().name("pipe-server".into()).spawn(move || serve(server, listener))?;
        let refresher = Arc::clone(&shared);
        std::thread::Builder::new().name("tab-refresh".into()).spawn(move || loop {
            match woken.recv_timeout(REFRESH) {
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                _ => {
                    refresh(&refresher);
                }
            }
        })?;
        Ok(Core { shared })
    }

    pub fn terminal(&self) -> &WindowsTerminal {
        &self.shared.terminal
    }

    pub fn sessions(&self) -> Vec<SessionView> {
        lock(&self.shared.sessions).iter().map(Session::view).collect()
    }

    pub fn snapshot(&self) -> Snapshot {
        lock(&self.shared.snapshot).clone()
    }

    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *lock(&self.shared.notices))
    }

    /// Open tabs for `hosts`. Returns the new session ids.
    pub fn open(&self, hosts: &[HostRequest], target: Target) -> Vec<String> {
        let mut specs = Vec::new();
        {
            let mut sessions = lock(&self.shared.sessions);
            let mut taken: HashSet<String> =
                sessions.iter().filter(|s| s.state.is_open()).map(|s| s.label.clone()).collect();
            for host in hosts {
                let label = unique_label(&host.label, &taken);
                taken.insert(label.clone());
                let spec = TabSpec {
                    terminal_session: native_term_config::new_id(),
                    label: label.clone(),
                    session: native_term_config::new_id(),
                    alias: host.alias.clone(),
                };
                sessions.push(Session {
                    id: spec.session.clone(),
                    terminal_session: spec.terminal_session.clone(),
                    current_terminal_session: None,
                    label,
                    alias: host.alias.clone(),
                    state: State::Opening,
                    authenticated: false,
                    attempt: 0,
                    shim_pid: None,
                    link: None,
                    location: None,
                });
                specs.push(spec);
            }
        }
        self.shared.changed();
        let ids: Vec<String> = specs.iter().map(|s| s.session.clone()).collect();
        let shared = Arc::clone(&self.shared);
        std::thread::spawn(move || open_tabs(&shared, &target, &specs));
        ids
    }

    /// Bring the session's tab to the front.
    pub fn focus(&self, id: &str) {
        let shared = Arc::clone(&self.shared);
        let id = id.to_string();
        std::thread::spawn(move || {
            let Some(label) = shared.update(&id, |s| s.label.clone()) else { return };
            let snapshot = refresh(&shared);
            let Some((window, tab)) = snapshot.find(&label) else {
                shared.notice(format!("{label}: its tab wasn't found (a split tab is found once it's selected)"));
                return;
            };
            if let Err(e) = shared.terminal.select(window.handle, tab) {
                shared.notice(format!("{label}: {e}"));
            }
            refresh(&shared);
        });
    }

    pub fn reconnect(&self, id: &str) {
        self.send(id, AppMessage::Connect);
    }

    pub fn disconnect(&self, id: &str) {
        self.send(id, AppMessage::Disconnect);
    }

    /// Close the tab: the shim ends ssh and exits with 0. Without a shim
    /// link, the tab's close button is used.
    pub fn close(&self, id: &str) {
        let link = self.shared.update(id, |s| s.link.clone()).flatten();
        if let Some(link) = link {
            if link.send(&AppMessage::Close).is_ok() {
                self.shared.update(id, |s| s.state = State::Closed);
                return;
            }
        }
        let shared = Arc::clone(&self.shared);
        let id = id.to_string();
        std::thread::spawn(move || {
            let Some(label) = shared.update(&id, |s| s.label.clone()) else { return };
            let snapshot = refresh(&shared);
            match snapshot.find(&label) {
                Some((window, tab)) => match shared.terminal.close(window.handle, tab) {
                    Ok(true) => {
                        shared.update(&id, |s| s.state = State::Closed);
                    }
                    Ok(false) => shared.notice(format!("{label}: the tab changed, not closed")),
                    Err(e) => shared.notice(format!("{label}: {e}")),
                },
                None => {
                    shared.update(&id, |s| s.state = State::Closed);
                }
            }
        });
    }

    /// Forget sessions that are no longer open.
    pub fn clear_finished(&self) {
        lock(&self.shared.sessions).retain(|s| s.state.is_open());
        self.shared.changed();
    }

    fn send(&self, id: &str, message: AppMessage) {
        let link = self.shared.update(id, |s| (s.label.clone(), s.link.clone()));
        match link {
            Some((_, Some(link))) => {
                if let Err(e) = link.send(&message) {
                    self.shared.notice(format!("{id}: {e}"));
                }
            }
            Some((label, None)) => self.shared.notice(format!("{label}: its tab isn't connected to NativeTerm")),
            None => {}
        }
    }
}

/// `label`, or `label (2)`, `label (3)`, … if taken.
pub fn unique_label(label: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(label) {
        return label.to_string();
    }
    (2..).map(|n| format!("{label} ({n})")).find(|l| !taken.contains(l)).expect("unbounded")
}

fn open_tabs(shared: &Shared, target: &Target, specs: &[TabSpec]) {
    let fail = |specs: &[TabSpec], why: &str| {
        let mut sessions = lock(&shared.sessions);
        for spec in specs {
            if let Some(s) = sessions.iter_mut().find(|s| s.id == spec.session && s.state == State::Opening) {
                s.state = State::Failed(why.to_string());
            }
        }
        drop(sessions);
        shared.changed();
    };
    let report = match shared.terminal.open(target, specs) {
        Ok(report) => report,
        Err(e) => {
            shared.notice(format!("Windows Terminal could not be started: {e}"));
            fail(specs, "wt failed");
            return;
        }
    };
    if !report.pending.is_empty() {
        shared.notice(format!("{} tabs were not opened: another Terminal window became active", report.pending.len()));
        fail(&report.pending, "not sent");
    }
    let sent = &specs[..report.launched];
    let expected: Vec<String> = sent.iter().map(|s| s.label.clone()).collect();
    let (_, missing) = shared.terminal.wait_for(&shared.labels(), &expected, CONFIRM);
    if !missing.is_empty() {
        shared.notice(format!("{} tabs didn't appear in Windows Terminal: {}", missing.len(), missing.join(", ")));
        let missing: Vec<TabSpec> = sent.iter().filter(|s| missing.contains(&s.label)).cloned().collect();
        fail(&missing, "tab didn't appear");
    }
    shared.refresh_soon();
}

/// Read all tabs and store each session's position.
fn refresh(shared: &Shared) -> Snapshot {
    let labels = shared.labels();
    let snapshot = if labels.is_empty() {
        // nothing to look for
        Snapshot { windows: Vec::new(), complete: true }
    } else {
        shared.terminal.snapshot(&labels)
    };
    let mut changed = *lock(&shared.snapshot) != snapshot;
    let order = {
        let mut order = lock(&shared.window_order);
        if snapshot.complete {
            order.retain(|h| snapshot.windows.iter().any(|w| w.handle == *h));
        }
        for w in &snapshot.windows {
            if !order.contains(&w.handle) {
                order.push(w.handle);
            }
        }
        order.clone()
    };
    {
        let mut sessions = lock(&shared.sessions);
        for s in sessions.iter_mut() {
            if !s.state.is_open() {
                if s.location.take().is_some() {
                    changed = true;
                }
                continue;
            }
            let found = snapshot.windows.iter().find_map(|w| {
                w.tabs.iter().find(|t| t.claim.as_ref().is_some_and(|c| c.label == s.label)).map(|t| Location {
                    window_number: order.iter().position(|h| *h == w.handle).map_or(0, |n| n + 1),
                    window: w.handle,
                    tab_index: t.index,
                    title: t.name.clone(),
                    selected: t.selected,
                    mixed: t.claim.as_ref().is_some_and(|c| c.mixed),
                })
            });
            // an incomplete scan keeps what was known
            if (found.is_some() || snapshot.complete) && s.location != found {
                s.location = found;
                changed = true;
            }
        }
    }
    if changed {
        *lock(&shared.snapshot) = snapshot.clone();
        // only then: an idle NativeTerm doesn't repaint
        shared.changed();
    }
    snapshot
}

fn serve(shared: Arc<Shared>, mut listener: PipeListener) {
    loop {
        let conn = match listener.accept() {
            Ok(conn) => Arc::new(conn),
            Err(e) => {
                shared.notice(format!("pipe server stopped: {e}"));
                return;
            }
        };
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || handle_connection(&shared, conn));
    }
}

fn handle_connection(shared: &Shared, conn: Arc<PipeConnection>) {
    let Ok(Some(ShimMessage::Hello { protocol, role, pid, wt_session, session, alias })) =
        conn.recv::<ShimMessage>(Duration::from_secs(10))
    else {
        return;
    };
    if protocol != PROTOCOL_VERSION {
        shared.notice(format!("a shim with protocol {protocol} connected (expected {PROTOCOL_VERSION}); update it"));
    }
    let _ = conn.send(&AppMessage::Welcome { protocol: PROTOCOL_VERSION });

    if role == Role::AuthSignal {
        // the LocalCommand helper: one message, then it's gone
        if let Ok(Some(ShimMessage::Authenticated)) = conn.recv::<ShimMessage>(Duration::from_secs(5)) {
            if let Some(guid) = wt_session {
                let mut sessions = lock(&shared.sessions);
                if let Some(s) = sessions.iter_mut().find(|s| s.matches_terminal_session(&guid)) {
                    s.authenticated = true;
                    s.state = State::Connected;
                }
                drop(sessions);
                shared.changed();
            }
        }
        return;
    }

    let Some(alias) = alias else {
        // restored or duplicated pane; without a registry of earlier runs
        // there is nothing to replace it with
        let _ = conn.send(&AppMessage::LocalShell);
        return;
    };

    let id = {
        let mut sessions = lock(&shared.sessions);
        let known = sessions.iter().position(|s| {
            session.as_deref() == Some(s.id.as_str())
                || wt_session.as_deref().is_some_and(|g| s.matches_terminal_session(g))
        });
        let index = match known {
            Some(i) => i,
            None => {
                // a tab from an earlier NativeTerm run: adopt it
                let taken: HashSet<String> =
                    sessions.iter().filter(|s| s.state.is_open()).map(|s| s.label.clone()).collect();
                sessions.push(Session {
                    id: session.clone().unwrap_or_else(native_term_config::new_id),
                    terminal_session: wt_session.clone().unwrap_or_default(),
                    current_terminal_session: None,
                    label: unique_label(&alias, &taken),
                    alias: alias.clone(),
                    state: State::Connecting,
                    authenticated: false,
                    attempt: 0,
                    shim_pid: None,
                    link: None,
                    location: None,
                });
                sessions.len() - 1
            }
        };
        let s = &mut sessions[index];
        s.current_terminal_session = wt_session.clone();
        s.shim_pid = Some(pid);
        s.link = Some(Arc::clone(&conn));
        if s.state == State::Opening {
            s.state = State::Connecting;
        }
        s.id.clone()
    };
    shared.changed();
    shared.refresh_soon();

    loop {
        match conn.recv::<ShimMessage>(Duration::from_secs(3600)) {
            Ok(Some(message)) => {
                shared.update(&id, |s| apply(s, &message));
            }
            Ok(None) => {}
            Err(_) => break,
        }
    }
    shared.update(&id, |s| {
        if s.link.as_ref().is_some_and(|l| Arc::ptr_eq(l, &conn)) {
            s.link = None;
            if s.state != State::Closed {
                s.state = State::Gone;
            }
        }
    });
    shared.refresh_soon();
}

fn apply(s: &mut Session, message: &ShimMessage) {
    match message {
        ShimMessage::Connecting { attempt } => {
            s.authenticated = false;
            s.attempt = *attempt;
            s.state = State::Connecting;
        }
        ShimMessage::Authenticated => {
            s.authenticated = true;
            s.state = State::Connected;
        }
        ShimMessage::Exited { code } => {
            s.state = match classify_exit(*code, s.authenticated) {
                SessionEnd::LoginFailed => State::LoginFailed(*code),
                SessionEnd::Disconnected => State::Disconnected(*code),
                SessionEnd::Closed(c) => State::Ended(c),
            }
        }
        ShimMessage::Closing => s.state = State::Closed,
        ShimMessage::Hello { .. } => {}
    }
}

/// `nativeterm-shim.exe` next to the running program.
pub fn default_shim_path() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(exe.with_file_name("nativeterm-shim.exe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_labels() {
        let taken: HashSet<String> = ["web01", "web01 (2)"].iter().map(|s| s.to_string()).collect();
        assert_eq!(unique_label("db01", &taken), "db01");
        assert_eq!(unique_label("web01", &taken), "web01 (3)");
    }

    fn session() -> Session {
        Session {
            id: "s".into(),
            terminal_session: "6E7A0000-0000-4000-8000-000000000001".into(),
            current_terminal_session: None,
            label: "web01".into(),
            alias: "web01".into(),
            state: State::Opening,
            authenticated: false,
            attempt: 0,
            shim_pid: None,
            link: None,
            location: None,
        }
    }

    #[test]
    fn lifecycle() {
        let mut s = session();
        apply(&mut s, &ShimMessage::Connecting { attempt: 1 });
        assert_eq!(s.state, State::Connecting);
        apply(&mut s, &ShimMessage::Exited { code: 255 });
        assert_eq!(s.state, State::LoginFailed(255));
        apply(&mut s, &ShimMessage::Connecting { attempt: 2 });
        apply(&mut s, &ShimMessage::Authenticated);
        assert_eq!(s.state, State::Connected);
        apply(&mut s, &ShimMessage::Exited { code: -1 });
        assert_eq!(s.state, State::Disconnected(-1));
        apply(&mut s, &ShimMessage::Connecting { attempt: 3 });
        assert_eq!(s.attempt, 3);
        apply(&mut s, &ShimMessage::Authenticated);
        apply(&mut s, &ShimMessage::Exited { code: 0 });
        assert_eq!(s.state, State::Ended(0));
        apply(&mut s, &ShimMessage::Closing);
        assert!(!s.state.is_open());
    }

    #[test]
    fn terminal_session_matching() {
        let mut s = session();
        assert!(s.matches_terminal_session("{6e7a0000-0000-4000-8000-000000000001}"));
        assert!(!s.matches_terminal_session("6e7a0000-0000-4000-8000-000000000002"));
        s.current_terminal_session = Some("6e7a0000-0000-4000-8000-000000000002".into());
        assert!(s.matches_terminal_session("6E7A0000-0000-4000-8000-000000000002"), "after Restart connection");
    }
}
