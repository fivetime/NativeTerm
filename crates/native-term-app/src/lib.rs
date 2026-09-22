//! NativeTerm's core, without UI: the open sessions (kept in `state.db`),
//! the pipe server the shims talk to, and the Terminal backend doing the
//! work on background threads. The GUI (`main.rs`) only reads views and
//! sends commands.

pub mod actions;
pub mod commands;
mod connect_queue;
pub mod data_dir;
pub mod data_lock;
pub mod diag;
pub mod fuzzy;
pub mod i18n;
pub mod import;
pub mod notes;
mod previews;
pub mod quick;
pub mod registry;
pub mod settings;
pub mod shortcuts;
pub mod tab_menu;
pub mod tmux_send;
pub mod toast;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant, SystemTime};

#[cfg(windows)]
use native_term_platform::windows_terminal::WindowsTerminal;
use native_term_platform::{Change, MenuTab, OverlayMenu, Subscription, TerminalBackend};
use native_term_platform::{Snapshot, TabSpec, Target, WindowId};
use native_term_session::pipe::{self, PipeConnection, PipeListener};
use native_term_session::protocol::{AppMessage, Role, ShimMessage};
use native_term_session::{classify_exit, SessionEnd, PROTOCOL_VERSION};

pub use previews::Preview;
use registry::{Record, Registry};

/// Tabs are rescanned when Terminal reports a change; this is only the
/// fallback for changes without a notification (renames). A scan costs
/// ≈ 70 ms of CPU (release, one window), so it is rare.
const FALLBACK_REFRESH: Duration = Duration::from_secs(60);
/// Terminal reports changes in bursts (dozens per opened tab).
const DEBOUNCE: Duration = Duration::from_millis(150);
/// Opening more hosts than this at once connects them through the queue.
const PACED_OVER: usize = 3;
/// Automatic reconnects before giving up (the setting is off by default).
const AUTO_RECONNECT_TRIES: u32 = 10;
const AUTO_RECONNECT_SETTING: &str = "auto_reconnect";
/// `state.db` setting: close NativeTerm's tabs when NativeTerm exits.
pub const CLOSE_ON_EXIT_SETTING: &str = "close_tabs_on_exit";
/// Connected at least this long: the retry count starts over.
const STABLE_CONNECTION: Duration = Duration::from_secs(60);
const CONFIRM: Duration = Duration::from_secs(15);
/// Sessions from `state.db` whose shim doesn't turn up by then are gone
/// (their window was closed while NativeTerm wasn't running).
pub const DETACHED_GRACE: Duration = Duration::from_secs(12);
/// After selecting a tab, its panes are read a moment later ("Locate").
const LOCATE_SETTLE: Duration = Duration::from_millis(250);
/// Restored placeholders arrive one by one; replace them together.
const REPLACE_GATHER: Duration = Duration::from_millis(1500);
/// A session that reported closing is "closed with its window" if the
/// window is gone this soon after (Terminal keeps its last window a while
/// to save the layout).
const WINDOW_CLOSE_CHECK: Duration = Duration::from_secs(10);
/// How long Terminal's session restore may bring back such a session.
pub const RESTORABLE_FOR: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// `wt` was asked; the shim hasn't said hello yet.
    Opening,
    /// Known from `state.db`; its shim hasn't said hello yet.
    Detached,
    /// A restored session in a new tab, not connected yet.
    Waiting,
    Connecting,
    Connected,
    LoginFailed(i32),
    /// The server was never reached (a direct connection that was never
    /// established); otherwise like `LoginFailed`.
    Unreachable(i32),
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

    /// Ssh isn't running: connecting is possible.
    pub fn can_connect(&self) -> bool {
        matches!(
            self,
            State::Waiting | State::LoginFailed(_) | State::Unreachable(_) | State::Disconnected(_) | State::Ended(_)
        )
    }

    pub fn describe(&self) -> String {
        match self {
            State::Opening => t!("state-opening"),
            State::Detached => t!("state-detached"),
            State::Waiting => t!("state-waiting"),
            State::Connecting => t!("state-connecting"),
            State::Connected => t!("state-connected"),
            State::LoginFailed(c) => t!("state-login-failed", code = c),
            State::Unreachable(c) => t!("state-unreachable", code = c),
            State::Disconnected(c) => t!("state-disconnected", code = c),
            State::Ended(c) => t!("state-ended", code = c),
            State::Failed(why) => t!("state-failed", reason = why.as_str()),
            State::Gone => t!("state-gone"),
            State::Closed => t!("state-closed"),
        }
    }
}

/// Where a session's tab is, as of the last refresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// Stable number of the window (order of first appearance).
    pub window_number: usize,
    pub window: WindowId,
    pub tab_index: usize,
    pub title: String,
    pub selected: bool,
    pub mixed: bool,
}

/// A tab's console screen, as its shim read it: what a tab that Terminal
/// has never rendered can still be shown as (`ShimMessage::Screen`).
#[derive(Clone, Debug)]
pub struct Screen {
    pub columns: u16,
    pub lines: Vec<String>,
    pub at: SystemTime,
    /// For not asking again too soon.
    asked: Instant,
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
    /// Automatic reconnects since the last successful login, when one is
    /// scheduled or running.
    pub auto_retry: Option<u32>,
    /// Kept out of batch closes and group sends; closed only when unlocked.
    pub locked: bool,
    /// The host's name in the tree when it was renamed after the tab
    /// opened (the tab keeps its title; clones and new tabs use this).
    pub renamed_to: Option<String>,
    /// Where the tab was when NativeTerm last saw it (window, tab), for
    /// sessions it hasn't found again.
    pub last_position: Option<(usize, usize)>,
    /// A serial session that has received nothing since then (seconds
    /// since the Unix epoch).
    pub quiet_since: Option<u64>,
    /// The special commands its connection takes now (`AppMessage::Special`),
    /// e.g. "brk".
    pub specials: Vec<String>,
}

pub(crate) struct Session {
    pub(crate) id: String,
    /// The GUID of the tab NativeTerm opened; the shim's current one may
    /// differ after "Restart connection".
    terminal_session: String,
    current_terminal_session: Option<String>,
    pub(crate) label: String,
    pub(crate) alias: String,
    pub(crate) state: State,
    authenticated: bool,
    attempt: u32,
    shim_pid: Option<u32>,
    pub(crate) link: Option<Arc<PipeConnection>>,
    pub(crate) location: Option<Location>,
    /// Closed together with its window: a restored pane may bring it back.
    restorable: bool,
    /// Opened without port forwards (a clone); kept when the tab is
    /// replaced after a restore.
    no_forwards: bool,
    /// Automatic reconnects since the connection was last stable.
    auto_retries: u32,
    /// Typed after every login.
    on_login: Option<String>,
    /// When the current connection logged in.
    connected_at: Option<Instant>,
    locked: bool,
    /// Window number and tab index from `state.db`.
    last_position: Option<(usize, usize)>,
    quiet_since: Option<u64>,
    specials: Vec<String>,
    /// The tab's console screen, and when it was last asked for.
    pub(crate) screen: Option<Screen>,
    /// The shim said the server wasn't reached in this attempt.
    unreachable: bool,
    /// The attempt an automatic reconnect started: if it fails too, the
    /// retries go on.
    retry_attempt: Option<u32>,
}

impl Session {
    fn new(id: String, terminal_session: String, label: String, alias: String, state: State) -> Session {
        Session {
            id,
            terminal_session,
            current_terminal_session: None,
            label,
            alias,
            state,
            authenticated: false,
            attempt: 0,
            shim_pid: None,
            link: None,
            location: None,
            restorable: false,
            no_forwards: false,
            auto_retries: 0,
            connected_at: None,
            on_login: None,
            locked: false,
            last_position: None,
            quiet_since: None,
            specials: Vec::new(),
            screen: None,
            unreachable: false,
            retry_attempt: None,
        }
    }

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
            auto_retry: (self.auto_retries > 0
                && (matches!(self.state, State::Disconnected(_) | State::Connecting) || self.retrying()))
            .then_some(self.auto_retries),
            locked: self.locked,
            renamed_to: None,
            last_position: self.last_position,
            quiet_since: self.quiet_since.filter(|_| self.state.is_open()),
            specials: if self.state.is_open() { self.specials.clone() } else { Vec::new() },
        }
    }

    /// This attempt was an automatic reconnect that never reached the
    /// server (the network is still down): the retries go on. A failed
    /// login, or an end that can't be told apart from one, stops them:
    /// repeated bad logins get addresses banned (fail2ban).
    fn retrying(&self) -> bool {
        matches!(self.state, State::Unreachable(_)) && self.retry_attempt == Some(self.attempt)
    }

    fn matches_terminal_session(&self, guid: &str) -> bool {
        let guid = guid.trim_matches(['{', '}']);
        self.terminal_session.eq_ignore_ascii_case(guid)
            || self.current_terminal_session.as_deref().is_some_and(|g| g.eq_ignore_ascii_case(guid))
    }
}

/// A restored pane waiting to be replaced by a proper tab.
struct Placeholder {
    session: String,
    conn: Arc<PipeConnection>,
    window: Option<WindowId>,
}

type Repaint = Box<dyn Fn() + Send + Sync>;
/// What the Ctrl+Tab grid is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SwitcherState {
    /// The grid is on screen.
    pub open: bool,
    /// The window and tab index it would switch to.
    pub pick: Option<(WindowId, usize)>,
    /// How many grids were shown, and how many switched a tab.
    pub shown: u32,
    pub switched: u32,
}

/// Asks the main window for something (see `tab_menu::MenuRequest`).
type Ask = Arc<dyn Fn(tab_menu::MenuRequest) + Send + Sync>;

pub(crate) struct Shared {
    terminal: Arc<dyn TerminalBackend>,
    registry: Option<Registry>,
    pub(crate) sessions: Mutex<Vec<Session>>,
    /// Closed with their window in an earlier run; only matched against
    /// restored placeholders, not shown.
    restorable: Mutex<Vec<Session>>,
    notices: Mutex<Vec<String>>,
    snapshot: Mutex<Snapshot>,
    /// Terminal windows in the order NativeTerm first saw them, for
    /// stable window numbers (Z order changes with every activation).
    window_order: Mutex<Vec<WindowId>>,
    repaint: Mutex<Option<Repaint>>,
    wake: Mutex<Sender<()>>,
    /// Keeps the Terminal change notifications alive.
    watcher: Mutex<Option<Box<dyn Subscription>>>,
    /// NativeTerm's own tab menu, once started.
    menu: Mutex<Option<Box<dyn OverlayMenu>>>,
    /// Asks the main window (dialogs, the files window); set with the tab
    /// menu, used by helpers' requests too.
    ask: Mutex<Option<Ask>>,
    /// Full tab scans so far (diagnostics).
    scans: std::sync::atomic::AtomicU64,
    placeholders: Mutex<Sender<Placeholder>>,
    /// Reconnect dropped sessions by themselves (a setting).
    auto_reconnect: std::sync::atomic::AtomicBool,
    /// The tab list is shown: scan every tab even without sessions.
    all_tabs: std::sync::atomic::AtomicBool,
    /// Pictures of the tabs last seen selected, for the tab list.
    previews: Mutex<previews::Previews>,
    /// The next scan only looks again at titles (the tab list's rescan):
    /// no pictures, those follow Terminal's notifications only.
    titles_only: std::sync::atomic::AtomicBool,
    /// The Terminal window that was last in front.
    last_terminal: std::sync::atomic::AtomicU64,
    /// A program that isn't our helper tried the pipe: said once.
    told_of_stranger: std::sync::atomic::AtomicBool,
    /// Where sent commands are recorded (`<data dir>\\audit`).
    audit_dir: Mutex<Option<PathBuf>>,
    /// `settings.toml`, once the data directory is known. Without it
    /// (tests, a data directory that can't be read) settings fall back to
    /// `state.db`.
    settings: Mutex<Option<Arc<crate::settings::Settings>>>,
    /// Each saved host's current name, by alias.
    host_labels: Mutex<HashMap<String, String>>,
    /// Each saved host's tab look, by alias (from the app's tree).
    host_looks: Mutex<HashMap<String, native_term_config::appearance::Appearance>>,
    /// Sessions of the last run whose tabs never turned up, ready to be
    /// opened again: what to open, and which record it came from (see
    /// `Core::lost_at_start`).
    lost_at_start: Mutex<Vec<(String, HostRequest)>>,
    /// Sessions to connect, in order (see `connect_queue`).
    connect_queue: Mutex<Sender<String>>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn changed(&self) {
        if let Some(repaint) = lock(&self.repaint).as_ref() {
            repaint();
        }
    }

    fn notice(&self, text: String) {
        crate::diag::line(&text);
        lock(&self.notices).push(text);
        self.changed();
    }

    fn refresh_soon(&self) {
        let _ = lock(&self.wake).send(());
    }

    /// The name a new tab for this session gets: the host's current name
    /// in the tree, else the session's own without its `(2)` suffix.
    /// A host's tab look now (after an edit too).
    fn look(&self, alias: &str) -> native_term_config::appearance::Appearance {
        lock(&self.host_looks).get(alias).cloned().unwrap_or_default()
    }

    pub(crate) fn fresh_label(&self, alias: &str, label: &str) -> String {
        lock(&self.host_labels).get(alias).cloned().unwrap_or_else(|| tab_menu::base_label(label).to_string())
    }

    fn view(&self, s: &Session) -> SessionView {
        let mut view = s.view();
        view.renamed_to =
            lock(&self.host_labels).get(&s.alias).filter(|l| l.as_str() != tab_menu::base_label(&s.label)).cloned();
        view
    }

    /// Connect these sessions through the queue, in this order.
    fn queue_connect(&self, ids: &[String]) {
        let queue = lock(&self.connect_queue);
        for id in ids {
            let _ = queue.send(id.clone());
        }
    }

    fn labels(&self) -> HashSet<String> {
        lock(&self.sessions).iter().filter(|s| s.state.is_open()).map(|s| s.label.clone()).collect()
    }

    /// Write to `state.db`; failures become notices.
    fn db(&self, what: &str, f: impl FnOnce(&Registry) -> registry::Result<()>) {
        if let Some(registry) = &self.registry {
            if let Err(e) = f(registry) {
                self.notice(format!("state.db ({what}): {e}"));
            }
        }
    }

    /// Change a session; a session that ends or comes back is recorded.
    fn update<R>(&self, id: &str, f: impl FnOnce(&mut Session) -> R) -> Option<R> {
        let (result, ended, back) = {
            let mut sessions = lock(&self.sessions);
            let s = sessions.iter_mut().find(|s| s.id == id)?;
            let was_open = s.state.is_open();
            let result = f(s);
            let open = s.state.is_open();
            (result, was_open && !open, (!was_open && open).then(|| s.current_terminal_session.clone()))
        };
        if ended {
            self.db("close", |r| r.closed(id));
        }
        if let Some(guid) = back {
            self.db("reopen", |r| r.seen_terminal_session(id, guid.as_deref()));
        }
        self.changed();
        Some(result)
    }
}

#[derive(Clone)]
pub struct Core {
    pub(crate) shared: Arc<Shared>,
}

/// A host to open: its alias and the label it is shown with.
#[derive(Clone, Debug)]
pub struct HostRequest {
    pub alias: String,
    pub label: String,
    /// A clone: open without port forwards.
    pub no_forwards: bool,
    /// Typed after every login (`NativeTermOnLogin`).
    pub on_login: Option<String>,
    /// The tab's session id, when it matters (reopening a persistent
    /// session: the server-side name comes from it); a new one otherwise.
    pub session: Option<String>,
}

impl HostRequest {
    pub fn new(alias: impl Into<String>, label: impl Into<String>) -> HostRequest {
        HostRequest { alias: alias.into(), label: label.into(), no_forwards: false, on_login: None, session: None }
    }
}

impl Core {
    /// Serve the pipe, pick up the sessions `state.db` knows, and start the
    /// background threads. Fails with `AddrInUse` if another NativeTerm is
    /// running.
    pub fn start(terminal: impl TerminalBackend, registry: Option<Registry>) -> io::Result<Core> {
        Core::start_with_pipe(terminal, registry, &pipe::pipe_name()?)
    }

    /// `start`, serving `pipe` instead of the one the shims look for: a
    /// test's own NativeTerm beside the real one.
    pub fn start_with_pipe(terminal: impl TerminalBackend, registry: Option<Registry>, pipe: &str) -> io::Result<Core> {
        let terminal: Arc<dyn TerminalBackend> = Arc::new(terminal);
        let listener = PipeListener::bind(pipe)?;
        let (wake, woken) = mpsc::channel();
        let (placeholders, queued) = mpsc::channel();
        let (to_connect, connect_ids) = mpsc::channel();
        let mut sessions = Vec::new();
        let mut restorable = Vec::new();
        let mut notices = Vec::new();
        // sessions of the last run whose tabs are not there any more
        let mut lost: Vec<(String, HostRequest)> = Vec::new();
        if let Some(registry) = &registry {
            let to_session = |r: Record, state: State| {
                let mut s = Session::new(r.id, r.terminal_session, r.label, r.alias, state);
                s.current_terminal_session = r.current_terminal_session;
                s.no_forwards = r.no_forwards;
                s.locked = r.locked;
                s.last_position = r.window_number.zip(r.tab_index).map(|(w, t)| (w as usize, t as usize));
                s
            };
            match registry.open_sessions() {
                Ok(records) => {
                    for r in records {
                        // its shim is gone (shut down, signed out, the tab closed
                        // while NativeTerm wasn't running): so is the tab. Kept
                        // restorable, in case Terminal restores the pane.
                        if !shim_alive(r.shim) {
                            if let Err(e) = registry.closed_with_window(&r.id) {
                                notices.push(format!("state.db: {e}"));
                            }
                            // NativeTerm didn't close them itself (it was
                            // killed, or Terminal took them with it), so
                            // they can be offered back
                            lost.push((r.id.clone(), HostRequest::new(r.alias.clone(), r.label.clone())));
                            continue;
                        }
                        sessions.push(to_session(r, State::Detached));
                    }
                }
                Err(e) => notices.push(format!("state.db: {e}")),
            }
            match registry.restorable_sessions(RESTORABLE_FOR) {
                Ok(records) => restorable.extend(records.into_iter().map(|r| to_session(r, State::Closed))),
                Err(e) => notices.push(format!("state.db: {e}")),
            }
        }
        let shared = Arc::new(Shared {
            terminal,
            registry,
            sessions: Mutex::new(sessions),
            restorable: Mutex::new(restorable),
            notices: Mutex::new(notices),
            snapshot: Mutex::new(Snapshot::default()),
            window_order: Mutex::new(Vec::new()),
            repaint: Mutex::new(None),
            wake: Mutex::new(wake.clone()),
            watcher: Mutex::new(None),
            menu: Mutex::new(None),
            ask: Mutex::new(None),
            scans: Default::default(),
            placeholders: Mutex::new(placeholders),
            auto_reconnect: Default::default(),
            all_tabs: Default::default(),
            previews: Default::default(),
            titles_only: Default::default(),
            last_terminal: Default::default(),
            told_of_stranger: Default::default(),
            audit_dir: Mutex::new(None),
            settings: Mutex::new(None),
            host_labels: Mutex::new(HashMap::new()),
            host_looks: Mutex::new(HashMap::new()),
            connect_queue: Mutex::new(to_connect),
            lost_at_start: Mutex::new(lost),
        });
        let queue = Arc::downgrade(&shared);
        std::thread::Builder::new()
            .name("connect-queue".into())
            .spawn(move || connect_queue::run(queue, connect_ids))?;
        let auto = shared.registry.as_ref().and_then(|r| r.setting(AUTO_RECONNECT_SETTING).ok().flatten());
        shared.auto_reconnect.store(auto.as_deref() == Some("1"), std::sync::atomic::Ordering::Relaxed);
        let server = Arc::clone(&shared);
        std::thread::Builder::new().name("pipe-server".into()).spawn(move || serve(server, listener))?;
        let wake = Mutex::new(wake);
        let weak: Weak<Shared> = Arc::downgrade(&shared);
        let watcher = shared.terminal.subscribe(Arc::new(move |change: Change| {
            match change {
                Change::Foreground => {
                    if let Some(shared) = weak.upgrade() {
                        if let Some(front) = shared.terminal.foreground() {
                            shared.last_terminal.store(front.0, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    return;
                }
                Change::Popup => return,
                Change::Content => {}
                Change::Tabs | Change::Windows | Change::Moved => {
                    // tab rectangles are stale until the next scan
                    if let Some(shared) = weak.upgrade() {
                        if let Some(menu) = lock(&shared.menu).as_ref() {
                            menu.invalidate();
                        }
                    }
                }
            }
            let _ = lock(&wake).send(());
        }));
        *lock(&shared.watcher) = Some(watcher);
        let refresher = Arc::clone(&shared);
        std::thread::Builder::new().name("tab-refresh".into()).spawn(move || loop {
            use std::sync::atomic::Ordering::Relaxed;
            // pictures only after a notification, never on a timer
            let picture = match woken.recv_timeout(FALLBACK_REFRESH) {
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Ok(()) => {
                    // let the burst finish (or the window drag end), then
                    // scan once
                    for _ in 0..20 {
                        std::thread::sleep(DEBOUNCE);
                        if woken.try_iter().count() == 0 {
                            break;
                        }
                    }
                    !refresher.titles_only.swap(false, Relaxed)
                }
                Err(mpsc::RecvTimeoutError::Timeout) => false,
            };
            scan(&refresher, picture);
        })?;
        let replacer = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("replace-restored".into())
            .spawn(move || replace_placeholders(&replacer, queued))?;
        let grace = Arc::clone(&shared);
        std::thread::Builder::new().name("detached-grace".into()).spawn(move || {
            std::thread::sleep(DETACHED_GRACE);
            let lost: Vec<(String, String, String)> = lock(&grace.sessions)
                .iter()
                .filter(|s| s.state == State::Detached)
                .map(|s| (s.id.clone(), s.alias.clone(), s.label.clone()))
                .collect();
            for (id, _, _) in &lost {
                grace.update(id, |s| {
                    if s.state == State::Detached {
                        s.state = State::Gone;
                    }
                });
            }
            if !lost.is_empty() {
                // their tabs never came back: Terminal was restarted, or
                // restored them from a program folder that has moved
                lock(&grace.lost_at_start).extend(
                    lost.iter().map(|(id, alias, label)| (id.clone(), HostRequest::new(alias.clone(), label.clone()))),
                );
                grace.notice(t!("notice-lost-sessions", count = lost.len()));
                grace.changed();
            }
        })?;
        Ok(Core { shared })
    }

    /// Start NativeTerm's own right-click menu on its tabs (once).
    /// `send` opens the send dialog for a session id.
    pub fn start_tab_menu(&self, ask: impl Fn(tab_menu::MenuRequest) + Send + Sync + 'static) -> io::Result<()> {
        let ask: Ask = Arc::new(ask);
        // kept even without a menu: helpers in tabs ask through it too
        *lock(&self.shared.ask) = Some(Arc::clone(&ask));
        let provider = Arc::new(tab_menu::Actions { core: Arc::downgrade(&self.shared), ask });
        let Some(menu) = self.shared.terminal.start_overlay_menu(provider)? else { return Ok(()) };
        // the keyboard hook can't read settings: it is told
        menu.set_ctrl_tab(self.setting(tab_menu::SWITCHER_SETTING).as_deref() == Some("on"));
        *lock(&self.shared.menu) = Some(menu);
        self.shared.refresh_soon();
        Ok(())
    }

    /// How long ago a drag from another window ended over a terminal
    /// window (the menu's hook sees it), if the backend has such a menu.
    pub fn tab_menu_since_drag_release(&self) -> Option<Duration> {
        lock(&self.shared.menu).as_ref().and_then(|m| m.since_drag_release())
    }

    /// Choose an item of the open tab menu (automation, tests).
    pub fn tab_menu_choose(&self, id: u32) {
        if let Some(menu) = lock(&self.shared.menu).as_ref() {
            menu.choose(id);
        }
    }

    pub fn tab_menu_hovered(&self) -> Option<u32> {
        lock(&self.shared.menu).as_ref().and_then(|m| m.hovered())
    }

    /// Whether Ctrl+Tab over a Terminal window with NativeTerm tabs
    /// shows NativeTerm's grid (the setting is kept in `state.db`; the
    /// menu thread is told so its keyboard hook needs no lookup).
    pub fn set_ctrl_tab(&self, on: bool) {
        self.set_setting(tab_menu::SWITCHER_SETTING, if on { "on" } else { "off" });
        if let Some(menu) = lock(&self.shared.menu).as_ref() {
            menu.set_ctrl_tab(on);
        }
    }

    /// What the menu thread is doing, which is what counts; the setting
    /// only says what to do at the next start.
    pub fn ctrl_tab(&self) -> bool {
        match lock(&self.shared.menu).as_ref() {
            Some(menu) => menu.ctrl_tab(),
            None => self.setting(tab_menu::SWITCHER_SETTING).as_deref() == Some("on"),
        }
    }

    /// What the Ctrl+Tab grid is doing (diagnostics, tests).
    pub fn switcher_state(&self) -> Option<SwitcherState> {
        let menu = lock(&self.shared.menu);
        let menu = menu.as_ref()?;
        let (shown, switched) = menu.switcher_counts();
        Some(SwitcherState { open: menu.switcher_open(), pick: menu.switcher_pick(), shown, switched })
    }

    pub fn tab_menu_debug(&self) -> String {
        lock(&self.shared.menu).as_ref().map(|m| m.debug_state()).unwrap_or_default()
    }

    /// Menus opened so far and whether one is open (diagnostics, tests).
    pub fn tab_menu_state(&self) -> Option<(u32, bool)> {
        lock(&self.shared.menu).as_ref().map(|m| (m.opened(), m.is_open()))
    }

    /// Called when anything visible changed (from background threads).
    pub fn set_repaint(&self, repaint: impl Fn() + Send + Sync + 'static) {
        *lock(&self.shared.repaint) = Some(Box::new(repaint));
    }

    pub fn terminal(&self) -> &dyn TerminalBackend {
        &*self.shared.terminal
    }

    /// The Windows Terminal behind the backend, for what only it has
    /// (its install, its profile fragment).
    #[cfg(windows)]
    pub fn windows_terminal(&self) -> Option<&WindowsTerminal> {
        self.shared.terminal.as_any().downcast_ref()
    }

    pub fn registry(&self) -> Option<&Registry> {
        self.shared.registry.as_ref()
    }

    pub fn sessions(&self) -> Vec<SessionView> {
        lock(&self.shared.sessions).iter().map(|s| self.shared.view(s)).collect()
    }

    pub fn snapshot(&self) -> Snapshot {
        lock(&self.shared.snapshot).clone()
    }

    /// Terminal change notifications so far: (window, tab) events.
    pub fn change_counts(&self) -> (u64, u64) {
        lock(&self.shared.watcher).as_ref().map_or((0, 0), |w| {
            let c = w.counts();
            (c.windows, c.tabs)
        })
    }

    pub fn scan_count(&self) -> u64 {
        self.shared.scans.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Show a notice (from any thread).
    pub fn add_notice(&self, text: String) {
        self.shared.notice(text);
    }

    /// The saved hosts' current names (alias → name), after every reload.
    /// Each saved host's tab color and color scheme, for the tabs opened
    /// from now on.
    pub fn set_host_looks(&self, looks: HashMap<String, native_term_config::appearance::Appearance>) {
        *lock(&self.shared.host_looks) = looks;
    }

    pub fn set_host_labels(&self, labels: HashMap<String, String>) {
        *lock(&self.shared.host_labels) = labels;
        self.shared.changed();
    }

    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *lock(&self.shared.notices))
    }

    /// Open tabs for `hosts`. Returns the new session ids.
    pub fn open(&self, hosts: &[HostRequest], target: Target) -> Vec<String> {
        let mut specs = Vec::new();
        let paced = hosts.len() > PACED_OVER;
        {
            let mut sessions = lock(&self.shared.sessions);
            let mut taken: HashSet<String> =
                sessions.iter().filter(|s| s.state.is_open()).map(|s| s.label.clone()).collect();
            for host in hosts {
                let label = unique_label(&host.label, &taken);
                taken.insert(label.clone());
                let look = self.shared.look(&host.alias);
                let spec = TabSpec {
                    terminal_session: native_term_config::new_id(),
                    label: label.clone(),
                    session: host.session.clone().unwrap_or_else(native_term_config::new_id),
                    alias: host.alias.clone(),
                    wait: paced,
                    no_forwards: host.no_forwards,
                    tab_color: look.tab_color,
                };
                sessions.push(Session::new(
                    spec.session.clone(),
                    spec.terminal_session.clone(),
                    label,
                    host.alias.clone(),
                    State::Opening,
                ));
                if let Some(s) = sessions.last_mut() {
                    s.no_forwards = host.no_forwards;
                    s.on_login = host.on_login.clone().filter(|c| !c.trim().is_empty());
                }
                specs.push(spec);
            }
        }
        for spec in &specs {
            self.shared.db("open", |r| {
                r.opened(&record_for(spec))?;
                r.count_use(&spec.alias)
            });
        }
        self.shared.changed();
        let ids: Vec<String> = specs.iter().map(|s| s.session.clone()).collect();
        if paced {
            self.shared.queue_connect(&ids);
        }
        let shared = Arc::clone(&self.shared);
        std::thread::spawn(move || {
            open_tabs(&shared, &target, &specs);
            // each new tab took the focus; the batch ends on its first one
            if specs.len() > 1 {
                let snapshot = refresh(&shared);
                if let Some((window, tab)) = snapshot.find(&specs[0].label) {
                    let _ = shared.terminal.select(window.handle, tab);
                }
            }
        });
        ids
    }

    /// Sessions whose tab NativeTerm hasn't found (a split tab that isn't
    /// selected shows no pane titles, for example).
    pub fn unlocated(&self) -> usize {
        lock(&self.shared.sessions)
            .iter()
            .filter(|s| s.state.is_open() && s.link.is_some() && s.location.is_none())
            .count()
    }

    /// Look for them: only the selected tab shows its panes, so each
    /// unclaimed tab is selected once, until every session is found. The
    /// selection is put back afterwards. Never done on its own.
    pub fn locate(&self) {
        let shared = Arc::clone(&self.shared);
        let core = self.clone();
        std::thread::spawn(move || {
            if core.unlocated() == 0 {
                return;
            }
            let snapshot = refresh(&shared);
            if core.unlocated() == 0 {
                return;
            }
            let mut selected = Vec::new();
            let mut candidates = Vec::new();
            for window in &snapshot.windows {
                if let Some(tab) = window.tabs.iter().find(|t| t.selected) {
                    selected.push((window.handle, tab.clone()));
                }
                for tab in window.tabs.iter().filter(|t| t.claim.is_none() && !t.selected) {
                    candidates.push((window.handle, tab.clone()));
                }
            }
            // a tab a session was last seen at is tried first: a split
            // tab can only be found by selecting it, and after a restart
            // it is usually still where it was, so the sweep normally
            // ends on its first pick instead of walking the whole strip
            let hints: std::collections::HashSet<(usize, usize)> = core
                .sessions()
                .iter()
                .filter(|s| s.state.is_open() && s.location.is_none())
                .filter_map(|s| s.last_position)
                .collect();
            let numbers: Vec<(WindowId, Option<usize>)> =
                candidates.iter().map(|(w, _)| (*w, core.window_number(*w))).collect();
            let number = |handle: WindowId| numbers.iter().find(|(w, _)| *w == handle).and_then(|(_, n)| *n);
            likely_first(&mut candidates, &hints, number);
            let mut looked_at = 0;
            for (window, tab) in candidates {
                if core.unlocated() == 0 {
                    break;
                }
                if shared.terminal.select(window, &tab).is_err() {
                    continue;
                }
                looked_at += 1;
                std::thread::sleep(LOCATE_SETTLE);
                refresh(&shared);
            }
            for (window, tab) in selected {
                let _ = shared.terminal.select(window, &tab);
            }
            refresh(&shared);
            let left = core.unlocated();
            shared.notice(if left == 0 {
                t!("notice-located", count = looked_at)
            } else {
                t!("notice-still-unlocated", count = left)
            });
        });
    }

    /// Bring the session's tab to the front.
    pub fn focus(&self, id: &str) {
        let shared = Arc::clone(&self.shared);
        let id = id.to_string();
        std::thread::spawn(move || {
            let Some(label) = shared.update(&id, |s| s.label.clone()) else { return };
            let snapshot = refresh(&shared);
            let Some((window, tab)) = snapshot.find(&label) else {
                shared.notice(t!("notice-tab-not-found", label = label.as_str()));
                return;
            };
            if let Err(e) = shared.terminal.select(window.handle, tab) {
                shared.notice(format!("{label}: {e}"));
            }
            refresh(&shared);
        });
    }

    /// Whether every tab is wanted (the tab list is shown); scans at once
    /// when turned on.
    pub fn want_all_tabs(&self, on: bool) {
        let was = self.shared.all_tabs.swap(on, std::sync::atomic::Ordering::Relaxed);
        if on && !was {
            self.shared.refresh_soon();
        }
    }

    /// The picture of the tab at `index` in `window` from when it was
    /// last seen selected.
    pub fn preview(&self, window: WindowId, index: usize) -> Option<Preview> {
        lock(&self.shared.previews).get(window, index)
    }

    /// What was on that tab's screen when it was last pictured (a tab
    /// NativeTerm doesn't run has no console of its own to ask).
    pub fn preview_text(&self, window: WindowId, index: usize) -> Option<std::sync::Arc<Vec<String>>> {
        lock(&self.shared.previews).text(window, index).filter(|lines| !lines.is_empty())
    }

    /// Scan Terminal's tabs again soon (titles change without a
    /// notification).
    pub fn rescan(&self) {
        self.shared.titles_only.store(true, std::sync::atomic::Ordering::Relaxed);
        self.shared.refresh_soon();
    }

    /// The stable number of a Terminal window (1-based), as in the session
    /// list.
    pub fn window_number(&self, handle: WindowId) -> Option<usize> {
        lock(&self.shared.window_order).iter().position(|h| *h == handle).map(|i| i + 1)
    }

    /// Switch to any tab (the user's own too): the tab at `index` in
    /// `window`, if it still has the title `name`, else a tab with that
    /// title in the same window.
    pub fn select_tab(&self, window: WindowId, index: usize, name: &str) {
        let shared = Arc::clone(&self.shared);
        let name = name.to_string();
        std::thread::spawn(move || {
            let snapshot = refresh(&shared);
            let found = snapshot.windows.iter().find(|w| w.handle == window).and_then(|w| {
                w.tabs
                    .iter()
                    .find(|t| t.index == index && t.name == name)
                    .or_else(|| w.tabs.iter().find(|t| t.name == name))
                    .cloned()
            });
            match found {
                Some(tab) => {
                    if let Err(e) = shared.terminal.select(window, &tab) {
                        shared.notice(format!("{name}: {e}"));
                    }
                }
                None => shared.notice(t!("notice-tab-gone", title = name.as_str())),
            }
            refresh(&shared);
        });
    }

    /// Set (or clear) what is typed after each login of a session.
    pub fn set_login_command(&self, id: &str, command: Option<String>) {
        self.shared.update(id, |s| s.on_login = command.filter(|c| !c.trim().is_empty()));
    }

    /// The sessions of the last run whose tabs never turned up. They can
    /// be opened again as they were, which is what a moved program folder
    /// or a restarted Terminal calls for.
    pub fn lost_at_start(&self) -> Vec<HostRequest> {
        lock(&self.shared.lost_at_start).iter().map(|(_, request)| request.clone()).collect()
    }

    /// Forget them. `reopened`: they were just opened again, so a pane
    /// Terminal brings back later would be a duplicate.
    pub fn forget_lost(&self, reopened: bool) {
        let lost = std::mem::take(&mut *lock(&self.shared.lost_at_start));
        if reopened {
            for (id, _) in &lost {
                let id = id.clone();
                self.shared.db("reopened", move |r| r.not_restorable(&id));
            }
        }
        self.clear_finished();
    }

    /// Record sent commands in `dir` (one file per month).
    pub fn set_audit_dir(&self, dir: PathBuf) {
        *lock(&self.shared.audit_dir) = Some(dir);
    }

    /// Type `text` into sessions, line by line (see `commands::lines`).
    /// Only sessions that are logged in get it: text typed at a password or
    /// host-key prompt would be taken as the answer. Every send is
    /// recorded in the audit log.
    pub fn send_text(&self, ids: &[String], text: &str, enter: bool) -> SendReport {
        let lines = commands::lines(text, enter);
        let mut report = SendReport::default();
        let mut audit = Vec::new();
        for id in ids {
            let target = lock(&self.shared.sessions)
                .iter()
                .find(|s| &s.id == id)
                .map(|s| (s.label.clone(), s.alias.clone(), s.state == State::Connected, s.link.clone()));
            let Some((label, alias, connected, link)) = target else { continue };
            let Some(link) = link.filter(|_| connected) else {
                report.skipped.push(label);
                continue;
            };
            let ok = lines
                .iter()
                .all(|(line, enter)| link.send(&AppMessage::SendText { text: line.clone(), enter: *enter }).is_ok());
            if ok {
                report.sent.push(label.clone());
                audit.push(format!("{label} ({alias})"));
            } else {
                report.failed.push(label);
            }
        }
        if !audit.is_empty() {
            if let Err(e) = self.audit(&audit, text) {
                self.shared.notice(format!("audit log: {e}"));
            }
        }
        report
    }

    /// Records a send made some other way (tmux on the server) in the
    /// audit log, like `send_text`'s own.
    pub fn record_send(&self, targets: &[String], text: &str) {
        if let Err(e) = self.audit(targets, text) {
            self.shared.notice(format!("audit log: {e}"));
        }
    }

    fn audit(&self, targets: &[String], text: &str) -> io::Result<()> {
        use std::io::Write;
        let Some(dir) = lock(&self.shared.audit_dir).clone() else { return Ok(()) };
        std::fs::create_dir_all(&dir)?;
        let (date, time) = utc_now();
        let file = dir.join(format!("commands-{}.log", &date[..7]));
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file)?;
        let text = text.replace('\r', "").replace('\n', " \u{23CE} ");
        writeln!(f, "{date} {time}Z\t{}\t{text}", targets.join(", "))
    }

    /// NativeTerm's session in the selected tab of the Terminal window that
    /// was last in front (what "the active session" means while NativeTerm
    /// itself has the focus).
    pub fn active_session(&self) -> Option<SessionView> {
        let snapshot = self.snapshot();
        let last = WindowId(self.shared.last_terminal.load(std::sync::atomic::Ordering::Relaxed));
        let window = snapshot
            .windows
            .iter()
            .find(|w| w.handle == last)
            .or_else(|| snapshot.windows.iter().find(|w| w.foreground))?;
        let tab = window.tabs.iter().find(|t| t.selected)?;
        self.sessions().into_iter().find(|s| {
            s.state.is_open()
                && s.location.as_ref().is_some_and(|l| l.window == window.handle && l.tab_index == tab.index)
        })
    }

    /// Close every session whose connection ended or failed.
    pub fn close_ended(&self) -> usize {
        let ended = actions::close_set(&self.sessions(), &actions::CloseSet::Ended);
        self.close_ids(&ended);
        ended.len()
    }

    /// Close every open session's tab except locked ones (NativeTerm is
    /// exiting): each shim is told over its pipe, so this returns at once
    /// and the tabs close after NativeTerm is gone; all of them are
    /// recorded as closed in `state.db` right away, so the next start
    /// doesn't look for them.
    pub fn close_all(&self) -> usize {
        let open: Vec<_> = lock(&self.shared.sessions)
            .iter()
            .filter(|s| !s.locked && s.state.is_open())
            .map(|s| (s.id.clone(), s.link.clone()))
            .collect();
        for (id, link) in &open {
            if let Some(link) = link {
                let _ = link.send(&AppMessage::Close);
            }
            // recorded now: the shims' own reports come after NativeTerm is
            // gone, and one never found (no link) is given up as well
            self.shared.update(id, |s| s.state = State::Closed);
        }
        open.len()
    }

    /// Whether NativeTerm closes its tabs when it exits (on unless turned off).
    pub fn close_on_exit(&self) -> bool {
        self.setting(CLOSE_ON_EXIT_SETTING).as_deref() != Some("0")
    }

    /// Lock or unlock a session (remembered across restarts).
    pub fn set_locked(&self, id: &str, locked: bool) {
        if self.shared.update(id, |s| s.locked = locked).is_some() {
            self.shared.db("lock", |r| r.set_locked(id, locked));
        }
    }

    /// Open the same host again (without port forwards), in the most
    /// recent Terminal window.
    pub fn clone_session(&self, id: &str) {
        let Some(s) = self.sessions().into_iter().find(|s| s.id == id) else { return };
        let label = self.shared.fresh_label(&s.alias, &s.label);
        let on_login = lock(&self.shared.sessions).iter().find(|x| x.id == id).and_then(|x| x.on_login.clone());
        self.open(&[HostRequest { no_forwards: true, on_login, ..HostRequest::new(s.alias, label) }], Target::Recent);
    }

    /// Connect a waiting session, or reconnect an ended one.
    pub fn connect(&self, id: &str) {
        self.send(id, AppMessage::Connect);
    }

    /// Connect several sessions through the connection queue, so jump
    /// hosts and the machine aren't hit all at once.
    pub fn connect_all(&self, ids: Vec<String>) {
        if ids.len() > 1 {
            toast::info(t!("toast-connecting", count = ids.len()));
        }
        self.shared.queue_connect(&ids);
    }

    /// Copy `state.db` to `path` (for moving the data directory).
    pub fn copy_state_to(&self, path: &Path) -> io::Result<()> {
        match &self.shared.registry {
            Some(registry) => registry.copy_to(path).map_err(io::Error::other),
            None => Err(io::Error::other("state.db isn't open")),
        }
    }

    /// Keep the settings in `settings.toml` from now on (the file is the
    /// place for them; see `settings.rs`).
    pub fn set_settings(&self, settings: Arc<crate::settings::Settings>) {
        // what was read from state.db before the file was known
        let auto = settings.get(AUTO_RECONNECT_SETTING).as_deref() == Some("1");
        *lock(&self.shared.settings) = Some(settings);
        self.shared.auto_reconnect.store(auto, std::sync::atomic::Ordering::Relaxed);
    }

    /// A setting: from `settings.toml`, or from `state.db` while no file
    /// is in use.
    pub fn setting(&self, key: &str) -> Option<String> {
        if let Some(settings) = lock(&self.shared.settings).as_ref() {
            return settings.get(key);
        }
        self.shared.registry.as_ref()?.setting(key).ok().flatten()
    }

    pub fn set_setting(&self, key: &str, value: &str) {
        let settings = lock(&self.shared.settings).clone();
        if let Some(settings) = settings {
            if let Err(e) = settings.set(key, value) {
                let path = settings.path().display().to_string();
                self.shared.notice(t!("notice-settings-not-written", path = path, error = e.to_string()));
            }
            return;
        }
        self.shared.db("setting", |r| r.set_setting(key, value));
    }

    /// The chosen language (`None`: the system's).
    pub fn language_setting(&self) -> Option<String> {
        self.setting(i18n::SETTING).filter(|l| !l.is_empty())
    }

    /// Choose the language (`None`: the system's), now and at the next start.
    pub fn set_language_setting(&self, choice: Option<&str>) {
        i18n::set_language(choice);
        self.shared.db("setting", |r| r.set_setting(i18n::SETTING, choice.unwrap_or("")));
        self.shared.changed();
    }

    pub fn auto_reconnect(&self) -> bool {
        self.shared.auto_reconnect.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Turn automatic reconnects on or off (remembered in the settings).
    pub fn set_auto_reconnect(&self, on: bool) {
        self.shared.auto_reconnect.store(on, std::sync::atomic::Ordering::Relaxed);
        self.set_setting(AUTO_RECONNECT_SETTING, if on { "1" } else { "0" });
    }

    /// Clear the tab's scrollback and screen.
    pub fn clear_screen(&self, id: &str) {
        self.send(id, AppMessage::ClearScreen);
    }

    /// Send one of the connection's special commands (see
    /// `SessionView::specials`), e.g. a serial line's Break.
    /// The tab's console screen, if it has been asked for and answered.
    #[must_use]
    pub fn screen(&self, id: &str) -> Option<Screen> {
        lock(&self.shared.sessions).iter().find(|s| s.id == id).and_then(|s| s.screen.clone())
    }

    /// Asks the tab for its console screen, at most every `every`. The
    /// answer arrives as `ShimMessage::Screen` and is kept for `screen`.
    pub fn ask_screen(&self, id: &str, every: Duration) {
        let link = self.shared.update(id, |s| {
            let fresh = s.screen.as_ref().is_some_and(|screen| screen.asked.elapsed() < every);
            if fresh || !s.state.is_open() {
                return None;
            }
            // remember the moment, so that several frames ask once
            if let Some(screen) = s.screen.as_mut() {
                screen.asked = Instant::now();
            }
            s.link.clone()
        });
        let Some(Some(link)) = link else { return };
        let _ = link.send(&AppMessage::Screen);
        // the first answer has nothing to stamp: remember it here
        self.shared.update(id, |s| {
            if s.screen.is_none() {
                s.screen = Some(Screen { columns: 0, lines: Vec::new(), at: SystemTime::now(), asked: Instant::now() });
            }
        });
    }

    pub fn send_special(&self, id: &str, name: &str) {
        self.send(id, AppMessage::Special { name: name.to_string() });
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
                    Ok(false) => shared.notice(t!("notice-tab-changed", label = label.as_str())),
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
        let mut sessions = lock(&self.shared.sessions);
        let before = sessions.len();
        sessions.retain(|s| s.state.is_open());
        let gone = before - sessions.len();
        drop(sessions);
        // the rows simply vanish; this says how many did
        if gone > 0 {
            toast::done(t!("toast-cleared", count = gone));
        }
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
            Some((label, None)) => self.shared.notice(t!("notice-not-linked", label = label.as_str())),
            None => {}
        }
    }
}

/// Whether the shim recorded for a session still runs (unknown: assumed,
/// the tab is looked for as before).
fn shim_alive(shim: Option<(u32, u64)>) -> bool {
    shim.is_none_or(|(pid, started)| native_term_os::process::started(pid) == Some(started))
}

fn record_for(spec: &TabSpec) -> Record {
    Record {
        id: spec.session.clone(),
        terminal_session: spec.terminal_session.clone(),
        current_terminal_session: None,
        label: spec.label.clone(),
        alias: spec.alias.clone(),
        opened_at: registry::now(),
        window_number: None,
        tab_index: None,
        no_forwards: spec.no_forwards,
        locked: false,
        shim: None,
    }
}

/// `label`, or `label (2)`, `label (3)`, … if taken.
pub fn unique_label(label: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(label) {
        return label.to_string();
    }
    (2..).map(|n| format!("{label} ({n})")).find(|l| !taken.contains(l)).expect("unbounded")
}

/// Open tabs and confirm them; returns the sessions whose tab didn't show.
///
/// A tab that never appears is asked for once more (see `resend`), in the
/// window this try used.
fn open_tabs(shared: &Shared, target: &Target, specs: &[TabSpec]) -> Vec<String> {
    let fail = |specs: &[TabSpec], why: &str| {
        for spec in specs {
            shared.update(&spec.session, |s| {
                if s.state == State::Opening {
                    s.state = State::Failed(why.to_string());
                }
            });
        }
        specs.iter().map(|s| s.session.clone()).collect::<Vec<_>>()
    };
    let mut failed = Vec::new();
    let mut target = target.clone();
    let mut specs = specs.to_vec();
    let mut once_more = true;
    loop {
        let report = match shared.terminal.open(&target, &specs) {
            Ok(report) => report,
            Err(e) => {
                shared.notice(t!("notice-terminal-failed", error = e.to_string()));
                failed.extend(fail(&specs, "wt failed"));
                break;
            }
        };
        if !report.pending.is_empty() {
            shared.notice(t!("notice-tabs-pending", count = report.pending.len()));
            failed.extend(fail(&report.pending, "not sent"));
        }
        let sent = specs[..report.launched].to_vec();
        let expected: Vec<String> = sent.iter().map(|s| s.label.clone()).collect();
        let (snapshot, missing) = shared.terminal.wait_for(&shared.labels(), &expected, CONFIRM);
        if missing.is_empty() {
            break;
        }
        let gone: Vec<TabSpec> = sent.into_iter().filter(|s| missing.contains(&s.label)).collect();
        // one more try, unless a window could not be read (the tab may be
        // in it, and a second one would be a duplicate) or its shim is
        // already talking to us (then the tab is there under another name)
        let again: Vec<TabSpec> = gone
            .iter()
            .filter(|s| shared.update(&s.session, |s| waiting_to_open(s)).unwrap_or(false))
            .cloned()
            .collect();
        if once_more && snapshot.complete && !again.is_empty() {
            once_more = false;
            shared.notice(t!("notice-tabs-resent", count = again.len(), labels = label_list(&again)));
            // `-w 0` goes to the most recently activated window: the one
            // this try made, so the tabs don't land in a window of their own
            if let Some(handle) = report.window {
                shared.terminal.activate(handle);
                target = Target::Recent;
            }
            specs = again.iter().filter_map(|s| resend(shared, s)).collect();
            if !specs.is_empty() {
                continue;
            }
        }
        shared.notice(t!("notice-tabs-missing", count = gone.len(), labels = label_list(&gone)));
        failed.extend(fail(&gone, "tab didn't appear"));
        break;
    }
    shared.refresh_soon();
    failed
}

fn label_list(specs: &[TabSpec]) -> String {
    specs.iter().map(|s| s.label.as_str()).collect::<Vec<_>>().join(", ")
}

/// Whether a session is still waiting for the tab that was asked for: no
/// shim of its own has spoken, and nothing else has moved it on.
fn waiting_to_open(s: &Session) -> bool {
    s.state == State::Opening && s.link.is_none()
}

/// The same session, asked for again under a new terminal GUID: the first
/// one may yet turn up (a Terminal that was busy), and two tabs claiming
/// one GUID cannot be told apart. The label and NativeTerm's own session
/// id stay, so the tab is claimed and the host is the same.
fn resend(shared: &Shared, spec: &TabSpec) -> Option<TabSpec> {
    let spec = shared.update(&spec.session, |s| {
        s.terminal_session = native_term_config::new_id();
        s.current_terminal_session = None;
        TabSpec { terminal_session: s.terminal_session.clone(), ..spec.clone() }
    })?;
    shared.db("resend", |r| r.opened(&record_for(&spec)));
    Some(spec)
}

/// Replace restored placeholders with proper tabs (same label and
/// session, a new terminal GUID, not connected), then close them. Each
/// window's placeholders are replaced together, in that window.
fn replace_placeholders(shared: &Shared, queued: Receiver<Placeholder>) {
    while let Ok(first) = queued.recv() {
        let mut batch = vec![first];
        let deadline = Instant::now() + REPLACE_GATHER;
        while let Ok(next) = queued.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            batch.push(next);
        }
        let mut windows: Vec<Option<WindowId>> = Vec::new();
        for p in &batch {
            if !windows.contains(&p.window) {
                windows.push(p.window);
            }
        }
        for target_window in windows {
            let group: Vec<&Placeholder> = batch.iter().filter(|p| p.window == target_window).collect();
            let mut specs = Vec::new();
            for p in &group {
                let taken = shared.labels();
                let look =
                    shared.update(&p.session, |s| s.alias.clone()).map(|alias| shared.look(&alias)).unwrap_or_default();
                let spec = shared.update(&p.session, |s| {
                    // a host renamed since: the new tab gets the new name
                    let fresh = shared.fresh_label(&s.alias, &s.label);
                    if fresh != tab_menu::base_label(&s.label) {
                        s.label = unique_label(&fresh, &taken);
                    }
                    s.terminal_session = native_term_config::new_id();
                    s.current_terminal_session = None;
                    s.state = State::Opening;
                    TabSpec {
                        terminal_session: s.terminal_session.clone(),
                        label: s.label.clone(),
                        session: s.id.clone(),
                        alias: s.alias.clone(),
                        wait: true,
                        no_forwards: s.no_forwards,
                        tab_color: look.tab_color.clone(),
                    }
                });
                if let Some(spec) = spec {
                    shared.db("replace", |r| r.opened(&record_for(&spec)));
                    specs.push(spec);
                }
            }
            // `-w 0` goes to the most recently activated window
            if let Some(handle) = target_window {
                shared.terminal.activate(handle);
            }
            let failed = open_tabs(shared, &Target::Recent, &specs);
            for p in group {
                let message = if failed.contains(&p.session) { AppMessage::LocalShell } else { AppMessage::Close };
                let _ = p.conn.send(&message);
            }
        }
    }
}

/// Read all tabs and store each session's position.
fn refresh(shared: &Shared) -> Snapshot {
    scan(shared, true)
}

/// Read Terminal's tabs; with `picture`, also take the selected tabs'
/// pictures (see `previews`).
fn scan(shared: &Shared, picture: bool) -> Snapshot {
    let labels = shared.labels();
    let all_tabs = shared.all_tabs.load(std::sync::atomic::Ordering::Relaxed);
    let snapshot = if labels.is_empty() && !all_tabs {
        // nothing to look for
        Snapshot { windows: Vec::new(), complete: true }
    } else {
        shared.scans.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        shared.terminal.snapshot(&labels)
    };
    let mut changed = {
        let before = lock(&shared.snapshot);
        lock(&shared.previews).prune(&before, &snapshot);
        *before != snapshot
    };
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
    let mut moved = Vec::new();
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
                let position = |l: &Option<Location>| l.as_ref().map(|l| (l.window_number, l.tab_index));
                if found.is_some() && position(&found) != position(&s.location) {
                    moved.push((s.id.clone(), position(&found)));
                }
                s.location = found;
                changed = true;
            }
        }
    }
    for (id, position) in moved {
        shared.db("position", |r| r.moved(&id, position.map(|p| p.0 as i64), position.map(|p| p.1 as i64)));
    }
    if let Some(menu) = lock(&shared.menu).as_ref() {
        let tabs = snapshot
            .windows
            .iter()
            .flat_map(|w| {
                w.tabs.iter().filter_map(move |t| {
                    let claim = t.claim.as_ref()?;
                    Some(MenuTab {
                        window: w.handle,
                        rect: t.rect?,
                        label: claim.label.clone(),
                        title: t.name.clone(),
                        mixed: claim.mixed,
                        index: t.index,
                    })
                })
            })
            .collect();
        menu.set_tabs(tabs);
    }
    if changed {
        *lock(&shared.snapshot) = snapshot.clone();
    }
    // the selected tabs, as they look now
    let pictured = picture && previews::take(&shared.previews, &*shared.terminal, &snapshot);
    if changed || pictured {
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
                shared.notice(t!("notice-pipe-stopped", error = e.to_string()));
                return;
            }
        };
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || handle_connection(&shared, conn));
    }
}

/// Whether two paths name the same program. Windows compares paths
/// without case, and a program started through a short (8.3) name or a
/// link still runs the same file, so both are resolved when they can be.
fn same_program(one: &Path, two: &Path) -> bool {
    if one.as_os_str().eq_ignore_ascii_case(two.as_os_str()) {
        return true;
    }
    match (std::fs::canonicalize(one), std::fs::canonicalize(two)) {
        (Ok(one), Ok(two)) => one == two,
        _ => false,
    }
}

/// Only NativeTerm's own helper may speak on the pipe.
///
/// The pipe's ACL already keeps other users out (and remote clients are
/// refused), but any program of this user could otherwise open it, claim
/// a session's GUID and be handed what was meant for that tab. The
/// connecting process must therefore be the very `nativeterm-shim.exe`
/// NativeTerm starts its tabs with (see "Named pipe access" in
/// `docs/ARCHITECTURE.md`).
fn our_shim(shared: &Shared, conn: &PipeConnection) -> Result<(), String> {
    let pid = conn.client_pid().map_err(|e| e.to_string())?;
    let image = native_term_os::process::image(pid).ok_or_else(|| format!("pid {pid}"))?;
    match same_program(&image, shared.terminal.shim_path()) {
        true => Ok(()),
        false => Err(image.display().to_string()),
    }
}

fn handle_connection(shared: &Arc<Shared>, conn: Arc<PipeConnection>) {
    if let Err(what) = our_shim(shared, &conn) {
        crate::diag::line(&format!("a program that is not our helper spoke on the pipe: {what}"));
        // said once: a program that loops must not fill the window
        if !shared.told_of_stranger.swap(true, std::sync::atomic::Ordering::Relaxed) {
            shared.notice(t!("notice-pipe-stranger", program = what));
        }
        conn.close();
        return;
    }
    let Ok(Some(ShimMessage::Hello { protocol, role, pid, wt_session, session, alias, terminal_window })) =
        conn.recv::<ShimMessage>(Duration::from_secs(10))
    else {
        return;
    };
    if protocol != PROTOCOL_VERSION {
        shared.notice(t!("notice-old-shim", protocol = protocol, expected = PROTOCOL_VERSION));
    }
    let _ = conn.send(&AppMessage::Welcome { protocol: PROTOCOL_VERSION });

    if role == Role::Request {
        // a helper in a tab: one request, then it's gone
        let asked = conn.recv::<ShimMessage>(Duration::from_secs(5));
        let found = wt_session.as_deref().and_then(|guid| {
            lock(&shared.sessions)
                .iter()
                .find(|s| s.state.is_open() && s.matches_terminal_session(guid))
                .map(|s| (s.alias.clone(), s.id.clone()))
        });
        let ask = lock(&shared.ask).clone();
        if let (Ok(Some(message)), Some((alias, session)), Some(ask)) = (asked, found, ask) {
            match message {
                ShimMessage::OpenFiles => ask(tab_menu::MenuRequest::Files { alias, session }),
                ShimMessage::Dropped { paths, text } => ask(tab_menu::MenuRequest::Dropped {
                    alias,
                    session,
                    paths: paths.into_iter().map(std::path::PathBuf::from).collect(),
                    text,
                }),
                _ => {}
            }
        }
        return;
    }

    if role == Role::AuthSignal {
        // the LocalCommand helper: one message, then it's gone
        if let Ok(Some(ShimMessage::Authenticated)) = conn.recv::<ShimMessage>(Duration::from_secs(5)) {
            if let Some(guid) = wt_session {
                let id =
                    lock(&shared.sessions).iter().find(|s| s.matches_terminal_session(&guid)).map(|s| s.id.clone());
                if let Some(id) = id {
                    shared.update(&id, |s| {
                        s.authenticated = true;
                        s.state = State::Connected;
                    });
                }
            }
        }
        return;
    }

    let Some(alias) = alias else {
        // a restored pane of a session NativeTerm knows gets replaced;
        // anything else (duplicated panes, unknown GUIDs) is a local shell
        let restored = wt_session.as_deref().and_then(|guid| {
            let detached = {
                let mut sessions = lock(&shared.sessions);
                let taken: HashSet<String> =
                    sessions.iter().filter(|s| s.state.is_open()).map(|s| s.label.clone()).collect();
                sessions
                    .iter_mut()
                    .filter(|s| s.link.is_none() && s.matches_terminal_session(guid))
                    .find(|s| s.state == State::Detached || (s.restorable && !s.state.is_open()))
                    .map(|s| {
                        if s.restorable {
                            // closed with its window earlier in this run
                            s.restorable = false;
                            s.label = unique_label(&s.label, &taken);
                            s.state = State::Detached;
                        }
                        s.id.clone()
                    })
            };
            detached.or_else(|| {
                // closed with its window: bring it back into the list
                let mut restorable = lock(&shared.restorable);
                let index = restorable.iter().position(|s| s.matches_terminal_session(guid))?;
                let mut s = restorable.remove(index);
                drop(restorable);
                let mut sessions = lock(&shared.sessions);
                let taken: HashSet<String> =
                    sessions.iter().filter(|s| s.state.is_open()).map(|s| s.label.clone()).collect();
                s.label = unique_label(&s.label, &taken);
                s.state = State::Detached;
                let id = s.id.clone();
                sessions.push(s);
                Some(id)
            })
        });
        match restored {
            Some(session) => {
                let _ = conn.send(&AppMessage::Hold);
                let placeholder = Placeholder { session, conn, window: terminal_window.map(WindowId::from_wire) };
                let _ = lock(&shared.placeholders).send(placeholder);
            }
            None => {
                let _ = conn.send(&AppMessage::LocalShell);
            }
        }
        return;
    };

    let known = lock(&shared.sessions)
        .iter()
        .find(|s| {
            session.as_deref() == Some(s.id.as_str())
                || wt_session.as_deref().is_some_and(|g| s.matches_terminal_session(g))
        })
        .map(|s| s.id.clone());
    let id = match known {
        Some(id) => id,
        None => {
            // a tab from an earlier run that state.db doesn't know: adopt it
            let (id, label) = {
                let mut sessions = lock(&shared.sessions);
                let taken: HashSet<String> =
                    sessions.iter().filter(|s| s.state.is_open()).map(|s| s.label.clone()).collect();
                let id = session.clone().unwrap_or_else(native_term_config::new_id);
                let label = unique_label(&alias, &taken);
                let guid = wt_session.clone().unwrap_or_default();
                sessions.push(Session::new(id.clone(), guid, label.clone(), alias.clone(), State::Connecting));
                (id, label)
            };
            let record = Record {
                id: id.clone(),
                terminal_session: wt_session.clone().unwrap_or_default(),
                current_terminal_session: None,
                label,
                alias: alias.clone(),
                opened_at: registry::now(),
                window_number: None,
                tab_index: None,
                // unknown; the shim keeps its own command line anyway
                no_forwards: false,
                locked: false,
                shim: None,
            };
            shared.db("adopt", |r| r.opened(&record));
            id
        }
    };
    let current = shared
        .update(&id, |s| {
            let differs = wt_session.as_deref().is_some_and(|g| !g.eq_ignore_ascii_case(&s.terminal_session));
            s.current_terminal_session = if differs { wt_session.clone() } else { None };
            s.shim_pid = Some(pid);
            s.link = Some(Arc::clone(&conn));
            if !s.state.is_open() || matches!(s.state, State::Opening | State::Detached) {
                s.state = State::Connecting;
            }
            s.current_terminal_session.clone()
        })
        .flatten();
    shared.db("hello", |r| r.seen_terminal_session(&id, current.as_deref()));
    if let Some(started) = native_term_os::process::started(pid) {
        shared.db("shim", |r| r.set_shim(&id, pid, started));
    }
    shared.refresh_soon();

    loop {
        match conn.recv::<ShimMessage>(Duration::from_secs(3600)) {
            Ok(Some(message)) => {
                let (window, retry, login, lost_connect) = shared
                    .update(&id, |s| {
                        let was_connected = s.state == State::Connected;
                        // we had told it to connect (see below)
                        let was_connecting = s.state == State::Connecting;
                        apply(s, &message);
                        let logged_in = s.state == State::Connected && !was_connected;
                        let login = if logged_in { s.on_login.clone() } else { None };
                        let exited = matches!(message, ShimMessage::Exited { .. });
                        let retry = match &s.state {
                            State::Disconnected(_) if exited => Some(Retry::AfterDrop),
                            _ if exited && s.retrying() => Some(Retry::AfterFailedRetry),
                            _ => None,
                        };
                        // it says it is waiting although it was told to
                        // connect: the message went down with a link that
                        // broke (the shim replays its state when it comes
                        // back), so it has to be told again
                        let lost = was_connecting && matches!(message, ShimMessage::Waiting);
                        (s.location.as_ref().map(|l| l.window), retry.map(|r| (s.attempt, r)), login, lost)
                    })
                    .map_or((None, None, None, false), |(w, r, l, lost)| (Some(w), Some(r), l, lost));
                if lost_connect {
                    shared.queue_connect(std::slice::from_ref(&id));
                }
                if let Some(command) = login {
                    Core { shared: Arc::clone(shared) }.send_text(std::slice::from_ref(&id), &command, true);
                }
                if let Some((attempt, why)) = retry.flatten() {
                    schedule_reconnect(shared, &id, attempt, why);
                }
                if let ShimMessage::PasswordRefused = &message {
                    let label = shared.update(&id, |s| s.label.clone()).unwrap_or_default();
                    shared.notice(t!("notice-password-refused", label = label.as_str()));
                }
                if let ShimMessage::Closing = &message {
                    match window.flatten() {
                        Some(window) => check_window_closed(shared, &id, window),
                        None => debug(shared, format!("{id}: closing, but its window isn't known")),
                    }
                }
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

/// Waits before automatic reconnect number n (1-based): quick at first,
/// then once a minute.
fn reconnect_delay(n: u32) -> Duration {
    const STEPS: [u64; 5] = [3, 10, 30, 60, 60];
    Duration::from_secs(STEPS[(n.max(1) as usize - 1).min(STEPS.len() - 1)])
}

/// The retry count after a drop: a connection that stayed up for a while
/// starts over; one that drops right after login keeps counting, so a
/// host that accepts and then closes isn't retried forever.
fn next_retry(previous: u32, connected_for: Option<Duration>) -> u32 {
    let stable = connected_for.is_some_and(|d| d >= STABLE_CONNECTION);
    if stable {
        1
    } else {
        previous + 1
    }
}

/// Why an automatic reconnect is scheduled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Retry {
    /// The connection dropped after a login.
    AfterDrop,
    /// An automatic reconnect never reached the server (the network is
    /// still down): it counts as the next try.
    AfterFailedRetry,
}

/// A dropped session (never a failed first login) is connected again after
/// a while, if the setting is on and nothing happened to it meanwhile; a
/// reconnect that fails is tried again, up to `AUTO_RECONNECT_TRIES`.
fn schedule_reconnect(shared: &Arc<Shared>, id: &str, attempt: u32, why: Retry) {
    use std::sync::atomic::Ordering;
    if !shared.auto_reconnect.load(Ordering::Relaxed) {
        return;
    }
    let Some(n) = shared.update(id, |s| {
        s.auto_retries = match why {
            Retry::AfterDrop => next_retry(s.auto_retries, s.connected_at.map(|t| t.elapsed())),
            // the last login's age says nothing here: keep counting
            Retry::AfterFailedRetry => s.auto_retries + 1,
        };
        s.auto_retries
    }) else {
        return;
    };
    if n > AUTO_RECONNECT_TRIES {
        let label = shared
            .update(id, |s| {
                s.auto_retries = 0;
                s.retry_attempt = None;
                s.label.clone()
            })
            .unwrap_or_default();
        shared.notice(t!("notice-gave-up", label = label.as_str(), tries = AUTO_RECONNECT_TRIES));
        return;
    }
    // many sessions drop together (sleep, network change): spread them
    let spread = id.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b))) % 2000;
    let delay = reconnect_delay(n) + Duration::from_millis(spread);
    let shared = Arc::clone(shared);
    let id = id.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let still = shared
            .update(&id, |s| {
                let waiting = matches!(s.state, State::Disconnected(_)) || s.retrying();
                let still = waiting && s.attempt == attempt && s.link.is_some();
                if still {
                    // the shim numbers its next attempt this way
                    s.retry_attempt = Some(attempt + 1);
                }
                still
            })
            .unwrap_or(false);
        if still && shared.auto_reconnect.load(Ordering::Relaxed) {
            shared.queue_connect(&[id]);
        }
    });
}

/// What happened to a command that was sent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SendReport {
    /// Labels of the sessions that got it.
    pub sent: Vec<String>,
    /// Not logged in (or no shim): not sent.
    pub skipped: Vec<String>,
    /// The shim couldn't be reached.
    pub failed: Vec<String>,
}

/// ("YYYY-MM-DD", "HH:MM:SS") in UTC.
pub(crate) fn utc_now() -> (String, String) {
    utc(registry::now().max(0))
}

fn utc(secs: i64) -> (String, String) {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // civil from days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (format!("{year:04}-{month:02}-{day:02}"), format!("{:02}:{:02}:{:02}", rest / 3600, rest / 60 % 60, rest % 60))
}

/// Watch, while the tab's shim is ending, whether its window goes too.
fn check_window_closed(shared: &Shared, id: &str, window: WindowId) {
    let deadline = Instant::now() + WINDOW_CLOSE_CHECK;
    while Instant::now() < deadline {
        if !shared.terminal.window_ids().contains(&window) {
            shared.update(id, |s| s.restorable = true);
            shared.db("closed with window", |r| r.closed_with_window(id));
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    debug(shared, format!("{id}: closed, its window stayed"));
}

/// Put the tabs a session was last seen at first, so the locating
/// sweep usually stops after one selection. `number`: the stable window
/// number of a window handle, as the hints were written with.
fn likely_first(
    candidates: &mut [(WindowId, native_term_platform::TabView)],
    hints: &std::collections::HashSet<(usize, usize)>,
    number: impl Fn(WindowId) -> Option<usize>,
) {
    candidates.sort_by_key(|(window, tab)| {
        let hinted = number(*window).is_some_and(|w| hints.contains(&(w, tab.index)));
        (!hinted, *window, tab.index)
    });
}

/// Diagnostics as notices when `NATIVETERM_DEBUG` is set.
fn debug(shared: &Shared, text: String) {
    if std::env::var_os("NATIVETERM_DEBUG").is_some() {
        shared.notice(format!("debug: {text}"));
    }
}

fn apply(s: &mut Session, message: &ShimMessage) {
    if !matches!(
        message,
        ShimMessage::Quiet { .. }
            | ShimMessage::Hello { .. }
            | ShimMessage::Specials { .. }
            | ShimMessage::Unreachable
            | ShimMessage::PasswordRefused
            | ShimMessage::OpenFiles
    ) {
        s.quiet_since = None;
    }
    match message {
        // only from a `Request` helper, never on a session link
        ShimMessage::OpenFiles | ShimMessage::Dropped { .. } => {}
        ShimMessage::Waiting => s.state = State::Waiting,
        ShimMessage::Connecting { attempt } => {
            s.authenticated = false;
            s.unreachable = false;
            s.specials.clear();
            s.attempt = *attempt;
            s.state = State::Connecting;
        }
        ShimMessage::Authenticated => {
            s.authenticated = true;
            s.state = State::Connected;
            s.connected_at = Some(Instant::now());
        }
        ShimMessage::Exited { code } => {
            s.specials.clear();
            s.state = match classify_exit(*code, s.authenticated) {
                SessionEnd::LoginFailed if s.unreachable => State::Unreachable(*code),
                SessionEnd::LoginFailed => State::LoginFailed(*code),
                SessionEnd::Disconnected => State::Disconnected(*code),
                SessionEnd::Closed(c) => State::Ended(c),
            }
        }
        ShimMessage::Closing => s.state = State::Closed,
        ShimMessage::Quiet { since } => s.quiet_since = Some(*since),
        ShimMessage::Specials { names } => s.specials = names.clone(),
        ShimMessage::Screen { columns, lines } => {
            let asked = s.screen.as_ref().map_or_else(Instant::now, |screen| screen.asked);
            s.screen = Some(Screen { columns: *columns, lines: lines.clone(), at: SystemTime::now(), asked });
        }
        ShimMessage::Unreachable => s.unreachable = true,
        ShimMessage::PasswordRefused => {}
        // Hello is the connection's start
        ShimMessage::Heard | ShimMessage::Hello { .. } => {}
    }
}

/// `nativeterm-shim.exe` next to the running program.
pub fn default_shim_path() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(exe.with_file_name(format!("nativeterm-shim{}", std::env::consts::EXE_SUFFIX)))
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
        Session::new(
            "s".into(),
            "6E7A0000-0000-4000-8000-000000000001".into(),
            "web01".into(),
            "web01".into(),
            State::Opening,
        )
    }

    #[test]
    fn lifecycle() {
        let mut s = session();
        apply(&mut s, &ShimMessage::Waiting);
        assert!(s.state.can_connect());
        apply(&mut s, &ShimMessage::Connecting { attempt: 1 });
        assert_eq!(s.state, State::Connecting);
        apply(&mut s, &ShimMessage::Exited { code: 255 });
        assert_eq!(s.state, State::LoginFailed(255));
        apply(&mut s, &ShimMessage::Connecting { attempt: 2 });
        apply(&mut s, &ShimMessage::Authenticated);
        assert_eq!(s.state, State::Connected);
        assert!(!s.state.can_connect());
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
    fn reconnect_delays_grow_and_level_off() {
        assert_eq!(reconnect_delay(1), Duration::from_secs(3));
        assert_eq!(reconnect_delay(2), Duration::from_secs(10));
        assert_eq!(reconnect_delay(4), Duration::from_secs(60));
        assert_eq!(reconnect_delay(9), Duration::from_secs(60));
        assert_eq!(reconnect_delay(0), Duration::from_secs(3));
    }

    #[test]
    fn only_a_stable_connection_resets_the_retry_count() {
        assert_eq!(next_retry(0, None), 1);
        assert_eq!(next_retry(3, Some(Duration::from_secs(2))), 4, "dropped right after login");
        assert_eq!(next_retry(3, Some(Duration::from_secs(600))), 1);
        let mut s = session();
        s.auto_retries = 3;
        apply(&mut s, &ShimMessage::Connecting { attempt: 4 });
        assert_eq!(s.view().auto_retry, Some(3));
        apply(&mut s, &ShimMessage::Authenticated);
        assert!(s.connected_at.is_some());
        assert_eq!(s.view().auto_retry, None, "shown only while reconnecting");
    }

    /// The sweep selects tabs one by one, which the person sees, so a
    /// tab a session was last seen at is tried first.
    #[test]
    fn where_a_session_was_last_seen_is_looked_at_first() {
        let tab = |index: usize| native_term_platform::TabView {
            index,
            name: format!("tab {index}"),
            selected: false,
            rect: None,
            claim: None,
        };
        // window handle 10 is window 1, handle 20 is window 2
        let number = |handle: WindowId| match handle.0 {
            10 => Some(1),
            20 => Some(2),
            _ => None,
        };
        let w = WindowId;
        let mut candidates = vec![(w(10), tab(0)), (w(10), tab(3)), (w(20), tab(1)), (w(20), tab(5))];
        let hints = std::collections::HashSet::from([(2, 1), (1, 3)]);
        likely_first(&mut candidates, &hints, number);
        let order: Vec<(u64, usize)> = candidates.iter().map(|(w, t)| (w.0, t.index)).collect();
        assert_eq!(order, vec![(10, 3), (20, 1), (10, 0), (20, 5)]);

        // without hints the order is steady (window, then strip order)
        let mut candidates = vec![(w(20), tab(5)), (w(10), tab(3)), (w(10), tab(0))];
        likely_first(&mut candidates, &Default::default(), number);
        let order: Vec<(u64, usize)> = candidates.iter().map(|(w, t)| (w.0, t.index)).collect();
        assert_eq!(order, vec![(10, 0), (10, 3), (20, 5)]);

        // a window NativeTerm has no number for is simply not hinted
        let mut candidates = vec![(w(30), tab(1)), (w(10), tab(3))];
        likely_first(&mut candidates, &hints, number);
        assert_eq!(candidates[0].0, w(10));
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc(0), ("1970-01-01".to_string(), "00:00:00".to_string()));
        assert_eq!(utc(1_789_000_000), ("2026-09-10".to_string(), "00:26:40".to_string()));
        assert_eq!(utc(951_782_400).0, "2000-02-29");
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
