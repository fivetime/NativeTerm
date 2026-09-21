//! Files over SFTP (`native_term_sftp`), SecureFX-style: one window for
//! every session, local files on the left and the server's on the right,
//! each side with a tab per session; the two tab rows move together.
//! Files go across with the buttons between them or by dragging from one
//! side to the other (and from Explorer onto the server's side). The
//! server's side also renames, deletes, creates folders and edits in
//! place: a file is downloaded, opened with its program, and uploaded
//! again whenever it is saved. A session is opened from a host's menu or
//! a terminal tab's menu; for a tab kept in tmux the server's side starts
//! in the tab's current folder. Everything that touches the network or
//! the disk runs on a thread; the window only shows what comes back.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use native_term_app::t;
use native_term_sftp::transfer::{self, Item, Progress};
use native_term_sftp::wire::Attrs;
use native_term_sftp::{Entry, Names, Session};

use crate::icons;

pub const RED: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
pub const GREEN: egui::Color32 = egui::Color32::from_rgb(0x2e, 0xa0, 0x43);
const ROW: f32 = 22.0;
/// Log lines kept per session.
const LOG_LINES: usize = 200;

/// The encodings offered for file names (any `encoding_rs` label can be
/// set in the config).
const ENCODINGS: [&str; 14] = [
    "auto",
    "UTF-8",
    "GBK",
    "gb18030",
    "Big5",
    "Shift_JIS",
    "EUC-JP",
    "EUC-KR",
    "windows-1250",
    "windows-1251",
    "windows-1252",
    "KOI8-R",
    "ISO-8859-2",
    "windows-1256",
];

/// What opening a session's files needs.
pub struct Spec {
    pub alias: String,
    pub label: String,
    pub ssh: PathBuf,
    /// `-F` for a folder other than `~/.ssh`.
    pub config: Option<PathBuf>,
    pub names: Names,
    /// `nativeterm-shim.exe`: ssh's askpass helper.
    pub shim: PathBuf,
    /// The terminal tab's tmux session (a persistent session): the
    /// server's side starts in its current folder.
    pub tmux_session: Option<String>,
    /// Where the last folders per host are kept (`state.db`); none in a
    /// run without one.
    pub memory: Option<native_term_app::Core>,
    /// The host's credential set (`NativeTermCredential`), if it uses one.
    pub credential: Option<String>,
}

/// How many files a connection copies at once (`state.db`); the files
/// window's own control changes it.
pub const AT_ONCE_SETTING: &str = "files.at_once";
/// What it is without a setting, and as far as it can be set.
pub const AT_ONCE_DEFAULT: usize = 3;
pub const AT_ONCE_MAX: usize = 16;

thread_local! {
    /// Sessions asked for since the window last looked.
    static PENDING: RefCell<Vec<Spec>> = const { RefCell::new(Vec::new()) };
    /// Files to upload to a host (its alias) as soon as its side is
    /// connected: dropped into a terminal tab, uploaded here.
    static PENDING_UPLOADS: RefCell<Vec<(String, Vec<PathBuf>)>> = const { RefCell::new(Vec::new()) };
    /// The open window's context (to wake it).
    static WINDOW: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// Opens a session in the files window (the window too, if it isn't open;
/// a host already there is shown).
pub fn open(spec: Spec) {
    PENDING.with(|p| p.borrow_mut().push(spec));
    if let Some(ctx) = WINDOW.with(|w| w.borrow().clone()) {
        ctx.request_repaint();
    }
    let viewport = egui::ViewportBuilder::default()
        .with_title(t!("files-window-title"))
        .with_inner_size([1200.0, 760.0])
        .with_min_inner_size([760.0, 420.0])
        .with_drag_and_drop(true);
    crate::window::open("files", viewport, |ctx| Box::new(FilesWindow::new(ctx)));
}

/// Uploads `files` to `alias`'s current folder, once its side of the
/// window is connected (`open` it first). Files dropped into a terminal
/// tab come this way.
pub fn upload_into(alias: &str, files: &[PathBuf]) {
    if files.is_empty() {
        return;
    }
    PENDING_UPLOADS.with(|p| p.borrow_mut().push((alias.to_string(), files.to_vec())));
    if let Some(ctx) = WINDOW.with(|w| w.borrow().clone()) {
        ctx.request_repaint();
    }
}

/// A server's directory entry as shown.
#[derive(Clone)]
struct RemoteRow {
    entry: Entry,
    name: String,
    /// A folder, or a link to one.
    dir: bool,
}

/// A local file, folder or drive.
#[derive(Clone)]
struct LocalRow {
    name: String,
    path: PathBuf,
    dir: bool,
    size: Option<u64>,
    modified: Option<u64>,
}

/// One line of a list, whichever side: `key` identifies it for selection.
struct Line {
    key: Vec<u8>,
    name: String,
    glyph: char,
    size: Option<u64>,
    modified: Option<u64>,
    mode: Option<String>,
}

/// Files dragged from one side to the other.
struct Dragged {
    tab: u64,
    from_remote: bool,
    keys: Vec<Vec<u8>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Open,
    Transfer,
    Edit,
    Rename,
    CopyPath,
    Delete,
}

/// A question from ssh (password, passphrase, new host key, code), for
/// the window to ask.
struct Question {
    prompt: String,
    /// Typed text isn't shown (all but yes/no questions).
    secret: bool,
    reply: Sender<Option<String>>,
}

enum What {
    Connected(Arc<Session>, Vec<u8>),
    Failed(String),
    Listed {
        path: Vec<u8>,
        result: Result<Vec<RemoteRow>, String>,
    },
    LocalListed {
        path: Option<PathBuf>,
        result: Result<Vec<LocalRow>, String>,
    },
    JobDone {
        job: u64,
        result: Result<(), native_term_sftp::Error>,
    },
    Notice(String, bool),
    Refresh,
    LocalRefresh,
    Ask(Question),
    /// A folder's subfolders, for a tree.
    RemoteTree {
        path: Vec<u8>,
        folders: Vec<(String, Vec<u8>)>,
    },
    LocalTree {
        path: PathBuf,
        folders: Vec<(String, PathBuf)>,
    },
    /// Synchronize's comparison.
    Compared(Result<Vec<native_term_sftp::sync::Found>, native_term_sftp::Error>),
}

/// What happened, for which session.
struct Event {
    tab: u64,
    what: What,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Upload,
    Download,
    Delete,
}

enum JobState {
    Running,
    /// Stopped with the partial file kept: Resume goes on from there.
    Paused,
    Done,
    Failed(String),
    Cancelled,
}

/// What a transfer copies, kept so it can go on after a pause or an error.
#[derive(Clone)]
enum Work {
    Upload {
        names: Names,
        files: Vec<PathBuf>,
        into: Vec<u8>,
    },
    Download {
        names: Names,
        items: Vec<(Vec<u8>, Attrs)>,
        folder: PathBuf,
    },
    /// Each to its own path (a synchronization).
    UploadPairs {
        names: Names,
        pairs: Vec<(PathBuf, Vec<u8>)>,
    },
    DownloadPairs {
        names: Names,
        pairs: Vec<(Vec<u8>, Attrs, PathBuf)>,
    },
}

impl Work {
    fn names(&self) -> &Names {
        match self {
            Work::Upload { names, .. }
            | Work::Download { names, .. }
            | Work::UploadPairs { names, .. }
            | Work::DownloadPairs { names, .. } => names,
        }
    }

    fn uploads(&self) -> bool {
        matches!(self, Work::Upload { .. } | Work::UploadPairs { .. })
    }
}

struct Job {
    id: u64,
    tab: u64,
    host: String,
    kind: Kind,
    title: String,
    progress: Arc<Progress>,
    state: JobState,
    started: Instant,
    finished: Option<Instant>,
    /// Transfers (not deletes): what to copy and, once planned, the plan.
    work: Option<Work>,
    plan: Arc<Mutex<Option<Vec<Item>>>>,
}

impl Job {
    /// Bytes per second since it (re)started (what was already copied
    /// before doesn't count).
    fn speed(&self) -> f64 {
        let secs = self.finished.unwrap_or_else(Instant::now).duration_since(self.started).as_secs_f64().max(0.001);
        let p = &self.progress;
        p.done.load(Ordering::Relaxed).saturating_sub(p.skipped.load(Ordering::Relaxed)) as f64 / secs
    }
}

/// What a button in the queue asks for.
enum JobAction {
    Pause,
    Resume,
    Cancel,
    Remove,
}

/// A file being edited: uploaded again whenever its local copy changes.
struct Edit {
    name: String,
    local: PathBuf,
    status: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
    /// Set when the file on the server changed meanwhile: an upload waits
    /// for "Overwrite".
    conflict: Arc<AtomicBool>,
    overwrite: Arc<AtomicBool>,
}

/// The server's side of a session.
struct Remote {
    sftp: Option<Arc<Session>>,
    /// How many files this connection copies at once (`AT_ONCE_SETTING`),
    /// shared by all of its transfers.
    slots: Arc<native_term_sftp::transfer::Slots>,
    failed: Option<String>,
    path: Vec<u8>,
    path_text: String,
    rows: Vec<RemoteRow>,
    listing: bool,
    error: Option<String>,
    selected: HashSet<Vec<u8>>,
    anchor: Option<usize>,
    renaming: Option<(Vec<u8>, String)>,
    names: Names,
}

/// The local side of a session: a folder, or `None` for the drives.
struct Local {
    path: Option<PathBuf>,
    path_text: String,
    rows: Vec<LocalRow>,
    error: Option<String>,
    selected: HashSet<Vec<u8>>,
    anchor: Option<usize>,
    renaming: Option<(Vec<u8>, String)>,
}

/// A folder tree beside a list: each folder's subfolders once read, and
/// which folders are open. Folders are read when opened; the folders
/// above the one shown are opened by themselves.
struct Tree<P> {
    nodes: HashMap<P, Node<P>>,
}

struct Node<P> {
    /// Name and path of each subfolder; `None` until read.
    children: Option<Vec<(String, P)>>,
    open: bool,
    loading: bool,
}

impl<P> Default for Tree<P> {
    fn default() -> Tree<P> {
        Tree { nodes: HashMap::new() }
    }
}

impl<P: Clone + Eq + Hash> Tree<P> {
    fn node(&mut self, path: &P) -> &mut Node<P> {
        self.nodes.entry(path.clone()).or_insert(Node { children: None, open: false, loading: false })
    }

    /// Opens `path`; whether its subfolders need reading.
    fn open(&mut self, path: &P) -> bool {
        let node = self.node(path);
        node.open = true;
        let read = node.children.is_none() && !node.loading;
        if read {
            node.loading = true;
        }
        read
    }
}

struct Tab {
    id: u64,
    spec: Spec,
    remote: Remote,
    local: Local,
    remote_tree: Tree<Vec<u8>>,
    local_tree: Tree<PathBuf>,
    edits: Vec<Edit>,
    log: Vec<(String, String, bool)>,
    /// The folders last written to `memory` (written again only when
    /// another is shown).
    kept: (Option<Vec<u8>>, Option<PathBuf>),
}

/// What a confirmation is about.
enum Confirm {
    DeleteRemote { tab: u64, items: Vec<(Vec<u8>, Attrs)>, what: String, folders: bool },
    RecycleLocal { tab: u64, paths: Vec<PathBuf>, what: String },
    CloseTab(u64),
    CloseWindow,
}

struct FilesWindow {
    ctx: egui::Context,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    jobs: Vec<Job>,
    /// A new folder: session, server's side, name.
    new_folder: Option<(u64, bool, String)>,
    confirm: Option<Confirm>,
    close: bool,
    /// ssh's question being asked (session, question, answer typed).
    question: Option<(u64, Question, String)>,
    /// The side keys go to (clicked last): the server's.
    remote_focus: bool,
    /// Where the server's side is (for files dropped from Explorer).
    remote_rect: Option<egui::Rect>,
    edit_dir: PathBuf,
    computer: String,
    /// The local tree's top: Desktop, Documents, Downloads, the drives.
    local_roots: Vec<(String, PathBuf)>,
    /// Synchronize: the folders compared, what was found, what to do.
    sync: Option<crate::files_sync::SyncDialog>,
}

impl FilesWindow {
    fn new(ctx: &egui::Context) -> FilesWindow {
        WINDOW.with(|w| *w.borrow_mut() = Some(ctx.clone()));
        let (tx, rx) = mpsc::channel();
        FilesWindow {
            ctx: ctx.clone(),
            tx,
            rx,
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            jobs: Vec::new(),
            new_folder: None,
            confirm: None,
            close: false,
            question: None,
            remote_focus: true,
            remote_rect: None,
            edit_dir: std::env::temp_dir().join("NativeTerm-edit"),
            computer: std::env::var("COMPUTERNAME").unwrap_or_default(),
            local_roots: native_term_win::shell::user_folders()
                .into_iter()
                .chain(native_term_win::shell::drives())
                .map(|p| {
                    (p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(p.display().to_string()), p)
                })
                .collect(),
            sync: None,
        }
    }

    fn tab(&mut self, id: u64) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// Runs `work` on a thread; its event comes back with a repaint.
    fn spawn(&self, tab: u64, work: impl FnOnce() -> What + Send + 'static) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Event { tab, what: work() });
            ctx.request_repaint();
        });
    }

    fn log(&mut self, tab: u64, text: String, error: bool) {
        if let Some(t) = self.tab(tab) {
            let time = native_term_win::local_time_of_day(unix_now());
            t.log.push((time, text, error));
            if t.log.len() > LOG_LINES {
                t.log.remove(0);
            }
        }
    }

    /// Sessions asked for: a new tab, or the host's tab shown (and moved
    /// to the terminal's folder, if it says one).
    /// Starts the uploads asked for with `upload_into`, once the host's
    /// side is connected and its folder known. Anything for a host that
    /// isn't there (or isn't connected yet) waits.
    fn take_pending_uploads(&mut self, ctx: &egui::Context) {
        let waiting = PENDING_UPLOADS.with(|p| std::mem::take(&mut *p.borrow_mut()));
        if waiting.is_empty() {
            return;
        }
        let mut keep = Vec::new();
        for (alias, files) in waiting {
            let ready = self
                .tabs
                .iter()
                .find(|t| t.spec.alias == alias)
                .filter(|t| t.remote.sftp.is_some() && !t.remote.path.is_empty())
                .map(|t| t.id);
            match ready {
                Some(id) => self.upload_to(id, files, None),
                None => keep.push((alias, files)),
            }
        }
        if !keep.is_empty() {
            // the host is still connecting: look again in a moment
            PENDING_UPLOADS.with(|p| p.borrow_mut().extend(keep));
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Where settings are kept (`state.db`), from whichever tab has it.
    fn core(&self) -> Option<&native_term_app::Core> {
        self.tabs.iter().find_map(|t| t.spec.memory.as_ref())
    }

    /// How many files a connection copies at once.
    fn at_once(&self) -> usize {
        self.core()
            .and_then(|core| core.setting(AT_ONCE_SETTING))
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(AT_ONCE_DEFAULT)
            .clamp(1, AT_ONCE_MAX)
    }

    /// Changes it for the connections that are open, and keeps it.
    fn set_at_once(&mut self, at_once: usize) {
        let at_once = at_once.clamp(1, AT_ONCE_MAX);
        if let Some(core) = self.core() {
            core.set_setting(AT_ONCE_SETTING, &at_once.to_string());
        }
        for tab in &self.tabs {
            tab.remote.slots.set_width(at_once);
        }
    }

    fn take_pending(&mut self) {
        let pending = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
        for spec in pending {
            if let Some(i) = self.tabs.iter().position(|t| t.spec.alias == spec.alias) {
                self.active = i;
                if let Some(session) = spec.tmux_session.clone() {
                    let id = self.tabs[i].id;
                    self.tabs[i].spec.tmux_session = Some(session);
                    self.follow_terminal(id);
                }
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            let names = spec.names;
            let at_once = self.at_once();
            self.tabs.push(Tab {
                id,
                spec,
                remote: Remote {
                    sftp: None,
                    slots: Arc::new(native_term_sftp::transfer::Slots::new(at_once)),
                    failed: None,
                    path: Vec::new(),
                    path_text: String::new(),
                    rows: Vec::new(),
                    listing: false,
                    error: None,
                    selected: HashSet::new(),
                    anchor: None,
                    renaming: None,
                    names,
                },
                local: Local {
                    path: None,
                    path_text: String::new(),
                    rows: Vec::new(),
                    error: None,
                    selected: HashSet::new(),
                    anchor: None,
                    renaming: None,
                },
                remote_tree: Tree::default(),
                local_tree: Tree::default(),
                edits: Vec::new(),
                log: Vec::new(),
                kept: (None, None),
            });
            self.active = self.tabs.len() - 1;
            self.connect(id);
            self.start_local(id);
        }
    }

    /// The local side's first folder: the one shown last for this host if
    /// it is still there, else Downloads (`NATIVETERM_LOCAL_START`, for
    /// tests, before both). Read and listed in the background.
    fn start_local(&mut self, id: u64) {
        let Some(tab) = self.tab(id) else { return };
        let (memory, alias) = (tab.spec.memory.clone(), tab.spec.alias.clone());
        self.spawn(id, move || {
            let path = std::env::var_os("NATIVETERM_LOCAL_START")
                .map(PathBuf::from)
                .or_else(|| recall(memory.as_ref(), &local_key_for(&alias)).map(PathBuf::from).filter(|p| p.is_dir()))
                .or_else(native_term_win::shell::downloads_folder);
            let result = list_local(path.as_deref()).map_err(|e| e.to_string());
            What::LocalListed { path, result }
        });
    }

    /// Keeps the folder shown as this host's last (in the background; only
    /// when it changed).
    fn remember(&mut self, id: u64, remote: Option<Vec<u8>>, local: Option<PathBuf>) {
        let Some(tab) = self.tab(id) else { return };
        let Some(memory) = tab.spec.memory.clone() else { return };
        let alias = tab.spec.alias.clone();
        let mut writes = Vec::new();
        if let Some(path) = remote.filter(|p| tab.kept.0.as_ref() != Some(p)) {
            writes.push((remote_key_for(&alias), hex(&path)));
            tab.kept.0 = Some(path);
        }
        if let Some(path) = local.filter(|p| tab.kept.1.as_ref() != Some(p)) {
            writes.push((local_key_for(&alias), path.display().to_string()));
            tab.kept.1 = Some(path);
        }
        if writes.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            if let Some(registry) = memory.registry() {
                for (key, value) in writes {
                    let _ = registry.set_setting(&key, &value);
                }
            }
        });
    }

    /// Connects in the background. ssh's questions come here through the
    /// shim as askpass helper and a private pipe: the account's saved
    /// password (Credential Manager) answers its own password prompt, as in
    /// a terminal tab; everything else is asked in this window.
    fn connect(&mut self, id: u64) {
        let Some(tab) = self.tab(id) else { return };
        tab.remote.failed = None;
        tab.remote.sftp = None;
        let spec = &tab.spec;
        let (ssh, config, alias, shim, tmux) =
            (spec.ssh.clone(), spec.config.clone(), spec.alias.clone(), spec.shim.clone(), spec.tmux_session.clone());
        let (memory, credential) = (spec.memory.clone(), spec.credential.clone());
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.log(id, t!("files-log-connecting", host = alias.as_str()), false);
        self.spawn(id, move || {
            let effective =
                native_term_config::effective::effective_with(&ssh, config.as_deref(), &alias).unwrap_or_default();
            let target = native_term_config::password::target(&effective, credential.as_deref());
            let saved = target.as_ref().is_some_and(|t| {
                native_term_win::credentials::read(&t.name)
                    .ok()
                    .flatten()
                    .is_some_and(|s| s.comment != native_term_config::password::REFUSED)
            });
            let ssh_pid = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let served = Arc::new(AtomicBool::new(false));
            let pipe = serve_questions(id, target.clone(), Arc::clone(&ssh_pid), Arc::clone(&served), tx, ctx);
            let askpass = pipe.ok().filter(|_| shim.exists()).map(|pipe| native_term_sftp::Askpass {
                program: shim.clone(),
                // unanswered = cancelled: ssh's console here is hidden
                env: vec![
                    ("NATIVETERM_ASKPASS".to_string(), pipe),
                    ("NATIVETERM_ASKPASS_NO_CONSOLE".to_string(), "1".to_string()),
                ],
                password_prompts: saved.then_some(1),
            });
            let connected = Session::connect(&ssh, config.as_deref(), &alias, askpass.as_ref(), |pid| {
                ssh_pid.store(pid, Ordering::SeqCst)
            });
            match connected.and_then(|sftp| sftp.realpath(b".").map(|home| (sftp, home))) {
                Ok((sftp, home)) => {
                    // the terminal tab's folder, else the one shown last for
                    // this host (if it is still a folder), else home
                    let last = || {
                        recall(memory.as_ref(), &remote_key_for(&alias))
                            .and_then(|h| unhex(&h))
                            .filter(|p| sftp.stat(p).is_ok_and(|a| a.is_dir()))
                    };
                    let start = tmux
                        .and_then(|s| terminal_folder(&ssh, config.as_deref(), &alias, &s, &sftp))
                        .or_else(last)
                        .unwrap_or(home);
                    What::Connected(Arc::new(sftp), start)
                }
                Err(e) => {
                    let text = e.to_string();
                    // the saved password was given and refused: marked, not tried again
                    if served.load(Ordering::SeqCst) && text.contains("Permission denied") {
                        if let Some(t) = &target {
                            if let Ok(Some(mut s)) = native_term_win::credentials::read(&t.name) {
                                s.comment = native_term_config::password::REFUSED.to_string();
                                let _ = native_term_win::credentials::write(&t.name, &s);
                            }
                        }
                    }
                    What::Failed(text)
                }
            }
        });
    }

    /// The server's side to the terminal tab's current folder.
    fn follow_terminal(&mut self, id: u64) {
        let Some(tab) = self.tab(id) else { return };
        let (Some(sftp), Some(session)) = (tab.remote.sftp.clone(), tab.spec.tmux_session.clone()) else { return };
        let (ssh, config, alias) = (tab.spec.ssh.clone(), tab.spec.config.clone(), tab.spec.alias.clone());
        let names = tab.remote.names;
        self.spawn(id, move || match terminal_folder(&ssh, config.as_deref(), &alias, &session, &sftp) {
            Some(path) => list_remote(&sftp, names, path),
            None => What::Refresh,
        });
    }

    fn list(&mut self, id: u64, path: Vec<u8>) {
        let Some(tab) = self.tab(id) else { return };
        let Some(sftp) = tab.remote.sftp.clone() else { return };
        tab.remote.listing = true;
        let names = tab.remote.names;
        self.spawn(id, move || list_remote(&sftp, names, path));
    }

    fn go(&mut self, id: u64, path: Vec<u8>) {
        if let Some(tab) = self.tab(id) {
            tab.remote.selected.clear();
            tab.remote.anchor = None;
            tab.remote.renaming = None;
        }
        self.list(id, path);
    }

    fn refresh(&mut self, id: u64) {
        let Some(path) = self.tab(id).map(|t| t.remote.path.clone()) else { return };
        self.list(id, path);
    }

    fn list_local(&mut self, id: u64, path: Option<PathBuf>) {
        if let Some(tab) = self.tab(id) {
            tab.local.selected.clear();
            tab.local.anchor = None;
            tab.local.renaming = None;
        }
        self.spawn(id, move || {
            let result = list_local(path.as_deref()).map_err(|e| e.to_string());
            What::LocalListed { path, result }
        });
    }

    fn refresh_local(&mut self, id: u64) {
        let Some(path) = self.tab(id).map(|t| t.local.path.clone()) else { return };
        self.list_local(id, path);
    }

    fn handle(&mut self, event: Event) {
        let id = event.tab;
        match event.what {
            What::Connected(sftp, start) => {
                let home = self.tab(id).map(|t| t.remote.names.decode(&start)).unwrap_or_default();
                if let Some(tab) = self.tab(id) {
                    tab.remote.sftp = Some(sftp);
                }
                self.log(id, t!("files-log-connected", path = home), false);
                self.go(id, start);
            }
            What::Failed(e) => {
                self.log(id, e.clone(), true);
                if let Some(tab) = self.tab(id) {
                    tab.remote.failed = Some(e);
                }
            }
            What::Listed { path, result } => {
                let mut lost = None;
                let mut expand = None;
                if let Some(tab) = self.tab(id) {
                    let r = &mut tab.remote;
                    r.listing = false;
                    match result {
                        Ok(rows) => {
                            r.path_text = r.names.decode(&path);
                            r.path = path;
                            r.rows = rows;
                            r.error = None;
                            let keys: HashSet<&Vec<u8>> = r.rows.iter().map(|x| &x.entry.name).collect();
                            r.selected.retain(|n| keys.contains(n));
                            expand = Some(r.path.clone());
                        }
                        // the connection went: say so, offer to reconnect
                        Err(e) => match r.sftp.as_ref().and_then(|s| s.closed()) {
                            Some(why) => {
                                r.failed = Some(why.clone());
                                r.sftp = None;
                                lost = Some(why);
                            }
                            None => r.error = Some(e),
                        },
                    }
                }
                if let Some(why) = lost {
                    self.log(id, why, true);
                }
                if let Some(path) = expand {
                    self.remember(id, Some(path.clone()), None);
                    self.expand_remote(id, &path);
                }
            }
            What::LocalListed { path, result } => {
                let computer = t!("files-computer");
                let mut expand = None;
                if let Some(tab) = self.tab(id) {
                    let l = &mut tab.local;
                    match result {
                        Ok(rows) => {
                            l.path_text = path.as_ref().map(|p| p.display().to_string()).unwrap_or(computer);
                            l.path = path.clone();
                            l.rows = rows;
                            l.error = None;
                            if let Some(path) = &path {
                                expand = Some(path.clone());
                            }
                        }
                        Err(e) => l.error = Some(e),
                    }
                }
                if let Some(path) = expand {
                    self.remember(id, None, Some(path.clone()));
                    self.expand_local(id, &path);
                }
            }
            What::JobDone { job, result } => {
                let mut after = None;
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == job) {
                    j.finished = Some(Instant::now());
                    j.state = match &result {
                        Ok(()) => JobState::Done,
                        Err(native_term_sftp::Error::Cancelled) if j.progress.paused() => JobState::Paused,
                        Err(native_term_sftp::Error::Cancelled) => JobState::Cancelled,
                        Err(e) => JobState::Failed(e.to_string()),
                    };
                    let line = match &j.state {
                        JobState::Done => (t!("files-log-done", what = j.title.as_str()), false),
                        JobState::Paused => (t!("files-job-paused", what = j.title.as_str()), false),
                        JobState::Cancelled => (t!("files-job-cancelled", what = j.title.as_str()), false),
                        JobState::Failed(e) => (format!("{}: {e}", j.title), true),
                        JobState::Running => (String::new(), false),
                    };
                    after = Some((j.tab, j.kind, line));
                }
                if let Some((tab, kind, (text, error))) = after {
                    self.log(tab, text, error);
                    match kind {
                        Kind::Download => self.refresh_local(tab),
                        Kind::Upload | Kind::Delete => self.refresh(tab),
                    }
                }
            }
            What::Notice(text, error) => self.log(id, text, error),
            What::Compared(result) => {
                use crate::files_sync::Scan;
                if let Some(d) = self.sync.as_mut().filter(|d| d.tab == id) {
                    d.scan = match result {
                        Ok(found) => Scan::Done(found),
                        Err(native_term_sftp::Error::Cancelled) => Scan::Failed(t!("files-sync-cancelled")),
                        Err(e) => Scan::Failed(e.to_string()),
                    };
                }
            }
            What::Refresh => self.refresh(id),
            What::LocalRefresh => self.refresh_local(id),
            What::RemoteTree { path, folders } => {
                if let Some(tab) = self.tab(id) {
                    let node = tab.remote_tree.node(&path);
                    node.children = Some(folders);
                    node.loading = false;
                }
            }
            What::LocalTree { path, folders } => {
                if let Some(tab) = self.tab(id) {
                    let node = tab.local_tree.node(&path);
                    node.children = Some(folders);
                    node.loading = false;
                }
            }
            What::Ask(question) => {
                if let Some((_, old, _)) = self.question.take() {
                    let _ = old.reply.send(None);
                }
                if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
                    self.active = i;
                }
                self.question = Some((id, question, String::new()));
            }
        }
    }

    /// Opens the server's folders from `/` down to `path` in the tree.
    fn expand_remote(&mut self, id: u64, path: &[u8]) {
        let mut chain = vec![b"/".to_vec()];
        let mut at = b"/".to_vec();
        for part in path.split(|&c| c == b'/').filter(|p| !p.is_empty()) {
            at = native_term_sftp::join(&at, part);
            chain.push(at.clone());
        }
        for folder in chain {
            self.open_remote_folder(id, folder);
        }
    }

    fn open_remote_folder(&mut self, id: u64, folder: Vec<u8>) {
        let Some(tab) = self.tab(id) else { return };
        let Some(sftp) = tab.remote.sftp.clone() else { return };
        if !tab.remote_tree.open(&folder) {
            return;
        }
        let names = tab.remote.names;
        self.spawn(id, move || {
            let mut folders: Vec<(String, Vec<u8>)> = sftp
                .read_dir(&folder)
                .unwrap_or_default()
                .into_iter()
                .filter(|e| {
                    e.attrs.is_dir()
                        || (e.attrs.is_symlink()
                            && sftp.stat(&native_term_sftp::join(&folder, &e.name)).is_ok_and(|a| a.is_dir()))
                })
                .map(|e| (names.decode(&e.name), native_term_sftp::join(&folder, &e.name)))
                .collect();
            folders.sort_by_key(|(name, _)| name.to_lowercase());
            What::RemoteTree { path: folder, folders }
        });
    }

    /// Opens the local folders from the drive down to `path` in the tree.
    fn expand_local(&mut self, id: u64, path: &Path) {
        let mut chain: Vec<PathBuf> = path.ancestors().map(Path::to_path_buf).collect();
        chain.reverse();
        for folder in chain {
            self.open_local_folder(id, folder);
        }
    }

    fn open_local_folder(&mut self, id: u64, folder: PathBuf) {
        let Some(tab) = self.tab(id) else { return };
        if !tab.local_tree.open(&folder) {
            return;
        }
        self.spawn(id, move || {
            let mut folders: Vec<(String, PathBuf)> = std::fs::read_dir(&folder)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                        .collect()
                })
                .unwrap_or_default();
            folders.sort_by_key(|(name, _)| name.to_lowercase());
            What::LocalTree { path: folder, folders }
        });
    }

    /// A tree node's chevron: open (read if needed) or close.
    fn toggle_remote(&mut self, id: u64, folder: Vec<u8>) {
        let open = self.tab(id).is_some_and(|t| t.remote_tree.node(&folder).open);
        match open {
            true => {
                if let Some(t) = self.tab(id) {
                    t.remote_tree.node(&folder).open = false;
                }
            }
            false => self.open_remote_folder(id, folder),
        }
    }

    fn toggle_local(&mut self, id: u64, folder: PathBuf) {
        let open = self.tab(id).is_some_and(|t| t.local_tree.node(&folder).open);
        match open {
            true => {
                if let Some(t) = self.tab(id) {
                    t.local_tree.node(&folder).open = false;
                }
            }
            false => self.open_local_folder(id, folder),
        }
    }

    /// Items dragged from the server's list, as a download needs them.
    fn dragged_remote(&self, dragged: &Dragged) -> Vec<(Vec<u8>, Attrs)> {
        let r = &self.tabs[self.active].remote;
        r.rows
            .iter()
            .filter(|x| dragged.keys.contains(&x.entry.name))
            .map(|x| {
                let mut attrs = x.entry.attrs.clone();
                if x.dir && attrs.is_symlink() {
                    attrs.permissions = Some(0o040755);
                }
                (native_term_sftp::join(&r.path, &x.entry.name), attrs)
            })
            .collect()
    }

    /// Files dragged from the local list.
    fn dragged_local(&self, dragged: &Dragged) -> Vec<PathBuf> {
        let l = &self.tabs[self.active].local;
        l.rows.iter().filter(|r| dragged.keys.contains(&local_key(r))).map(|r| r.path.clone()).collect()
    }

    fn remote_selection(&self, id: u64) -> Vec<(Vec<u8>, Attrs)> {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return Vec::new() };
        let r = &tab.remote;
        r.rows
            .iter()
            .filter(|x| r.selected.contains(&x.entry.name))
            .map(|x| {
                let mut attrs = x.entry.attrs.clone();
                if x.dir && attrs.is_symlink() {
                    // a link to a folder is downloaded as that folder
                    attrs.permissions = Some(0o040755);
                }
                (native_term_sftp::join(&r.path, &x.entry.name), attrs)
            })
            .collect()
    }

    fn local_selection(&self, id: u64) -> Vec<PathBuf> {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return Vec::new() };
        let l = &tab.local;
        l.rows.iter().filter(|x| l.selected.contains(&local_key(x))).map(|x| x.path.clone()).collect()
    }

    fn add_job(&mut self, tab: u64, kind: Kind, title: String, work: Option<Work>) -> (u64, Arc<Progress>) {
        let id = self.next_id;
        self.next_id += 1;
        let host = self.tabs.iter().find(|t| t.id == tab).map(|t| t.spec.label.clone()).unwrap_or_default();
        let progress = Arc::new(Progress::default());
        self.jobs.push(Job {
            id,
            tab,
            host,
            kind,
            title,
            progress: Arc::clone(&progress),
            state: JobState::Running,
            started: Instant::now(),
            finished: None,
            work,
            plan: Arc::new(Mutex::new(None)),
        });
        (id, progress)
    }

    /// Starts a transfer, or goes on with a paused or failed one: from the
    /// first file not yet copied, and in that file from where its partial
    /// copy ends. It needs the session connected.
    fn run_job(&mut self, id: u64) {
        let Some(job) = self.jobs.iter().find(|j| j.id == id) else { return };
        let (tab, Some(work)) = (job.tab, job.work.clone()) else { return };
        let Some(sftp) = self.tabs.iter().find(|t| t.id == tab).and_then(|t| t.remote.sftp.clone()) else {
            self.log(tab, t!("files-job-not-connected"), true);
            return;
        };
        let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) else { return };
        let progress = Arc::clone(&job.progress);
        progress.cancel.store(false, Ordering::Relaxed);
        progress.pause.store(false, Ordering::Relaxed);
        job.state = JobState::Running;
        job.started = Instant::now();
        job.finished = None;
        let plan = Arc::clone(&job.plan);
        let slots = self
            .tabs
            .iter()
            .find(|t| t.id == tab)
            .map_or_else(|| Arc::new(native_term_sftp::transfer::Slots::new(1)), |t| Arc::clone(&t.remote.slots));
        self.spawn(tab, move || {
            let known = plan.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let items = match known {
                Some(items) => Ok(items),
                None => {
                    // planned again from scratch (a pause while planning)
                    progress.total.store(0, Ordering::Relaxed);
                    progress.next.store(0, Ordering::Relaxed);
                    plan_work(&sftp, &work, &progress)
                }
            };
            let result = items.and_then(|items| {
                *plan.lock().unwrap_or_else(|e| e.into_inner()) = Some(items.clone());
                if work.uploads() {
                    transfer::upload(&sftp, work.names(), &items, &progress, &slots)
                } else {
                    transfer::download(&sftp, work.names(), &items, &progress, &slots)
                }
            });
            What::JobDone { job: id, result }
        });
    }

    /// Synchronize: the local folder shown with the server's folder shown.
    fn open_sync(&mut self, id: u64) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return };
        let Some(local) = tab.local.path.clone() else { return };
        let (host, remote, text) = (tab.spec.label.clone(), tab.remote.path.clone(), tab.remote.path_text.clone());
        let scan = crate::files_sync::Scan::Failed(String::new());
        self.sync = Some(crate::files_sync::SyncDialog::new(id, host, local, remote, text, scan));
        self.compare(id);
    }

    /// Compares the dialog's folders again, in the background.
    fn compare(&mut self, id: u64) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return };
        let (Some(sftp), names) = (tab.remote.sftp.clone(), tab.remote.names) else { return };
        let Some(d) = self.sync.as_mut().filter(|d| d.tab == id) else { return };
        let (seen, stop) = (Arc::new(std::sync::atomic::AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
        d.scan = crate::files_sync::Scan::Running { seen: Arc::clone(&seen), stop: Arc::clone(&stop) };
        let (local, remote) = (d.local.clone(), d.remote.clone());
        self.spawn(id, move || {
            What::Compared(native_term_sftp::sync::compare(&sftp, &names, &local, &remote, &seen, &stop))
        });
    }

    /// Synchronize's Start: the copies as transfers (pausable like any),
    /// the deletions as deletes (local ones to the Recycle Bin).
    fn start_sync(&mut self, id: u64, plan: crate::files_sync::Plan) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return };
        if tab.remote.sftp.is_none() {
            self.log(id, t!("files-job-not-connected"), true);
            return;
        }
        let names = tab.remote.names;
        if !plan.uploads.is_empty() {
            let title = t!("files-job-sync-upload", count = plan.uploads.len());
            let work = Work::UploadPairs { names, pairs: plan.uploads };
            let (job, _) = self.add_job(id, Kind::Upload, title, Some(work));
            self.run_job(job);
        }
        if !plan.downloads.is_empty() {
            let title = t!("files-job-sync-download", count = plan.downloads.len());
            let work = Work::DownloadPairs { names, pairs: plan.downloads };
            let (job, _) = self.add_job(id, Kind::Download, title, Some(work));
            self.run_job(job);
        }
        if !plan.delete_remote.is_empty() {
            let what = describe(&plan.delete_remote.iter().map(|(p, _)| names.decode(last(p))).collect::<Vec<_>>());
            self.delete_remote(id, plan.delete_remote, what);
        }
        if !plan.delete_local.is_empty() {
            self.recycle_local(id, plan.delete_local);
        }
    }

    fn job_action(&mut self, id: u64, action: JobAction) {
        let Some(i) = self.jobs.iter().position(|j| j.id == id) else { return };
        let job = &mut self.jobs[i];
        match (action, &job.state) {
            (JobAction::Pause, JobState::Running) => job.progress.pause.store(true, Ordering::Relaxed),
            (JobAction::Resume, JobState::Paused | JobState::Failed(_)) => self.run_job(id),
            (JobAction::Cancel, JobState::Running) => job.progress.cancel.store(true, Ordering::Relaxed),
            (JobAction::Cancel, JobState::Paused | JobState::Failed(_)) => {
                // nothing runs: the partial file is removed here
                job.state = JobState::Cancelled;
                job.finished = Some(Instant::now());
                let (tab, upload) = (job.tab, job.kind == Kind::Upload);
                let (plan, progress) = (Arc::clone(&job.plan), Arc::clone(&job.progress));
                let line = t!("files-job-cancelled", what = job.title.as_str());
                let sftp = self.tabs.iter().find(|t| t.id == tab).and_then(|t| t.remote.sftp.clone());
                self.log(tab, line, false);
                // then the side it was on is shown again, without the file
                self.spawn(tab, move || {
                    if let Some(items) = plan.lock().unwrap_or_else(|e| e.into_inner()).as_deref() {
                        transfer::discard(sftp.as_deref(), items, &progress, upload);
                    }
                    if upload {
                        What::Refresh
                    } else {
                        What::LocalRefresh
                    }
                });
            }
            (JobAction::Remove, JobState::Done | JobState::Cancelled | JobState::Failed(_)) => {
                self.jobs.remove(i);
            }
            _ => {}
        }
    }

    /// Local files and folders to the server's folder shown.
    fn upload(&mut self, id: u64, files: Vec<PathBuf>) {
        self.upload_to(id, files, None);
    }

    /// Local files and folders to a server's folder (`None`: the one shown).
    fn upload_to(&mut self, id: u64, files: Vec<PathBuf>, into: Option<Vec<u8>>) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return };
        if tab.remote.sftp.is_none() || files.is_empty() {
            return;
        }
        let (names, into) = (tab.remote.names, into.unwrap_or_else(|| tab.remote.path.clone()));
        let title = describe(
            &files
                .iter()
                .map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                .collect::<Vec<_>>(),
        );
        let work = Work::Upload { names, files, into };
        let (job, _) = self.add_job(id, Kind::Upload, t!("files-job-upload", what = title), Some(work));
        self.run_job(job);
    }

    /// The server's files and folders to the local folder shown.
    fn download(&mut self, id: u64, items: Vec<(Vec<u8>, Attrs)>) {
        self.download_to(id, items, None);
    }

    /// The server's files and folders to a local folder (`None`: the one
    /// shown).
    fn download_to(&mut self, id: u64, items: Vec<(Vec<u8>, Attrs)>, into: Option<PathBuf>) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return };
        if tab.remote.sftp.is_none() {
            return;
        }
        let Some(folder) = into.or_else(|| tab.local.path.clone()) else {
            let text = t!("files-local-no-folder");
            self.log(id, text, true);
            return;
        };
        if items.is_empty() {
            return;
        }
        let names = tab.remote.names;
        let title = describe(&items.iter().map(|(p, _)| names.decode(last(p))).collect::<Vec<_>>());
        let work = Work::Download { names, items, folder };
        let (job, _) = self.add_job(id, Kind::Download, t!("files-job-download", what = title), Some(work));
        self.run_job(job);
    }

    fn delete_remote(&mut self, id: u64, items: Vec<(Vec<u8>, Attrs)>, what: String) {
        let Some(sftp) = self.tab(id).and_then(|t| t.remote.sftp.clone()) else { return };
        let (job, _) = self.add_job(id, Kind::Delete, t!("files-job-delete", what = what), None);
        self.spawn(id, move || {
            let result = items.iter().try_for_each(|(path, attrs)| transfer::remove(&sftp, path, attrs));
            What::JobDone { job, result }
        });
    }

    fn recycle_local(&mut self, id: u64, paths: Vec<PathBuf>) {
        self.spawn(id, move || match native_term_win::shell::recycle(&paths) {
            Ok(()) => What::LocalRefresh,
            Err(e) => What::Notice(e.to_string(), true),
        });
    }

    fn rename_remote(&mut self, id: u64, old: Vec<u8>, new: String) {
        let Some(tab) = self.tab(id) else { return };
        let Some(sftp) = tab.remote.sftp.clone() else { return };
        let Some(new_name) = tab.remote.names.encode(new.trim()).filter(|n| !n.is_empty() && !n.contains(&b'/')) else {
            let text = t!("files-bad-name", name = new.as_str());
            self.log(id, text, true);
            return;
        };
        let (from, to) =
            (native_term_sftp::join(&tab.remote.path, &old), native_term_sftp::join(&tab.remote.path, &new_name));
        self.spawn(id, move || match sftp.rename(&from, &to, false) {
            Ok(()) => What::Refresh,
            Err(e) => What::Notice(e.to_string(), true),
        });
    }

    fn rename_local(&mut self, id: u64, old: PathBuf, new: String) {
        let new = new.trim().to_string();
        if new.is_empty() || new.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
            self.log(id, t!("files-bad-name", name = new.as_str()), true);
            return;
        }
        let to = old.with_file_name(&new);
        self.spawn(id, move || match std::fs::rename(&old, &to) {
            Ok(()) => What::LocalRefresh,
            Err(e) => What::Notice(e.to_string(), true),
        });
    }

    fn mkdir(&mut self, id: u64, remote: bool, name: String) {
        let Some(tab) = self.tab(id) else { return };
        if remote {
            let Some(sftp) = tab.remote.sftp.clone() else { return };
            let Some(bytes) = tab.remote.names.encode(name.trim()).filter(|n| !n.is_empty() && !n.contains(&b'/'))
            else {
                let text = t!("files-bad-name", name = name.as_str());
                self.log(id, text, true);
                return;
            };
            let path = native_term_sftp::join(&tab.remote.path, &bytes);
            self.spawn(id, move || match sftp.mkdir(&path) {
                Ok(()) => What::Refresh,
                Err(e) => What::Notice(e.to_string(), true),
            });
        } else {
            let Some(folder) = tab.local.path.clone() else { return };
            let path = folder.join(name.trim());
            self.spawn(id, move || match std::fs::create_dir(&path) {
                Ok(()) => What::LocalRefresh,
                Err(e) => What::Notice(e.to_string(), true),
            });
        }
    }

    /// Downloads a file, opens it with its program, and uploads it again
    /// whenever it's saved.
    fn edit(&mut self, id: u64, row: &RemoteRow) {
        let edit_dir = self.edit_dir.clone();
        let ctx = self.ctx.clone();
        let Some(tab) = self.tab(id) else { return };
        let Some(sftp) = tab.remote.sftp.clone() else { return };
        let remote = native_term_sftp::join(&tab.remote.path, &row.entry.name);
        let folder = edit_dir
            .join(transfer::local_name(&Names::default(), tab.spec.alias.as_bytes()))
            .join(format!("{:x}", unique()));
        let local = folder.join(transfer::local_name(&tab.remote.names, &row.entry.name));
        let status = Arc::new(Mutex::new(t!("files-edit-opening")));
        let (stop, conflict, overwrite) =
            (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        tab.edits.push(Edit {
            name: row.name.clone(),
            local: local.clone(),
            status: Arc::clone(&status),
            stop: Arc::clone(&stop),
            conflict: Arc::clone(&conflict),
            overwrite: Arc::clone(&overwrite),
        });
        std::thread::spawn(move || {
            let set = |text: String| {
                *status.lock().unwrap_or_else(|e| e.into_inner()) = text;
                ctx.request_repaint();
            };
            let started = std::fs::create_dir_all(&folder)
                .map_err(native_term_sftp::Error::from)
                .and_then(|()| sftp.stat(&remote))
                .and_then(|attrs| sftp.download(&remote, &local, &mut |_| true).map(|_| attrs));
            let attrs = match started {
                Ok(a) => a,
                Err(e) => return set(t!("files-edit-failed", error = e.to_string())),
            };
            if let Err(e) = native_term_win::shell::open_file(&local) {
                return set(t!("files-edit-failed", error = e.to_string()));
            }
            set(t!("files-edit-watching"));
            watch_and_upload(&sftp, &remote, &local, attrs, &stop, &conflict, &overwrite, &set);
        });
    }

    fn running(&self, tab: Option<u64>) -> usize {
        self.jobs.iter().filter(|j| matches!(j.state, JobState::Running) && tab.is_none_or(|t| j.tab == t)).count()
    }

    fn edits(&self, tab: Option<u64>) -> usize {
        self.tabs.iter().filter(|t| tab.is_none_or(|id| t.id == id)).map(|t| t.edits.len()).sum()
    }

    fn close_tab(&mut self, id: u64) {
        // as by Pause (the partial files stay to be continued)
        for job in self.jobs.iter().filter(|j| j.tab == id) {
            job.progress.pause.store(true, Ordering::Relaxed);
        }
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            let tab = self.tabs.remove(i);
            end_tab(&tab);
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len().saturating_sub(1);
            }
        }
    }

    /// One row of session tabs; `remote`: the server's side (with close).
    fn tab_row(&mut self, ui: &mut egui::Ui, remote: bool) {
        let mut close = None;
        // both rows the same height, centered (the server's has close buttons)
        let row = egui::vec2(ui.available_width(), 28.0);
        ui.allocate_ui_with_layout(row, egui::Layout::left_to_right(egui::Align::Center), |ui| {
            for (i, tab) in self.tabs.iter().enumerate() {
                let text = if remote {
                    tab.spec.label.clone()
                } else {
                    t!("files-local-tab", computer = self.computer.as_str())
                };
                let text = if remote && tab.remote.failed.is_some() { format!("{text} ⚠") } else { text };
                if ui.selectable_label(i == self.active, text).on_hover_text(tab.spec.alias.as_str()).clicked() {
                    self.active = i;
                    self.remote_focus = remote;
                }
                if remote {
                    let button = ui.small_button(icon(icons::CLEAR)).on_hover_text(t!("files-close-tab"));
                    button.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t!("files-close-tab"))
                    });
                    if button.clicked() {
                        close = Some(tab.id);
                    }
                }
                ui.add_space(6.0);
            }
        });
        if let Some(id) = close {
            if self.running(Some(id)) + self.edits(Some(id)) > 0 {
                self.confirm = Some(Confirm::CloseTab(id));
            } else {
                self.close_tab(id);
            }
        }
    }

    fn local_side(&mut self, ui: &mut egui::Ui) {
        self.tab_row(ui, false);
        ui.separator();
        let Some(tab) = self.tabs.get(self.active) else { return };
        let id = tab.id;
        let (connected, selected, at_folder) =
            (tab.remote.sftp.is_some(), !tab.local.selected.is_empty(), tab.local.path.is_some());
        let mut path_text = tab.local.path_text.clone();
        let mut go_to = None;
        let mut up = false;
        let mut refresh = false;
        let mut upload = false;
        let mut delete = false;
        let mut new_folder = false;
        ui.horizontal(|ui| {
            let b = ui.add_enabled(at_folder, egui::Button::new(icon(icons::UP))).on_hover_text(t!("files-up"));
            b.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, at_folder, t!("files-up")));
            up = b.clicked();
            let b = ui.button(icon(icons::REFRESH)).on_hover_text(t!("files-refresh"));
            b.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t!("files-refresh")));
            refresh = b.clicked();
            let edit = ui.add(
                egui::TextEdit::singleline(&mut path_text).desired_width((ui.available_width() - 290.0).max(160.0)),
            );
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                go_to = Some(path_text.clone());
            }
            let b = egui::Button::new(format!("{} {}", t!("files-upload-to-remote"), icons::UPLOAD));
            upload = ui.add_enabled(connected && selected, b).on_hover_text(t!("files-upload-hint")).clicked();
            delete = ui
                .add_enabled(
                    selected && at_folder,
                    egui::Button::new(format!("{} {}", icons::DELETE, t!("files-delete"))),
                )
                .clicked();
            new_folder = ui
                .add_enabled(at_folder, egui::Button::new(format!("{} {}", icons::NEW_FOLDER, t!("files-new-folder"))))
                .clicked();
        });
        if let Some(tab) = self.tab(id) {
            tab.local.path_text = path_text;
        }
        if up {
            let parent = self.tabs[self.active].local.path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf));
            self.list_local(id, parent);
        }
        if refresh {
            self.refresh_local(id);
        }
        if let Some(text) = go_to {
            let path = PathBuf::from(text.trim());
            self.list_local(id, (!text.trim().is_empty()).then_some(path));
        }
        if upload {
            let files = self.local_selection(id);
            self.upload(id, files);
        }
        if delete {
            self.ask_recycle(id);
        }
        if new_folder {
            self.new_folder = Some((id, false, String::new()));
        }
        if let Some(e) = self.tabs.get(self.active).and_then(|t| t.local.error.clone()) {
            ui.colored_label(RED, e);
        }
        // the status line, under the tree and the list
        egui::Panel::bottom("local-status").show_inside(ui, |ui| {
            let local = &self.tabs[self.active].local;
            let chosen = local.rows.iter().filter(|r| local.selected.contains(&local_key(r)));
            let (count, bytes) = chosen.fold((0, 0), |(n, b), r| (n + 1, b + r.size.unwrap_or(0)));
            let folders = local.rows.iter().filter(|r| r.dir).count();
            status_line(ui, None, folders, local.rows.len() - folders, count, bytes);
        });
        // the tree
        let roots = self.local_roots.clone();
        let tree_out = egui::Panel::left("local-tree")
            .resizable(true)
            .default_size(180.0)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin { right: 6, ..Default::default() }))
            .show_inside(ui, |ui| {
                let tab = &self.tabs[self.active];
                tree(ui, "local-tree", &roots, &tab.local_tree, tab.local.path.as_ref(), false)
            })
            .inner;
        if let Some(folder) = tree_out.go {
            self.list_local(id, Some(folder));
        }
        if let Some(folder) = tree_out.toggle {
            self.toggle_local(id, folder);
        }
        if let Some((folder, dragged)) = tree_out.dropped.filter(|(_, d)| d.tab == id) {
            let items = self.dragged_remote(&dragged);
            self.download_to(id, items, Some(folder));
        }
        // the list
        let tab = &self.tabs[self.active];
        let lines: Vec<Line> = tab
            .local
            .rows
            .iter()
            .map(|r| Line {
                key: local_key(r),
                name: r.name.clone(),
                glyph: if r.dir { icons::FOLDER } else { icons::DOCUMENT },
                size: (!r.dir).then_some(r.size).flatten(),
                modified: r.modified,
                mode: None,
            })
            .collect();
        let mut local = std::mem::replace(&mut self.tabs[self.active].local, empty_local());
        let out = list(ui, "local-list", &lines, &mut local.selected, &mut local.anchor, &mut local.renaming, false);
        self.tabs[self.active].local = local;
        if out.clicked {
            self.remote_focus = false;
        }
        if let Some(i) = out.open {
            self.open_local(id, i);
        }
        if let Some((old, new)) = out.renamed {
            if let Some(row) = self.tabs[self.active].local.rows.iter().find(|r| local_key(r) == old) {
                let path = row.path.clone();
                self.rename_local(id, path, new);
            }
        }
        if out.drag {
            let keys = self.tabs[self.active].local.selected.iter().cloned().collect();
            out.response.dnd_set_drag_payload(Dragged { tab: id, from_remote: false, keys });
        }
        if let Some(dragged) = out.dropped.filter(|d| d.from_remote && d.tab == id) {
            let items = self.dragged_remote(&dragged);
            self.download(id, items);
        }
        match out.action {
            Some((i, Action::Open)) => self.open_local(id, i),
            Some((_, Action::Transfer)) => {
                let files = self.local_selection(id);
                self.upload(id, files);
            }
            Some((i, Action::Rename)) => {
                let row = self.tabs[self.active].local.rows[i].clone();
                self.tabs[self.active].local.renaming = Some((local_key(&row), row.name));
            }
            Some((_, Action::CopyPath)) => {
                let text: Vec<String> = self.local_selection(id).iter().map(|p| p.display().to_string()).collect();
                ui.ctx().copy_text(text.join("\n"));
            }
            Some((_, Action::Delete)) => self.ask_recycle(id),
            Some((_, Action::Edit)) | None => {}
        }
    }

    fn open_local(&mut self, id: u64, i: usize) {
        let Some(row) = self.tabs.get(self.active).and_then(|t| t.local.rows.get(i)).cloned() else { return };
        if row.dir {
            self.list_local(id, Some(row.path));
        } else if let Err(e) = native_term_win::shell::open_file(&row.path) {
            self.log(id, e.to_string(), true);
        }
    }

    fn ask_recycle(&mut self, id: u64) {
        let paths = self.local_selection(id);
        if paths.is_empty() {
            return;
        }
        let what = describe(
            &paths
                .iter()
                .map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                .collect::<Vec<_>>(),
        );
        self.confirm = Some(Confirm::RecycleLocal { tab: id, paths, what });
    }

    fn ask_delete_remote(&mut self, id: u64) {
        let items = self.remote_selection(id);
        if items.is_empty() {
            return;
        }
        let names = self.tabs.iter().find(|t| t.id == id).map(|t| t.remote.names).unwrap_or_default();
        let what = describe(&items.iter().map(|(p, _)| names.decode(last(p))).collect::<Vec<_>>());
        let folders = items.iter().any(|(_, a)| a.is_dir() && !a.is_symlink());
        self.confirm = Some(Confirm::DeleteRemote { tab: id, items, what, folders });
    }

    fn remote_side(&mut self, ui: &mut egui::Ui) {
        self.tab_row(ui, true);
        ui.separator();
        let Some(tab) = self.tabs.get(self.active) else {
            ui.weak(t!("files-no-tabs"));
            return;
        };
        let id = tab.id;
        // the status line, at the very bottom (in line with the local one)
        egui::Panel::bottom("remote-status").show_inside(ui, |ui| {
            let remote = &self.tabs[self.active].remote;
            let state = match (&remote.sftp, &remote.failed) {
                (_, Some(_)) => (RED, t!("files-status-failed")),
                (None, None) => (ui.visuals().weak_text_color(), t!("files-status-connecting")),
                (Some(_), None) => (GREEN, t!("files-status-connected")),
            };
            let chosen = remote.rows.iter().filter(|r| remote.selected.contains(&r.entry.name));
            let (count, bytes) =
                chosen.fold((0, 0), |(n, b), r| (n + 1, b + if r.dir { 0 } else { r.entry.attrs.size.unwrap_or(0) }));
            let folders = remote.rows.iter().filter(|r| r.dir).count();
            status_line(ui, Some(state), folders, remote.rows.len() - folders, count, bytes);
        });
        // the session's log, above the status line
        egui::Panel::bottom("files-log").resizable(true).default_size(110.0).show_inside(ui, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                for (time, text, error) in &self.tabs[self.active].log {
                    let line = format!("{time}  {text}");
                    if *error {
                        ui.colored_label(RED, line);
                    } else {
                        ui.weak(line);
                    }
                }
            });
        });
        let tab = &self.tabs[self.active];
        let connected = tab.remote.sftp.is_some();
        let selected = !tab.remote.selected.is_empty();
        let file = tab
            .remote
            .rows
            .iter()
            .find(|r| tab.remote.selected.len() == 1 && tab.remote.selected.contains(&r.entry.name) && !r.dir)
            .cloned();
        let at_root = tab.remote.path == b"/";
        let mut path_text = tab.remote.path_text.clone();
        let mut names = tab.remote.names;
        let (mut up, mut refresh, mut download, mut edit, mut delete, mut new_folder, mut go_to) =
            (false, false, false, false, false, false, None);
        let mut sync = false;
        let local_folder = tab.local.path.is_some();
        ui.horizontal(|ui| {
            let b =
                ui.add_enabled(connected && !at_root, egui::Button::new(icon(icons::UP))).on_hover_text(t!("files-up"));
            b.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, connected, t!("files-up")));
            up = b.clicked();
            let b =
                ui.add_enabled(connected, egui::Button::new(icon(icons::REFRESH))).on_hover_text(t!("files-refresh"));
            b.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, connected, t!("files-refresh")));
            refresh = b.clicked();
            let path = ui.add_enabled(
                connected,
                egui::TextEdit::singleline(&mut path_text).desired_width((ui.available_width() - 560.0).max(120.0)),
            );
            if path.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                go_to = Some(path_text.clone());
            }
            let b = egui::Button::new(format!("{} {}", icons::DOWNLOAD, t!("files-download-to-local")));
            download = ui.add_enabled(connected && selected, b).on_hover_text(t!("files-download-hint")).clicked();
            let b = egui::Button::new(format!("{} {}", icons::EDIT, t!("files-edit")));
            edit = ui.add_enabled(connected && file.is_some(), b).on_hover_text(t!("files-edit-hint")).clicked();
            delete = ui
                .add_enabled(
                    connected && selected,
                    egui::Button::new(format!("{} {}", icons::DELETE, t!("files-delete"))),
                )
                .clicked();
            new_folder = ui
                .add_enabled(connected, egui::Button::new(format!("{} {}", icons::NEW_FOLDER, t!("files-new-folder"))))
                .clicked();
            sync = ui
                .add_enabled(
                    connected && local_folder,
                    egui::Button::new(format!("{} {}", icons::SYNC, t!("files-sync"))),
                )
                .on_hover_text(t!("files-sync-hint"))
                .clicked();
            let shown =
                if matches!(names, Names::Auto { .. }) { t!("files-encoding-auto") } else { names.label().to_string() };
            egui::ComboBox::from_id_salt("files-encoding")
                .selected_text(shown)
                .width(100.0)
                .show_ui(ui, |ui| {
                    for label in ENCODINGS {
                        if let Some(choice) = Names::from_label(label) {
                            let text = if label == "auto" { t!("files-encoding-auto") } else { label.to_string() };
                            ui.selectable_value(&mut names, choice, text);
                        }
                    }
                })
                .response
                .on_hover_text(t!("files-encoding-hint"));
        });
        {
            let r = &mut self.tabs[self.active].remote;
            r.path_text = path_text;
            if names != r.names {
                r.names = names;
                for row in &mut r.rows {
                    row.name = names.decode(&row.entry.name);
                }
                r.path_text = names.decode(&r.path);
            }
        }
        if up {
            let parent = native_term_sftp::parent(&self.tabs[self.active].remote.path);
            self.go(id, parent);
        }
        if refresh {
            self.refresh(id);
        }
        if let Some(text) = go_to {
            match names.encode(text.trim()) {
                Some(p) if !p.is_empty() => self.go(id, p),
                _ => self.log(id, t!("files-bad-name", name = text.as_str()), true),
            }
        }
        if download {
            let items = self.remote_selection(id);
            self.download(id, items);
        }
        if let (true, Some(row)) = (edit, &file) {
            self.edit(id, row);
        }
        if delete {
            self.ask_delete_remote(id);
        }
        if new_folder {
            self.new_folder = Some((id, true, String::new()));
        }
        if sync {
            self.open_sync(id);
        }

        let tab = &self.tabs[self.active];
        match (&tab.remote.sftp, &tab.remote.failed) {
            (_, Some(error)) => {
                ui.colored_label(RED, error);
                ui.weak(t!("files-batch"));
                if ui.button(t!("files-reconnect")).clicked() {
                    self.connect(id);
                }
                return;
            }
            (None, None) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t!("files-connecting", host = tab.spec.alias.as_str()));
                });
                return;
            }
            (Some(_), None) => {}
        }
        if let Some(e) = &tab.remote.error {
            ui.colored_label(RED, e);
        }
        if tab.remote.listing {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak(t!("files-listing"));
            });
        }
        let roots = [("/".to_string(), b"/".to_vec())];
        let tree_out = egui::Panel::left("remote-tree")
            .resizable(true)
            .default_size(180.0)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin { right: 6, ..Default::default() }))
            .show_inside(ui, |ui| {
                let tab = &self.tabs[self.active];
                tree(ui, "remote-tree", &roots, &tab.remote_tree, Some(&tab.remote.path), true)
            })
            .inner;
        if let Some(folder) = tree_out.go {
            self.go(id, folder);
        }
        if let Some(folder) = tree_out.toggle {
            self.toggle_remote(id, folder);
        }
        if let Some((folder, dragged)) = tree_out.dropped.filter(|(_, d)| d.tab == id) {
            let files = self.dragged_local(&dragged);
            self.upload_to(id, files, Some(folder));
        }
        let tab = &self.tabs[self.active];
        let lines: Vec<Line> = tab
            .remote
            .rows
            .iter()
            .map(|r| Line {
                key: r.entry.name.clone(),
                name: r.name.clone(),
                glyph: if r.dir {
                    icons::FOLDER
                } else if r.entry.attrs.is_symlink() {
                    icons::LINK
                } else {
                    icons::DOCUMENT
                },
                size: if r.dir { None } else { r.entry.attrs.size },
                modified: r.entry.attrs.mtime().map(u64::from),
                mode: r.entry.attrs.permissions.map(mode_text),
            })
            .collect();
        let mut remote = std::mem::take(&mut self.tabs[self.active].remote.selected);
        let mut anchor = self.tabs[self.active].remote.anchor;
        let mut renaming = self.tabs[self.active].remote.renaming.take();
        let out = list(ui, "remote-list", &lines, &mut remote, &mut anchor, &mut renaming, true);
        {
            let r = &mut self.tabs[self.active].remote;
            r.selected = remote;
            r.anchor = anchor;
            r.renaming = renaming;
        }
        self.remote_rect = Some(out.response.rect);
        if out.clicked {
            self.remote_focus = true;
        }
        if let Some(i) = out.open {
            self.open_remote(id, i);
        }
        if let Some((old, new)) = out.renamed {
            self.rename_remote(id, old, new);
        }
        if out.drag {
            let keys = self.tabs[self.active].remote.selected.iter().cloned().collect();
            out.response.dnd_set_drag_payload(Dragged { tab: id, from_remote: true, keys });
        }
        if let Some(dragged) = out.dropped.filter(|d| !d.from_remote && d.tab == id) {
            let files = self.dragged_local(&dragged);
            self.upload(id, files);
        }
        match out.action {
            Some((i, Action::Open)) => self.open_remote(id, i),
            Some((_, Action::Transfer)) => {
                let items = self.remote_selection(id);
                self.download(id, items);
            }
            Some((i, Action::Edit)) => {
                let row = self.tabs[self.active].remote.rows[i].clone();
                self.edit(id, &row);
            }
            Some((i, Action::Rename)) => {
                let row = self.tabs[self.active].remote.rows[i].clone();
                self.tabs[self.active].remote.renaming = Some((row.entry.name.clone(), row.name));
            }
            Some((_, Action::CopyPath)) => {
                let tab = &self.tabs[self.active];
                let text: Vec<String> =
                    self.remote_selection(id).iter().map(|(p, _)| tab.remote.names.decode(p)).collect();
                ui.ctx().copy_text(text.join("\n"));
            }
            Some((_, Action::Delete)) => self.ask_delete_remote(id),
            None => {}
        }
    }

    fn open_remote(&mut self, id: u64, i: usize) {
        let Some(row) = self.tabs.get(self.active).and_then(|t| t.remote.rows.get(i)).cloned() else { return };
        if row.dir {
            let path = native_term_sftp::join(&self.tabs[self.active].remote.path, &row.entry.name);
            self.go(id, path);
        } else {
            self.edit(id, &row);
        }
    }

    /// The transfer queue (every session's) and edited files.
    fn activity(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong(t!("files-queue"));
            let running = self.running(None);
            let done = self.jobs.iter().filter(|j| matches!(j.state, JobState::Done)).count();
            let failed = self.jobs.iter().filter(|j| matches!(j.state, JobState::Failed(_))).count();
            let paused = self.jobs.iter().filter(|j| matches!(j.state, JobState::Paused)).count();
            if running > 0 {
                let speed: f64 = self
                    .jobs
                    .iter()
                    .filter(|j| matches!(j.state, JobState::Running) && j.kind != Kind::Delete)
                    .map(Job::speed)
                    .sum();
                ui.label(t!("files-queue-running", count = running));
                ui.weak(format!("{}/s", size_text(speed as u64)));
            }
            if paused > 0 {
                ui.label(t!("files-queue-paused", count = paused));
            }
            if done > 0 {
                ui.colored_label(GREEN, t!("files-queue-done", count = done));
            }
            if failed > 0 {
                ui.colored_label(RED, t!("files-queue-failed", count = failed));
            }
            let pausable = self.jobs.iter().any(|j| matches!(j.state, JobState::Running) && j.work.is_some());
            let label = format!("{} {}", icons::PAUSE, t!("files-queue-pause-all"));
            if ui.add_enabled(pausable, egui::Button::new(label).small()).clicked() {
                for job in self.jobs.iter().filter(|j| matches!(j.state, JobState::Running) && j.work.is_some()) {
                    job.progress.pause.store(true, Ordering::Relaxed);
                }
            }
            let label = format!("{} {}", icons::PLAY, t!("files-queue-resume-all"));
            if ui.add_enabled(paused > 0, egui::Button::new(label).small()).clicked() {
                let ids: Vec<u64> =
                    self.jobs.iter().filter(|j| matches!(j.state, JobState::Paused)).map(|j| j.id).collect();
                for id in ids {
                    self.run_job(id);
                }
            }
            // finished: done, cancelled or failed (paused ones stay)
            let finished = self.jobs.iter().any(|j| !matches!(j.state, JobState::Running | JobState::Paused));
            if ui.add_enabled(finished, egui::Button::new(t!("files-queue-clear")).small()).clicked() {
                self.jobs.retain(|j| matches!(j.state, JobState::Running | JobState::Paused));
            }
            // how many files each connection copies at once; the rest wait
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut at_once = self.at_once();
                let widget = egui::DragValue::new(&mut at_once).range(1..=AT_ONCE_MAX).speed(0.05);
                let response = ui.add(widget).on_hover_text(t!("files-at-once-hint"));
                ui.label(t!("files-at-once"));
                if response.changed() {
                    self.set_at_once(at_once);
                }
            });
        });
        let mut actions = Vec::new();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if self.jobs.is_empty() && self.edits(None) == 0 {
                ui.weak(t!("files-queue-empty"));
            }
            for job in &self.jobs {
                ui.horizontal(|ui| {
                    let done = job.progress.done.load(Ordering::Relaxed);
                    let total = job.progress.total.load(Ordering::Relaxed);
                    let secs =
                        job.finished.unwrap_or_else(Instant::now).duration_since(job.started).as_secs_f64().max(0.001);
                    let rate = job.speed();
                    let speed = format!("{}/s", size_text(rate as u64));
                    let fraction = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                    let figures = format!("{}%  {} / {}", (fraction * 100.0) as u32, size_text(done), size_text(total));
                    ui.weak(format!("[{}]", job.host));
                    // the buttons are on the right; the bar takes what's left
                    let buttons = 150.0;
                    match &job.state {
                        JobState::Running => {
                            let text = if job.kind == Kind::Delete {
                                job.title.clone()
                            } else {
                                // the time left, once something has gone (a rate to go by)
                                let left = (rate > 0.0 && total > done).then(|| {
                                    let secs = ((total - done) as f64 / rate).ceil() as u64;
                                    format!("  {}", t!("files-job-left", time = clock_text(secs)))
                                });
                                format!("{}  {figures}  {speed}{}", job.title, left.unwrap_or_default())
                            };
                            let width = ui.available_width() - buttons;
                            ui.add(egui::ProgressBar::new(fraction).desired_width(width).text(text));
                            if job.work.is_some()
                                && ui.small_button(format!("{} {}", icons::PAUSE, t!("files-job-pause"))).clicked()
                            {
                                actions.push((job.id, JobAction::Pause));
                            }
                            if ui.small_button(t!("button-cancel")).clicked() {
                                actions.push((job.id, JobAction::Cancel));
                            }
                        }
                        JobState::Paused => {
                            let text = format!("{}  {figures}  {}", job.title, t!("files-job-paused-short"));
                            let width = ui.available_width() - buttons;
                            ui.add(
                                egui::ProgressBar::new(fraction)
                                    .desired_width(width)
                                    .text(text)
                                    .fill(ui.visuals().widgets.inactive.bg_fill),
                            );
                            if ui.small_button(format!("{} {}", icons::PLAY, t!("files-job-resume"))).clicked() {
                                actions.push((job.id, JobAction::Resume));
                            }
                            if ui.small_button(t!("button-cancel")).clicked() {
                                actions.push((job.id, JobAction::Cancel));
                            }
                        }
                        JobState::Done => {
                            let size = match (job.kind, secs >= 1.0) {
                                (Kind::Delete, _) => String::new(),
                                (_, true) => format!("  {}  {speed}", size_text(done)),
                                (_, false) => format!("  {}", size_text(done)),
                            };
                            ui.colored_label(GREEN, format!("{} {}{size}", icons::ACCEPT, job.title));
                        }
                        JobState::Failed(e) => {
                            ui.colored_label(RED, format!("{}: {e}", job.title));
                            if job.work.is_some() {
                                let b = ui
                                    .small_button(format!("{} {}", icons::PLAY, t!("files-job-resume")))
                                    .on_hover_text(t!("files-job-resume-hint"));
                                if b.clicked() {
                                    actions.push((job.id, JobAction::Resume));
                                }
                            }
                        }
                        JobState::Cancelled => {
                            ui.weak(t!("files-job-cancelled", what = job.title.as_str()));
                        }
                    }
                    if matches!(job.state, JobState::Done | JobState::Cancelled | JobState::Failed(_)) {
                        let b = ui.small_button(icon(icons::CLEAR)).on_hover_text(t!("files-job-remove"));
                        b.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t!("files-job-remove"))
                        });
                        if b.clicked() {
                            actions.push((job.id, JobAction::Remove));
                        }
                    }
                });
            }
            for tab in &mut self.tabs {
                let mut stop = None;
                for (i, edit) in tab.edits.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.weak(format!("[{}]", tab.spec.label));
                        ui.label(format!("{} {}", icons::EDIT, edit.name));
                        ui.weak(edit.status.lock().unwrap_or_else(|e| e.into_inner()).as_str());
                        if edit.conflict.load(Ordering::Relaxed)
                            && ui.small_button(egui::RichText::new(t!("files-edit-overwrite")).color(RED)).clicked()
                        {
                            edit.overwrite.store(true, Ordering::Relaxed);
                        }
                        if ui
                            .small_button(t!("files-edit-stop"))
                            .on_hover_text(edit.local.display().to_string())
                            .clicked()
                        {
                            stop = Some(i);
                        }
                    });
                }
                if let Some(i) = stop {
                    let edit = tab.edits.remove(i);
                    edit.stop.store(true, Ordering::Relaxed);
                }
            }
        });
        for (id, action) in actions {
            self.job_action(id, action);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(d) = &mut self.sync {
            use crate::files_sync::Asked;
            let tab = d.tab;
            match d.show(ctx) {
                Some(Asked::Rescan) => self.compare(tab),
                Some(Asked::Start(plan)) => {
                    self.sync = None;
                    self.start_sync(tab, plan);
                }
                Some(Asked::Close) => self.sync = None,
                None => {}
            }
        }
        if let Some((tab, question, typed)) = &mut self.question {
            let host = self.tabs.iter().find(|t| t.id == *tab).map(|t| t.spec.alias.clone()).unwrap_or_default();
            let mut done = None;
            modal(ctx, "files-question", t!("files-question-title", host = host.as_str()), |ui| {
                ui.label(question.prompt.trim());
                let edit = ui.add(egui::TextEdit::singleline(typed).password(question.secret).desired_width(320.0));
                edit.request_focus();
                // no IME in a password field (it would compose the typing)
                if question.secret {
                    crate::dialogs::no_ime(&edit);
                }
                // the field keeps the focus (it gives it up on Enter and takes
                // it back in the same frame): Enter anywhere submits
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button(t!("button-ok")).clicked() || enter {
                        done = Some(true);
                    }
                    if ui.button(t!("button-cancel")).clicked() {
                        done = Some(false);
                    }
                });
            });
            if let Some(ok) = done {
                // the field's focus goes with the dialog (else the list's keys stay dead)
                ctx.memory_mut(|m| m.stop_text_input());
                if let Some((_, question, typed)) = self.question.take() {
                    let _ = question.reply.send(ok.then_some(typed));
                }
            }
        }
        if let Some((tab, remote, name)) = &mut self.new_folder {
            let (tab, remote) = (*tab, *remote);
            let mut done = None;
            modal(ctx, "files-new-folder", t!("files-new-folder"), |ui| {
                let edit = ui.add(egui::TextEdit::singleline(name).hint_text(t!("files-new-folder-hint")));
                edit.request_focus();
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button(t!("button-create")).clicked() || enter {
                        done = Some(true);
                    }
                    if ui.button(t!("button-cancel")).clicked() {
                        done = Some(false);
                    }
                });
            });
            if let Some(ok) = done {
                ctx.memory_mut(|m| m.stop_text_input());
                let name = self.new_folder.take().map(|(_, _, n)| n).unwrap_or_default();
                if ok && !name.trim().is_empty() {
                    self.mkdir(tab, remote, name);
                }
            }
        }
        let Some(confirm) = &self.confirm else { return };
        let (title, text, warning, button) = match confirm {
            Confirm::DeleteRemote { what, folders, .. } => (
                t!("files-delete"),
                t!("files-delete-confirm", what = what.as_str()),
                folders.then(|| t!("files-delete-folders")),
                t!("files-delete-now"),
            ),
            Confirm::RecycleLocal { what, .. } => {
                (t!("files-delete"), t!("files-recycle-confirm", what = what.as_str()), None, t!("files-recycle-now"))
            }
            Confirm::CloseTab(id) => (
                t!("files-close-title"),
                t!("files-close-running", count = self.running(Some(*id)))
                    + "\n"
                    + &t!("files-close-edits", count = self.edits(Some(*id))),
                None,
                t!("files-close-now"),
            ),
            Confirm::CloseWindow => (
                t!("files-close-title"),
                t!("files-close-running", count = self.running(None))
                    + "\n"
                    + &t!("files-close-edits", count = self.edits(None)),
                None,
                t!("files-close-now"),
            ),
        };
        let mut done = None;
        modal(ctx, "files-confirm", title, |ui| {
            ui.label(text);
            if let Some(w) = warning {
                ui.colored_label(RED, w);
            }
            ui.horizontal(|ui| {
                if ui.button(egui::RichText::new(button).color(RED)).clicked() {
                    done = Some(true);
                }
                if ui.button(t!("button-cancel")).clicked() {
                    done = Some(false);
                }
            });
        });
        match (done, self.confirm.take()) {
            (Some(true), Some(Confirm::DeleteRemote { tab, items, what, .. })) => self.delete_remote(tab, items, what),
            (Some(true), Some(Confirm::RecycleLocal { tab, paths, .. })) => self.recycle_local(tab, paths),
            (Some(true), Some(Confirm::CloseTab(id))) => self.close_tab(id),
            (Some(true), Some(Confirm::CloseWindow)) => self.close = true,
            (Some(_), _) => {}
            (None, pending) => self.confirm = pending,
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.tabs.is_empty() || self.question.is_some() || self.confirm.is_some()
        {
            return;
        }
        let (f5, back, delete, f2, enter) = ctx.input(|i| {
            let k = |key| i.key_pressed(key);
            (k(egui::Key::F5), k(egui::Key::Backspace), k(egui::Key::Delete), k(egui::Key::F2), k(egui::Key::Enter))
        });
        let id = self.tabs[self.active].id;
        if self.remote_focus {
            let tab = &self.tabs[self.active];
            if tab.remote.sftp.is_none() {
                return;
            }
            let single = tab
                .remote
                .rows
                .iter()
                .position(|r| tab.remote.selected.len() == 1 && tab.remote.selected.contains(&r.entry.name));
            let parent = (tab.remote.path != b"/").then(|| native_term_sftp::parent(&tab.remote.path));
            if f5 {
                self.refresh(id);
            }
            if let (true, Some(parent)) = (back, parent) {
                self.go(id, parent);
            }
            if delete {
                self.ask_delete_remote(id);
            }
            if let (true, Some(i)) = (f2, single) {
                let row = self.tabs[self.active].remote.rows[i].clone();
                self.tabs[self.active].remote.renaming = Some((row.entry.name.clone(), row.name));
            }
            if let (true, Some(i)) = (enter, single) {
                self.open_remote(id, i);
            }
        } else {
            let tab = &self.tabs[self.active];
            let single = tab
                .local
                .rows
                .iter()
                .position(|r| tab.local.selected.len() == 1 && tab.local.selected.contains(&local_key(r)));
            let path = tab.local.path.clone();
            if f5 {
                self.refresh_local(id);
            }
            if let (true, Some(p)) = (back, &path) {
                self.list_local(id, p.parent().map(Path::to_path_buf));
            }
            if delete && path.is_some() {
                self.ask_recycle(id);
            }
            if let (true, Some(i)) = (f2, single) {
                let row = self.tabs[self.active].local.rows[i].clone();
                self.tabs[self.active].local.renaming = Some((local_key(&row), row.name));
            }
            if let (true, Some(i)) = (enter, single) {
                self.open_local(id, i);
            }
        }
    }
}

/// What a tree reports back.
struct TreeOut<P> {
    /// A folder clicked: show it.
    go: Option<P>,
    /// A chevron clicked.
    toggle: Option<P>,
    /// Files from the other side dropped on a folder.
    dropped: Option<(P, Arc<Dragged>)>,
    /// Where the tree keeps which folder it last scrolled to.
    scrolled: egui::Id,
}

/// A folder tree: chevrons open and close, a click shows the folder, the
/// one shown is highlighted, and files dragged from the other side can be
/// dropped on a folder. `remote`: the server's side (it takes files from
/// the local side, and the other way round).
fn tree<P: Clone + Eq + Hash>(
    ui: &mut egui::Ui,
    salt: &str,
    roots: &[(String, P)],
    tree: &Tree<P>,
    current: Option<&P>,
    remote: bool,
) -> TreeOut<P> {
    let mut out = TreeOut { go: None, toggle: None, dropped: None, scrolled: egui::Id::new((salt, "scrolled")) };
    egui::ScrollArea::both().id_salt(salt).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (name, path) in roots {
            tree_node(ui, name, path, 0, tree, current, remote, &mut out);
        }
    });
    out
}

#[allow(clippy::too_many_arguments)]
fn tree_node<P: Clone + Eq + Hash>(
    ui: &mut egui::Ui,
    name: &str,
    path: &P,
    depth: usize,
    tree: &Tree<P>,
    current: Option<&P>,
    remote: bool,
    out: &mut TreeOut<P>,
) {
    let node = tree.nodes.get(path);
    let open = node.is_some_and(|n| n.open);
    let leaf = node.and_then(|n| n.children.as_ref()).is_some_and(|c| c.is_empty());
    let visuals = ui.visuals().clone();
    let font = egui::TextStyle::Body.resolve(ui.style());
    let color = if current == Some(path) { visuals.selection.stroke.color } else { visuals.text_color() };
    let galley = ui.painter().layout_no_wrap(name.to_string(), font.clone(), color);
    // as wide as the panel, or as the name where it's indented (the tree
    // scrolls sideways to deep folders)
    let indent = 4.0 + depth as f32 * 14.0;
    let width = ui.available_width().max(160.0).max(indent + 40.0 + galley.size().x + 8.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 20.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, current == Some(path), name)
    });
    let target = response.dnd_hover_payload::<Dragged>().is_some_and(|d| d.from_remote != remote);
    if current == Some(path) {
        ui.painter().rect_filled(rect, 3.0, visuals.selection.bg_fill);
        // brought into view once each time another folder is shown (and
        // once it is drawn at all: the folders above open first)
        let this = egui::Id::new(path);
        let fresh = ui.data_mut(|d| {
            let fresh = d.get_temp::<egui::Id>(out.scrolled) != Some(this);
            d.insert_temp(out.scrolled, this);
            fresh
        });
        if fresh {
            let text = egui::Rect::from_min_size(
                egui::pos2(rect.left() + indent, rect.top()),
                egui::vec2(40.0 + galley.size().x + 8.0, rect.height()),
            );
            ui.scroll_to_rect(text, None);
        }
    } else if target || response.hovered() {
        ui.painter().rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
    }
    if target {
        ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(1.5_f32, GREEN), egui::StrokeKind::Inside);
    }
    let x = rect.left() + indent;
    let y = rect.center().y;
    let painter = ui.painter_at(rect);
    let chevron = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(16.0, rect.height()));
    if !leaf {
        let glyph = if open { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT };
        let small = egui::FontId::new(font.size * 0.7, font.family.clone());
        painter.text(chevron.center(), egui::Align2::CENTER_CENTER, glyph, small, visuals.weak_text_color());
    }
    let folder = if open { icons::FOLDER_OPEN } else { icons::FOLDER };
    painter.text(egui::pos2(x + 18.0, y), egui::Align2::LEFT_CENTER, folder, font.clone(), color);
    painter.galley(egui::pos2(x + 40.0, y - galley.size().y / 2.0), galley, color);
    if response.clicked() {
        let on_chevron = response.interact_pointer_pos().is_some_and(|p| chevron.contains(p));
        if on_chevron && !leaf {
            out.toggle = Some(path.clone());
        } else {
            out.go = Some(path.clone());
        }
    }
    if response.double_clicked() && !leaf {
        out.toggle = Some(path.clone());
    }
    if let Some(dragged) = response.dnd_release_payload::<Dragged>().filter(|d| d.from_remote != remote) {
        out.dropped = Some((path.clone(), dragged));
    }
    if open {
        match node.and_then(|n| n.children.as_ref()) {
            Some(children) => {
                for (child_name, child) in children {
                    tree_node(ui, child_name, child, depth + 1, tree, current, remote, out);
                }
            }
            None => {
                ui.horizontal(|ui| {
                    ui.add_space(x + 18.0 - rect.left());
                    ui.spinner();
                });
            }
        }
    }
}

/// What a list reports back.
struct Listed {
    response: egui::Response,
    clicked: bool,
    open: Option<usize>,
    action: Option<(usize, Action)>,
    renamed: Option<(Vec<u8>, String)>,
    drag: bool,
    dropped: Option<Arc<Dragged>>,
}

/// A file list: selection (Ctrl, Shift), double-click, right-click menu,
/// inline rename, dragging out and dropping in.
fn list(
    ui: &mut egui::Ui,
    salt: &str,
    lines: &[Line],
    selected: &mut HashSet<Vec<u8>>,
    anchor: &mut Option<usize>,
    renaming: &mut Option<(Vec<u8>, String)>,
    remote: bool,
) -> Listed {
    let visuals = ui.visuals().clone();
    // a narrow list keeps the name readable: permissions go first, then
    // the date, then the size
    let width = ui.available_width();
    let mut columns = [80.0, 130.0, if remote { 104.0 } else { 0.0 }];
    for i in [2, 1, 0] {
        if width - columns.iter().sum::<f32>() < 180.0 {
            columns[i] = 0.0;
        }
    }
    let [size_w, date_w, mode_w] = columns;
    let area = ui.available_rect_before_wrap();
    let response = ui.interact(area, ui.id().with((salt, "drop")), egui::Sense::hover());
    let dropped = response.dnd_release_payload::<Dragged>();
    let hovered_drop = response.dnd_hover_payload::<Dragged>().is_some_and(|d| d.from_remote != remote);
    if hovered_drop {
        ui.painter().rect_stroke(area, 4.0, egui::Stroke::new(2.0_f32, GREEN), egui::StrokeKind::Inside);
    }
    let mut out = Listed { response, clicked: false, open: None, action: None, renamed: None, drag: false, dropped };
    // header, placed like the rows' columns
    {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
        let (font, color, y) = (egui::TextStyle::Body.resolve(ui.style()), visuals.weak_text_color(), rect.center().y);
        let name_right = rect.right() - size_w - date_w - mode_w;
        let painter = ui.painter_at(rect);
        painter.text(
            egui::pos2(rect.left() + 28.0, y),
            egui::Align2::LEFT_CENTER,
            t!("files-col-name"),
            font.clone(),
            color,
        );
        if size_w > 0.0 {
            painter.text(
                egui::pos2(name_right + size_w - 8.0, y),
                egui::Align2::RIGHT_CENTER,
                t!("files-col-size"),
                font.clone(),
                color,
            );
        }
        if date_w > 0.0 {
            painter.text(
                egui::pos2(name_right + size_w + 8.0, y),
                egui::Align2::LEFT_CENTER,
                t!("files-col-modified"),
                font.clone(),
                color,
            );
        }
        if mode_w > 0.0 {
            painter.text(
                egui::pos2(name_right + size_w + date_w + 4.0, y),
                egui::Align2::LEFT_CENTER,
                t!("files-col-mode"),
                font,
                color,
            );
        }
    }
    ui.separator();
    let modifiers = ui.input(|i| i.modifiers);
    egui::ScrollArea::vertical().id_salt(salt).auto_shrink([false, false]).show_rows(
        ui,
        ROW,
        lines.len(),
        |ui, range| {
            for i in range {
                let line = &lines[i];
                let is_selected = selected.contains(&line.key);
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::click_and_drag());
                // for screen readers and UI automation: a selectable item named after the file
                response.widget_info(|| {
                    egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, is_selected, &line.name)
                });
                if is_selected {
                    ui.painter().rect_filled(rect, 3.0, visuals.selection.bg_fill);
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
                }
                let color = if is_selected { visuals.selection.stroke.color } else { visuals.text_color() };
                let font = egui::TextStyle::Body.resolve(ui.style());
                let y = rect.center().y;
                let painter = ui.painter_at(rect);
                painter.text(
                    egui::pos2(rect.left() + 6.0, y),
                    egui::Align2::LEFT_CENTER,
                    line.glyph,
                    font.clone(),
                    color,
                );
                let name_right = rect.right() - size_w - date_w - mode_w;
                let name_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 28.0, rect.top()),
                    egui::pos2(name_right - 8.0, rect.bottom()),
                );
                if renaming.as_ref().is_some_and(|(k, _)| *k == line.key) {
                    let (_, text) = renaming.as_mut().expect("renaming");
                    let edit = ui.put(name_rect, egui::TextEdit::singleline(text));
                    // the focus once, when renaming starts (taken every frame,
                    // the field would never give it up)
                    let started = egui::Id::new((salt, "renaming"));
                    let fresh = ui.data_mut(|d| {
                        let fresh = d.get_temp::<Vec<u8>>(started).as_ref() != Some(&line.key);
                        d.insert_temp(started, line.key.clone());
                        fresh
                    });
                    if fresh {
                        edit.request_focus();
                    } else if edit.lost_focus() {
                        // Enter or a click elsewhere renames; Esc doesn't
                        let (key, text) = renaming.take().expect("renaming");
                        ui.data_mut(|d| d.remove::<Vec<u8>>(started));
                        let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
                        if !cancelled && !text.trim().is_empty() && text != line.name {
                            out.renamed = Some((key, text));
                        }
                    }
                } else {
                    ui.painter_at(name_rect).text(
                        egui::pos2(name_rect.left(), y),
                        egui::Align2::LEFT_CENTER,
                        &line.name,
                        font.clone(),
                        color,
                    );
                }
                if let (Some(size), true) = (line.size, size_w > 0.0) {
                    painter.text(
                        egui::pos2(name_right + size_w - 8.0, y),
                        egui::Align2::RIGHT_CENTER,
                        size_text(size),
                        font.clone(),
                        color,
                    );
                }
                if let (Some(m), true) = (line.modified, date_w > 0.0) {
                    painter.text(
                        egui::pos2(name_right + size_w + 8.0, y),
                        egui::Align2::LEFT_CENTER,
                        native_term_win::local_date_time(m),
                        font.clone(),
                        color,
                    );
                }
                if let (Some(mode), true) = (&line.mode, mode_w > 0.0) {
                    let mono = egui::TextStyle::Monospace.resolve(ui.style());
                    painter.text(
                        egui::pos2(name_right + size_w + date_w + 4.0, y),
                        egui::Align2::LEFT_CENTER,
                        mode,
                        mono,
                        color,
                    );
                }

                if response.clicked() {
                    out.clicked = true;
                    click(lines, i, modifiers, selected, anchor);
                }
                if response.double_clicked() {
                    out.open = Some(i);
                }
                if response.drag_started() {
                    if !is_selected {
                        *selected = std::iter::once(line.key.clone()).collect();
                        *anchor = Some(i);
                    }
                    out.drag = true;
                    out.response = response.clone();
                }
                if response.secondary_clicked() {
                    out.clicked = true;
                    if !is_selected {
                        *selected = std::iter::once(line.key.clone()).collect();
                        *anchor = Some(i);
                    }
                }
                response.context_menu(|ui| {
                    let many = selected.len() > 1;
                    let mut item = |ui: &mut egui::Ui, text: String, action: Action| {
                        if ui.button(text).clicked() {
                            out.action = Some((i, action));
                            ui.close();
                        }
                    };
                    item(ui, format!("{} {}", icons::OPEN, t!("files-open")), Action::Open);
                    if remote {
                        item(ui, format!("{} {}", icons::DOWNLOAD, t!("files-download-to-local")), Action::Transfer);
                        if !many && line.glyph != icons::FOLDER {
                            item(ui, format!("{} {}", icons::EDIT, t!("files-edit")), Action::Edit);
                        }
                    } else {
                        item(ui, format!("{} {}", t!("files-upload-to-remote"), icons::UPLOAD), Action::Transfer);
                    }
                    ui.separator();
                    if !many {
                        item(ui, format!("{} {}", icons::RENAME, t!("files-rename")), Action::Rename);
                    }
                    item(ui, t!("files-copy-path"), Action::CopyPath);
                    ui.separator();
                    if ui
                        .button(egui::RichText::new(format!("{} {}", icons::DELETE, t!("files-delete"))).color(RED))
                        .clicked()
                    {
                        out.action = Some((i, Action::Delete));
                        ui.close();
                    }
                });
            }
        },
    );
    out
}

fn click(
    lines: &[Line],
    i: usize,
    modifiers: egui::Modifiers,
    selected: &mut HashSet<Vec<u8>>,
    anchor: &mut Option<usize>,
) {
    let key = lines[i].key.clone();
    if modifiers.shift {
        let from = anchor.unwrap_or(i);
        let (a, b) = (from.min(i), from.max(i));
        if !modifiers.command {
            selected.clear();
        }
        selected.extend(lines[a..=b].iter().map(|l| l.key.clone()));
    } else if modifiers.command {
        if !selected.remove(&key) {
            selected.insert(key);
        }
        *anchor = Some(i);
    } else {
        *selected = std::iter::once(key).collect();
        *anchor = Some(i);
    }
}

impl crate::window::Ui for FilesWindow {
    fn ui(&mut self, ui: &mut egui::Ui) {
        if let Some(theme) = *crate::app::THEME.lock().unwrap_or_else(|e| e.into_inner()) {
            if ui.ctx().options(|o| o.theme_preference) != theme {
                ui.ctx().set_theme(theme);
            }
        }
        self.take_pending();
        while let Ok(event) = self.rx.try_recv() {
            self.handle(event);
        }
        let ctx = ui.ctx().clone();
        // after the events: a tab that has just connected is ready here
        self.take_pending_uploads(&ctx);
        // files dropped from Explorer onto the server's side: uploaded there
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            let over_remote =
                ctx.input(|i| i.pointer.latest_pos()).zip(self.remote_rect).is_none_or(|(p, r)| r.contains(p));
            match (self.tabs.get(self.active).map(|t| t.id), over_remote) {
                (Some(id), true) => self.upload(id, dropped),
                (Some(id), false) => self.log(id, t!("files-drop-remote-only"), true),
                (None, _) => {}
            }
        }
        self.keys(&ctx);
        // the queue is always there (empty, it says how to start a transfer)
        egui::Panel::bottom("files-activity")
            .resizable(true)
            .default_size(150.0)
            .min_size(64.0)
            .max_size(360.0)
            .show_inside(ui, |ui| self.activity(ui));
        let half = ui.available_width() / 2.0;
        // the same margins on both sides, so their rows line up
        let frame = egui::Frame::NONE.inner_margin(8.0_f32).fill(ui.visuals().panel_fill);
        egui::Panel::left("files-local")
            .resizable(true)
            .default_size(half)
            .frame(frame)
            .show_inside(ui, |ui| self.local_side(ui));
        egui::CentralPanel::default().frame(frame).show_inside(ui, |ui| self.remote_side(ui));
        self.dialogs(&ctx);
        // progress bars and edit states move by themselves
        if self.running(None) > 0 {
            ctx.request_repaint_after(Duration::from_millis(250));
        } else if self.edits(None) > 0 {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn close_requested(&mut self) -> bool {
        if self.running(None) + self.edits(None) == 0 {
            return true;
        }
        self.confirm = Some(Confirm::CloseWindow);
        false
    }

    fn wants_close(&self) -> bool {
        self.close
    }
}

impl Drop for FilesWindow {
    fn drop(&mut self) {
        WINDOW.with(|w| *w.borrow_mut() = None);
        if let Some((_, question, _)) = self.question.take() {
            let _ = question.reply.send(None);
        }
        // stopped as by Pause: a new transfer of the same files goes on
        // from their partial copies
        for job in &self.jobs {
            job.progress.pause.store(true, Ordering::Relaxed);
        }
        for tab in &self.tabs {
            end_tab(tab);
        }
    }
}

/// A side's status line: the connection (the server's side), what the
/// folder holds, and what is selected.
fn status_line(
    ui: &mut egui::Ui,
    state: Option<(egui::Color32, String)>,
    folders: usize,
    files: usize,
    selected: usize,
    bytes: u64,
) {
    ui.horizontal(|ui| {
        if let Some((color, text)) = state {
            // a dot drawn (the font may have no glyph for one)
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 4.0, color);
            ui.colored_label(color, text);
            ui.separator();
        }
        ui.weak(t!("files-status-items", folders = folders, files = files));
        if selected > 0 {
            ui.separator();
            ui.label(t!("files-status-selected", count = selected, size = size_text(bytes)));
        }
    });
}

/// Every file and folder a transfer involves.
fn plan_work(sftp: &Session, work: &Work, progress: &Progress) -> native_term_sftp::Result<Vec<Item>> {
    match work {
        Work::Upload { names, files, into } => transfer::plan_upload(names, files, into, progress),
        Work::UploadPairs { names, pairs } => transfer::plan_upload_pairs(names, pairs, progress),
        Work::DownloadPairs { names, pairs } => transfer::plan_download_pairs(sftp, names, pairs, progress),
        Work::Download { names, items, folder } => {
            let mut plan = Vec::new();
            for (path, attrs) in items {
                plan.extend(transfer::plan_download(sftp, names, path, attrs, folder, progress)?);
            }
            Ok(plan)
        }
    }
}

fn remote_key_for(alias: &str) -> String {
    format!("files.remote:{alias}")
}

fn local_key_for(alias: &str) -> String {
    format!("files.local:{alias}")
}

/// A value kept in `state.db`'s settings.
fn recall(memory: Option<&native_term_app::Core>, key: &str) -> Option<String> {
    memory?.registry()?.setting(key).ok().flatten()
}

/// A server's path (bytes, maybe not UTF-8) as text for the settings.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

fn end_tab(tab: &Tab) {
    for edit in &tab.edits {
        edit.stop.store(true, Ordering::Relaxed);
    }
    if let Some(sftp) = &tab.remote.sftp {
        sftp.disconnect();
    }
}

fn empty_local() -> Local {
    Local {
        path: None,
        path_text: String::new(),
        rows: Vec::new(),
        error: None,
        selected: HashSet::new(),
        anchor: None,
        renaming: None,
    }
}

/// A local row's key (its path, as bytes of UTF-16).
fn local_key(row: &LocalRow) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    row.path.as_os_str().encode_wide().flat_map(u16::to_le_bytes).collect()
}

/// A server folder's entries, folders first, by name.
fn list_remote(sftp: &Session, names: Names, path: Vec<u8>) -> What {
    let result = sftp.read_dir(&path).map_err(|e| e.to_string()).map(|entries| {
        let mut rows: Vec<RemoteRow> = entries
            .into_iter()
            .map(|entry| {
                // a link to a folder opens like one
                let dir = entry.attrs.is_dir()
                    || (entry.attrs.is_symlink()
                        && sftp.stat(&native_term_sftp::join(&path, &entry.name)).is_ok_and(|a| a.is_dir()));
                RemoteRow { name: names.decode(&entry.name), dir, entry }
            })
            .collect();
        rows.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        rows
    });
    What::Listed { path, result }
}

/// A local folder's entries (folders first, by name), or the drives.
fn list_local(path: Option<&Path>) -> std::io::Result<Vec<LocalRow>> {
    let Some(path) = path else {
        return Ok(native_term_win::shell::drives()
            .into_iter()
            .map(|d| LocalRow { name: d.display().to_string(), path: d, dir: true, size: None, modified: None })
            .collect());
    };
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let Ok(entry) = entry else { continue };
        let meta = entry.metadata().ok();
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        rows.push(LocalRow {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path(),
            dir: meta.as_ref().is_some_and(|m| m.is_dir()),
            size: meta.as_ref().map(|m| m.len()),
            modified,
        });
    }
    rows.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(rows)
}

/// The current folder of the terminal tab's tmux session (asked with
/// `ssh -o BatchMode=yes`: keys or the agent; otherwise the home folder).
fn terminal_folder(ssh: &Path, config: Option<&Path>, alias: &str, session: &str, sftp: &Session) -> Option<Vec<u8>> {
    let name: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    let command = format!("sh -c 'tmux display-message -p -t ={name}: \"#{{pane_current_path}}\"'");
    let out = native_term_config::persistent::run_remote_bytes(ssh, config, alias, &command).ok()?;
    let path: Vec<u8> = out.strip_suffix(b"\n").unwrap_or(&out).to_vec();
    (path.starts_with(b"/") && sftp.stat(&path).is_ok_and(|a| a.is_dir())).then_some(path)
}

/// Serves ssh's askpass questions on a private pipe (only this user may
/// open it; the name is random) and returns its name. Only the helper
/// started by our ssh (its parent) is answered: the account's own
/// password prompt from Credential Manager when saved, everything else by
/// asking in the window. Cancelling a question cancels the rest of this
/// connection's too (ssh would ask for the password again otherwise).
fn serve_questions(
    tab: u64,
    target: Option<native_term_config::password::Target>,
    ssh_pid: Arc<std::sync::atomic::AtomicU32>,
    served: Arc<AtomicBool>,
    tx: Sender<Event>,
    ctx: egui::Context,
) -> std::io::Result<String> {
    use native_term_session::pipe;
    let name = format!(r"\\.\pipe\NativeTerm-askpass-{}-{:016x}", std::process::id(), unique());
    let mut listener = pipe::PipeListener::bind(&name)?;
    std::thread::spawn(move || {
        let mut cancelled = false;
        while let Ok(conn) = listener.accept() {
            let Ok(Some(prompt)) = conn.recv::<String>(Duration::from_secs(5)) else { continue };
            let helper = conn.client_pid().unwrap_or(0);
            let ours =
                native_term_win::parent_pid(helper).is_some_and(|p| p != 0 && p == ssh_pid.load(Ordering::SeqCst));
            if !ours {
                let _ = conn.send(&None::<String>);
                continue;
            }
            let saved = target
                .as_ref()
                .filter(|t| t.answers(&prompt))
                .and_then(|t| native_term_win::credentials::read(&t.name).ok().flatten())
                .filter(|s| s.comment != native_term_config::password::REFUSED);
            let answer = match saved {
                Some(s) => {
                    served.store(true, Ordering::SeqCst);
                    Some(s.secret)
                }
                None if cancelled => None,
                None => {
                    let (reply, answer) = mpsc::channel();
                    let secret = !prompt.contains("(yes/no");
                    if tx
                        .send(Event { tab, what: What::Ask(Question { prompt: prompt.clone(), secret, reply }) })
                        .is_err()
                    {
                        break;
                    }
                    ctx.request_repaint();
                    let typed = answer.recv_timeout(Duration::from_secs(290)).ok().flatten();
                    cancelled = typed.is_none();
                    typed
                }
            };
            let _ = conn.send(&answer);
        }
    });
    Ok(name)
}

/// Watches an edited file's local copy and uploads it when it changes
/// (and has stopped changing: an editor may save in steps). It goes to a
/// temporary name next to the file first and then replaces it, so the
/// file on the server is never half written; where that isn't allowed
/// (no write access to the folder), the file is written in place. The
/// original's permissions are kept. If the server's file changed since it
/// was opened, nothing is uploaded until "Overwrite".
#[allow(clippy::too_many_arguments)]
fn watch_and_upload(
    sftp: &Session,
    remote: &[u8],
    local: &Path,
    mut attrs: Attrs,
    stop: &AtomicBool,
    conflict: &AtomicBool,
    overwrite: &AtomicBool,
    set: &dyn Fn(String),
) {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut seen = modified(local);
    let mut pending: Option<SystemTime> = None;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(700));
        if sftp.closed().is_some() {
            return set(t!("files-edit-disconnected"));
        }
        let now = modified(local);
        if now != seen {
            // changed: wait until it stays the same for one more look
            seen = now;
            pending = now;
            continue;
        }
        let forced = overwrite.swap(false, Ordering::Relaxed);
        if pending.is_none() && !forced {
            continue;
        }
        // changed on the server meanwhile?
        let server = sftp.stat(remote).ok();
        if !forced && server.as_ref().and_then(|a| a.mtime()) != attrs.mtime() {
            conflict.store(true, Ordering::Relaxed);
            set(t!("files-edit-conflict"));
            continue;
        }
        conflict.store(false, Ordering::Relaxed);
        set(t!("files-edit-uploading"));
        match upload_in_place(sftp, remote, local, &attrs) {
            Ok(()) => {
                pending = None;
                if let Ok(a) = sftp.stat(remote) {
                    attrs = Attrs { permissions: attrs.permissions, ..a };
                }
                let time = native_term_win::local_time_of_day(unix_now());
                set(t!("files-edit-uploaded", time = time));
            }
            Err(e) => set(t!("files-edit-failed", error = e.to_string())),
        }
    }
}

fn upload_in_place(sftp: &Session, remote: &[u8], local: &Path, attrs: &Attrs) -> native_term_sftp::Result<()> {
    let name = last(remote);
    let mut temp_name = b".".to_vec();
    temp_name.extend_from_slice(name);
    temp_name.extend_from_slice(format!(".nt-{:x}", unique()).as_bytes());
    let temp = native_term_sftp::join(&native_term_sftp::parent(remote), &temp_name);
    let replaced = sftp.upload(local, &temp, attrs.permissions, &mut |_| true).and_then(|_| {
        if let Some(p) = attrs.permissions {
            let _ = sftp.setstat(&temp, &Attrs { permissions: Some(p & 0o7777), ..Default::default() });
        }
        sftp.rename(&temp, remote, true)
    });
    match replaced {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = sftp.remove(&temp);
            sftp.upload(local, remote, None, &mut |_| true).map(|_| ())
        }
    }
}

fn modal(ctx: &egui::Context, id: &str, title: String, body: impl FnOnce(&mut egui::Ui)) {
    egui::Window::new(title)
        .id(egui::Id::new(id))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, body);
}

/// An icon alone (the icon font is a fallback of the text font).
fn icon(glyph: char) -> egui::RichText {
    egui::RichText::new(glyph.to_string())
}

/// A path's last part.
fn last(path: &[u8]) -> &[u8] {
    path.rsplit(|&c| c == b'/').find(|p| !p.is_empty()).unwrap_or(path)
}

/// "a.txt" or "a.txt and 3 more".
fn describe(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [first, rest @ ..] => t!("files-and-more", first = first.as_str(), count = rest.len()),
    }
}

pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A duration as a clock: `0:07`, `12:30`, `1:02:03`.
fn clock_text(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `drwxr-xr-x`, as `ls -l` shows it.
pub fn mode_text(mode: u32) -> String {
    let kind = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o020000 => 'c',
        0o060000 => 'b',
        0o010000 => 'p',
        0o140000 => 's',
        _ => '-',
    };
    let mut out = String::from(kind);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 7;
        out.push(if bits & 4 != 0 { 'r' } else { '-' });
        out.push(if bits & 2 != 0 { 'w' } else { '-' });
        out.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    out
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A number unlikely to repeat (for temporary names).
fn unique() -> u64 {
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    nanos ^ (u64::from(std::process::id()) << 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(1536), "1.5 KB");
        assert_eq!(size_text(5 * 1024 * 1024 * 1024), "5.0 GB");
        assert_eq!(clock_text(7), "0:07");
        assert_eq!(clock_text(750), "12:30");
        assert_eq!(clock_text(3723), "1:02:03");
        let gbk = [b'/', 0xd6, 0xd0, 0xce, 0xc4];
        assert_eq!(hex(&gbk), "2fd6d0cec4");
        assert_eq!(unhex(&hex(&gbk)).as_deref(), Some(&gbk[..]));
        assert_eq!(unhex("2fz0"), None);
        assert_eq!(unhex("2f0"), None);
        assert_eq!(unhex(""), Some(Vec::new()));
        assert_eq!(mode_text(0o040755), "drwxr-xr-x");
        assert_eq!(mode_text(0o100600), "-rw-------");
        assert_eq!(mode_text(0o120777), "lrwxrwxrwx");
        assert_eq!(last(b"/home/a/b.txt"), b"b.txt");
        assert_eq!(last(b"/home/a/"), b"a");
    }

    #[test]
    fn local_folders_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), b"12345").unwrap();
        std::fs::create_dir(dir.path().join("Z 文件夹")).unwrap();
        std::fs::write(dir.path().join("A.txt"), b"").unwrap();
        let rows = list_local(Some(dir.path())).unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Z 文件夹", "A.txt", "b.txt"]);
        assert_eq!(rows[2].size, Some(5));
        assert!(list_local(None).unwrap().iter().any(|r| r.dir), "the drives");
    }

    /// An edited file is uploaded when saved, by replacing (the server's
    /// file is never half written), keeping its permissions; a change on
    /// the server meanwhile holds the upload until "Overwrite".
    #[test]
    fn edits_go_back_to_the_server() {
        let server = Path::new(r"C:\Windows\System32\OpenSSH\sftp-server.exe");
        if !server.exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new(server);
        command.arg("-d").arg(dir.path());
        let sftp = Arc::new(Session::spawn(command).unwrap());
        let remote_dir = format!("/{}", dir.path().display().to_string().replace('\\', "/")).into_bytes();
        std::fs::write(dir.path().join("conf.txt"), b"v1").unwrap();
        let remote = native_term_sftp::join(&remote_dir, b"conf.txt");
        let local = dir.path().join("local-copy.txt");
        let attrs = sftp.stat(&remote).unwrap();
        sftp.download(&remote, &local, &mut |_| true).unwrap();

        let (stop, conflict, overwrite) =
            (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let status = Arc::new(Mutex::new(String::new()));
        let (s, r, l, st, c, o, stat) = (
            Arc::clone(&sftp),
            remote.clone(),
            local.clone(),
            Arc::clone(&stop),
            Arc::clone(&conflict),
            Arc::clone(&overwrite),
            Arc::clone(&status),
        );
        let watcher = std::thread::spawn(move || {
            watch_and_upload(&s, &r, &l, attrs, &st, &c, &o, &|text| *stat.lock().unwrap() = text);
        });
        let wait_for = |what: &dyn Fn() -> bool| {
            let end = Instant::now() + Duration::from_secs(15);
            while !what() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(100));
            }
            what()
        };
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(&local, b"v2 saved in the editor").unwrap();
        assert!(
            wait_for(&|| std::fs::read(dir.path().join("conf.txt")).unwrap() == b"v2 saved in the editor"),
            "uploaded"
        );
        // no temporary file left next to it
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!names.iter().any(|n| n.contains(".nt-")), "{names:?}");

        // someone changes it on the server; our next save waits
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(dir.path().join("conf.txt"), b"changed on the server").unwrap();
        std::fs::write(&local, b"v3").unwrap();
        assert!(wait_for(&|| conflict.load(Ordering::Relaxed)), "conflict seen");
        assert_eq!(std::fs::read(dir.path().join("conf.txt")).unwrap(), b"changed on the server");
        overwrite.store(true, Ordering::Relaxed);
        assert!(wait_for(&|| std::fs::read(dir.path().join("conf.txt")).unwrap() == b"v3"), "overwritten when asked");
        stop.store(true, Ordering::Relaxed);
        watcher.join().unwrap();
    }
}
