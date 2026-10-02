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

use native_term_skin::{Selection, Sort};
use std::cell::RefCell;
use std::collections::HashMap;
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
    let viewport = crate::skinned::viewport(&t!("files-window-title"), 1200.0, 760.0)
        .with_min_inner_size([760.0, 420.0])
        .with_drag_and_drop(true);
    crate::window::open(KEY, viewport, |ctx| {
        // shared with its dialogs' windows (see part_window)
        let files = std::rc::Rc::new(std::cell::RefCell::new(FilesWindow::new(ctx)));
        files.borrow_mut().me = std::rc::Rc::downgrade(&files);
        Box::new(crate::part_window::Shared(files))
    });
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
    /// Its row's place in the side's rows (the list shows them sorted).
    index: usize,
    name: String,
    glyph: char,
    dir: bool,
    kind: files_list::Kind,
    /// The local side's type column.
    kind_text: String,
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
    /// What may be done with it (the server's side).
    Permissions,
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
    /// The size and what is free where a side's folder is (bytes).
    Space {
        remote: bool,
        space: Option<(u64, u64)>,
    },
    JobDone {
        job: u64,
        result: Result<(), native_term_sftp::Error>,
    },
    Notice(String, bool),
    Refresh,
    LocalRefresh,
    Ask(Question),
    /// A file whose type isn't known: text or binary? (Auto)
    AskType {
        name: String,
        ext: String,
        reply: Sender<Option<files_mode::Answer>>,
    },
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
    selected: Selection<Vec<u8>>,
    sort: Sort,
    view: files_list::View,
    /// Only the names with this in them (any case).
    filter: String,
    /// The folder's file system: its size, what is free (bytes).
    space: Option<(u64, u64)>,
    renaming: Option<(Vec<u8>, String)>,
    names: Names,
    /// Drawn as connected without a connection: the pictures drawn off
    /// the screen (`files_snapshot`).
    pictured: bool,
}

impl Remote {
    /// Connected, as far as what is shown goes.
    fn up(&self) -> bool {
        self.sftp.is_some() || self.pictured
    }
}

/// The local side of a session: a folder, or `None` for the drives.
struct Local {
    path: Option<PathBuf>,
    path_text: String,
    rows: Vec<LocalRow>,
    error: Option<String>,
    selected: Selection<Vec<u8>>,
    sort: Sort,
    view: files_list::View,
    filter: String,
    space: Option<(u64, u64)>,
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

/// The files window's key (see `window::open`): its dialogs belong to it.
const KEY: &str = "files";

/// The files window's dialogs, each a window of its own that belongs to
/// it (see `part_window`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilesPart {
    /// Comparing and syncing two folders (modeless, made larger or smaller).
    Sync,
    /// A question of the server's.
    Question,
    NewFolder,
    /// Deleting, or closing with transfers going on.
    Confirm,
    /// What may be done with files on the server.
    Chmod,
    /// Text or binary, for a file Auto doesn't know.
    TransferType,
}

impl FilesPart {
    const ALL: [FilesPart; 6] = [
        FilesPart::Sync,
        FilesPart::Question,
        FilesPart::NewFolder,
        FilesPart::Confirm,
        FilesPart::Chmod,
        FilesPart::TransferType,
    ];

    fn window(self) -> crate::part_window::PartWindow {
        let key = match self {
            FilesPart::Sync => "files-sync",
            FilesPart::Question => "files-question",
            FilesPart::NewFolder => "files-new-folder",
            FilesPart::Confirm => "files-confirm",
            FilesPart::Chmod => "files-chmod",
            FilesPart::TransferType => "files-type",
        };
        let sync = self == FilesPart::Sync;
        let owner = crate::window::Owner::Window(KEY.into());
        crate::part_window::PartWindow { key, owner, modal: !sync, resizable: sync }
    }
}

impl crate::part_window::Parts for FilesWindow {
    type Part = FilesPart;

    fn show_part(&mut self, part: FilesPart, ctx: &egui::Context) {
        native_term_skin::set_room(ctx, self.ctx.content_rect());
        match part {
            FilesPart::Sync => self.sync_dialog(ctx),
            FilesPart::Question => self.question_dialog(ctx),
            FilesPart::NewFolder => self.new_folder_dialog(ctx),
            FilesPart::Confirm => self.confirm_dialog(ctx),
            FilesPart::Chmod => self.chmod_dialog(ctx),
            FilesPart::TransferType => self.type_dialog(ctx),
        }
        // what it did shows in the files window
        self.ctx.request_repaint();
    }

    fn part_open(&self, part: FilesPart) -> bool {
        match part {
            FilesPart::Sync => self.sync.is_some(),
            FilesPart::Question => self.question.is_some(),
            FilesPart::NewFolder => self.new_folder.is_some(),
            FilesPart::Confirm => self.confirm.is_some(),
            FilesPart::Chmod => self.chmod.is_some(),
            FilesPart::TransferType => self.ask_type.is_some(),
        }
    }
}

struct FilesWindow {
    /// The window itself, shared with its dialogs' windows.
    me: std::rc::Weak<std::cell::RefCell<FilesWindow>>,
    /// Dialogs' windows asked for and not open yet.
    parts_asked: Vec<FilesPart>,
    ctx: egui::Context,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    tabs: Vec<Tab>,
    /// The session the server's side shows (and the local side too while
    /// the sides are linked).
    active: usize,
    /// The session the local side shows when the sides aren't linked.
    local_active: Option<usize>,
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
    /// On that side, the tree was clicked last (the arrows are its), not
    /// the list.
    tree_focus: bool,
    /// Where the server's side is (for files dropped from Explorer).
    remote_rect: Option<egui::Rect>,
    edit_dir: PathBuf,
    computer: String,
    /// The local tree's top: Desktop, Documents, Downloads, the drives.
    local_roots: Vec<(String, PathBuf)>,
    /// Synchronize: the folders compared, what was found, what to do.
    sync: Option<crate::files_sync::SyncDialog>,
    /// What the panel below the sides shows.
    dock: files_dock::Dock,
    /// Folders kept to go back to.
    bookmarks: files_bookmarks::Bookmarks,
    /// What may be done with files on the server: being asked.
    chmod: Option<files_chmod::Chmod>,
    /// Text or binary: being asked (a transfer waits for it).
    ask_type: Option<files_mode::TypeQuestion>,
}

impl FilesWindow {
    fn new(ctx: &egui::Context) -> FilesWindow {
        WINDOW.with(|w| *w.borrow_mut() = Some(ctx.clone()));
        let (tx, rx) = mpsc::channel();
        FilesWindow {
            me: std::rc::Weak::new(),
            parts_asked: Vec::new(),
            ctx: ctx.clone(),
            tx,
            rx,
            tabs: Vec::new(),
            active: 0,
            local_active: None,
            next_id: 1,
            jobs: Vec::new(),
            new_folder: None,
            confirm: None,
            close: false,
            question: None,
            remote_focus: true,
            tree_focus: false,
            remote_rect: None,
            edit_dir: std::env::temp_dir().join("NativeTerm-edit"),
            computer: native_term_os::host::name(),
            local_roots: native_term_os::shell::user_folders()
                .into_iter()
                .chain(native_term_os::shell::drives())
                .map(|p| {
                    (p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(p.display().to_string()), p)
                })
                .collect(),
            sync: None,
            dock: files_dock::Dock::default(),
            bookmarks: files_bookmarks::Bookmarks::default(),
            chmod: None,
            ask_type: None,
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
            let time = native_term_os::time::local_time_of_day(unix_now());
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
                    selected: Selection::default(),
                    sort: NAME_SORT,
                    view: files_list::View::Details,
                    filter: String::new(),
                    space: None,
                    renaming: None,
                    names,
                    pictured: false,
                },
                local: Local {
                    path: None,
                    path_text: String::new(),
                    rows: Vec::new(),
                    error: None,
                    selected: Selection::default(),
                    sort: NAME_SORT,
                    view: files_list::View::Details,
                    filter: String::new(),
                    space: None,
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
                .or_else(native_term_os::shell::downloads_folder);
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
                native_term_os::credentials::read(&t.name)
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
                            let _ = native_term_os::credentials::update(&t.name, |s| {
                                s.comment = native_term_config::password::REFUSED.to_string();
                            });
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
            tab.remote.renaming = None;
            // a filter is for the folder it was typed in
            if tab.remote.path != path {
                tab.remote.filter.clear();
            }
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
            tab.local.renaming = None;
            if tab.local.path != path {
                tab.local.filter.clear();
            }
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
                            let keys: Vec<Vec<u8>> = r.rows.iter().map(|x| x.entry.name.clone()).collect();
                            r.selected.keep(&keys);
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
                    // what is free there (where the server says)
                    if let Some(sftp) = self.tab(id).and_then(|t| t.remote.sftp.clone()) {
                        self.spawn(id, move || {
                            let space = sftp.space(&path).ok().flatten().map(|s| (s.total, s.available));
                            What::Space { remote: true, space }
                        });
                    }
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
                    // (off this thread: a network drive may take its time)
                    let at = path.clone();
                    self.spawn(id, move || What::Space {
                        remote: false,
                        space: native_term_os::shell::disk_space(&at),
                    });
                }
            }
            What::Space { remote, space } => {
                if let Some(tab) = self.tab(id) {
                    if remote {
                        tab.remote.space = space;
                    } else {
                        tab.local.space = space;
                    }
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
            What::AskType { name, ext, reply } => {
                if let Some(old) = self.ask_type.take() {
                    let _ = old.reply.send(None);
                }
                self.ask_type =
                    Some(files_mode::TypeQuestion { name, ext, reply, text: false, always: false, all: false });
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
        let Some(tab) = self.tabs.iter().find(|t| t.id == dragged.tab) else { return Vec::new() };
        let r = &tab.remote;
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
        let Some(tab) = self.tabs.iter().find(|t| t.id == dragged.tab) else { return Vec::new() };
        let l = &tab.local;
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
        let mode = files_mode::Mode::read(self.core());
        let types = files_mode::chosen_types(self.core());
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
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
                    // and each file as text or binary (Auto may ask)
                    plan_work(&sftp, &work, &progress).and_then(|mut items| {
                        let ask = |name: &str, ext: &str| {
                            let (reply, answer) = mpsc::channel();
                            let what = What::AskType { name: name.to_string(), ext: ext.to_string(), reply };
                            tx.send(Event { tab, what }).ok()?;
                            ctx.request_repaint();
                            // (a cancel meanwhile ends the wait)
                            loop {
                                match answer.recv_timeout(Duration::from_millis(250)) {
                                    Ok(a) => return a,
                                    Err(mpsc::RecvTimeoutError::Timeout) if !progress.stopped() => continue,
                                    Err(_) => return None,
                                }
                            }
                        };
                        files_mode::decide(&mut items, mode, types, ask).map(|()| items)
                    })
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
        self.spawn(id, move || match native_term_os::shell::recycle(&paths) {
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
            if let Err(e) = native_term_os::shell::open_file(&local) {
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
            self.local_active =
                self.local_active.map(|l| if l > i { l - 1 } else { l.min(self.tabs.len().saturating_sub(1)) });
        }
    }

    /// The session a side shows: the server's side the active one, the
    /// local side its own while the sides aren't linked.
    pub(super) fn side_index(&self, remote: bool) -> usize {
        match (remote, self.local_active) {
            (false, Some(l)) => l.min(self.tabs.len().saturating_sub(1)),
            _ => self.active,
        }
    }

    /// One row of session tabs; `remote`: the server's side (with close).
    fn local_side(&mut self, ui: &mut egui::Ui) {
        self.strip(ui, false);
        let at = self.side_index(false);
        // where uploads go: the server's side's session
        let remote_id = self.tabs.get(self.active).map(|t| t.id);
        let Some(tab) = self.tabs.get(at) else { return };
        let id = tab.id;
        for asked in self.head(ui, false) {
            self.local_asked(id, asked, ui.ctx());
        }
        if let Some(e) = self.tabs.get(at).and_then(|t| t.local.error.clone()) {
            ui.colored_label(RED, e);
        }
        // the foot, under the tree and the list
        egui::Panel::bottom("local-status").frame(egui::Frame::NONE).show(ui, |ui| self.foot(ui, false));
        // the tree
        let roots = self.local_roots.clone();
        let tree_out = egui::Panel::left("local-tree")
            .resizable(true)
            .default_size(180.0)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin { right: 6, ..Default::default() }))
            .show(ui, |ui| {
                let tab = &self.tabs[at];
                let keyboard = !self.remote_focus && self.tree_focus;
                tree(ui, "local-tree", &roots, &tab.local_tree, tab.local.path.as_ref(), false, keyboard)
            })
            .inner;
        if tree_out.clicked {
            (self.remote_focus, self.tree_focus) = (false, true);
        }
        if let Some(folder) = tree_out.go {
            self.list_local(id, Some(folder));
        }
        if let Some(folder) = tree_out.toggle {
            self.toggle_local(id, folder);
        }
        if let Some((folder, dragged)) = tree_out.dropped {
            let items = self.dragged_remote(&dragged);
            self.download_to(dragged.tab, items, Some(folder));
        }
        // the list
        let tab = &self.tabs[at];
        let lines: Vec<Line> = tab
            .local
            .rows
            .iter()
            .enumerate()
            .map(|(index, r)| Line {
                key: local_key(r),
                index,
                name: r.name.clone(),
                glyph: if r.dir { icons::FOLDER } else { icons::DOCUMENT },
                dir: r.dir,
                kind: files_list::kind(&r.name, r.dir, false),
                kind_text: files_list::type_text(&r.name, r.dir),
                size: (!r.dir).then_some(r.size).flatten(),
                modified: r.modified,
                mode: None,
            })
            .collect();
        let lines = filtered(sorted(lines, tab.local.sort, false), &tab.local.filter);
        let keyboard = !self.remote_focus && !self.tree_focus;
        let mut local = std::mem::replace(&mut self.tabs[at].local, empty_local());
        let side = files_list::Side { salt: "local-list", remote: false, keyboard };
        let out = list(ui, side, &lines, &mut local.selected, &mut local.renaming, &mut local.sort, local.view);
        self.tabs[at].local = local;
        if out.clicked {
            (self.remote_focus, self.tree_focus) = (false, false);
        }
        if let Some(i) = out.open {
            self.open_local(id, i);
        }
        if let Some((old, new)) = out.renamed {
            if let Some(row) = self.tabs[at].local.rows.iter().find(|r| local_key(r) == old) {
                let path = row.path.clone();
                self.rename_local(id, path, new);
            }
        }
        if out.drag {
            let keys = self.tabs[at].local.selected.iter().cloned().collect();
            out.response.dnd_set_drag_payload(Dragged { tab: id, from_remote: false, keys });
        }
        if let Some(dragged) = out.dropped.filter(|d| d.from_remote) {
            let items = self.dragged_remote(&dragged);
            let here = self.tabs.get(at).and_then(|t| t.local.path.clone());
            self.download_to(dragged.tab, items, here);
        }
        match out.action {
            Some((i, Action::Open)) => self.open_local(id, i),
            Some((_, Action::Transfer)) => {
                let files = self.local_selection(id);
                if let Some(remote_id) = remote_id {
                    self.upload(remote_id, files);
                }
            }
            Some((i, Action::Rename)) => {
                let row = self.tabs[at].local.rows[i].clone();
                self.tabs[at].local.renaming = Some((local_key(&row), row.name));
            }
            Some((_, Action::CopyPath)) => {
                let text: Vec<String> = self.local_selection(id).iter().map(|p| p.display().to_string()).collect();
                ui.ctx().copy_text(text.join("\n"));
            }
            Some((_, Action::Delete)) => self.ask_recycle(id),
            Some((_, Action::Edit | Action::Permissions)) | None => {}
        }
    }

    /// A new terminal tab of the session's host, gone into the folder the
    /// server's side shows (typed after login, as a host's after-login
    /// command is; that one isn't typed in this tab).
    fn terminal_here(&mut self, id: u64) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return };
        let Some(core) = tab.spec.memory.clone() else { return };
        let folder = tab.remote.names.decode(&tab.remote.path);
        let request = native_term_app::HostRequest {
            on_login: Some(format!("cd {}", shell_quoted(&folder))),
            ..native_term_app::HostRequest::new(tab.spec.alias.clone(), tab.spec.label.clone())
        };
        core.open(&[request], native_term_platform::Target::Recent);
        self.log(id, t!("files-terminal-opened", folder = folder.as_str()), false);
    }

    /// What the local side's bar asked for.
    fn local_asked(&mut self, id: u64, asked: files_pane::Asked, ctx: &egui::Context) {
        use files_pane::Asked;
        let Some(tab) = self.tab(id) else { return };
        let path = tab.local.path.clone();
        match asked {
            Asked::Up => self.list_local(id, path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf))),
            Asked::Refresh => self.refresh_local(id),
            Asked::Crumb(i) => {
                let crumbs = files_pane::local_crumbs(path.as_deref(), &self.computer);
                if let Some((_, to)) = crumbs.into_iter().nth(i) {
                    self.list_local(id, to);
                }
            }
            Asked::Typed(text) => {
                let text = text.trim();
                self.list_local(id, (!text.is_empty()).then(|| PathBuf::from(text)));
            }
            Asked::Send => {
                let files = self.local_selection(id);
                let remote_id = self.tabs.get(self.active).map_or(id, |t| t.id);
                self.upload(remote_id, files);
            }
            Asked::Bookmark(picked) => self.bookmark_picked(false, picked),
            Asked::NewFolder if path.is_some() => self.new_folder = Some((id, false, String::new())),
            Asked::Delete if path.is_some() => self.ask_recycle(id),
            Asked::CopyPath => {
                let text: Vec<String> = self.local_selection(id).iter().map(|p| p.display().to_string()).collect();
                ctx.copy_text(text.join("\n"));
            }
            _ => {}
        }
    }

    /// What the server's side's bar asked for.
    fn remote_asked(&mut self, id: u64, asked: files_pane::Asked, ctx: &egui::Context) {
        use files_pane::Asked;
        let Some(tab) = self.tab(id) else { return };
        let (path, names) = (tab.remote.path.clone(), tab.remote.names);
        match asked {
            Asked::Up => self.go(id, native_term_sftp::parent(&path)),
            Asked::Refresh => self.refresh(id),
            Asked::Crumb(i) => {
                if let Some((_, to)) = files_pane::remote_crumbs(&path, names).into_iter().nth(i) {
                    self.go(id, to);
                }
            }
            Asked::Typed(text) => match names.encode(text.trim()) {
                Some(p) if !p.is_empty() => self.go(id, p),
                _ => self.log(id, t!("files-bad-name", name = text.as_str()), true),
            },
            Asked::Send => {
                let items = self.remote_selection(id);
                let here = self.tabs.get(self.side_index(false)).and_then(|t| t.local.path.clone());
                self.download_to(id, items, here);
            }
            Asked::Bookmark(picked) => self.bookmark_picked(true, picked),
            Asked::NewFolder => self.new_folder = Some((id, true, String::new())),
            Asked::Delete => self.ask_delete_remote(id),
            Asked::Edit => {
                let tab = &self.tabs[self.active];
                let file = tab
                    .remote
                    .rows
                    .iter()
                    .find(|r| !r.dir && tab.remote.selected.len() == 1 && tab.remote.selected.contains(&r.entry.name))
                    .cloned();
                if let Some(row) = file {
                    self.edit(id, &row);
                }
            }
            Asked::Sync => self.open_sync(id),
            Asked::Chmod => self.ask_chmod(id),
            Asked::Terminal => self.terminal_here(id),
            Asked::CopyPath => {
                let tab = &self.tabs[self.active];
                let text: Vec<String> =
                    self.remote_selection(id).iter().map(|(p, _)| tab.remote.names.decode(p)).collect();
                ctx.copy_text(text.join("\n"));
            }
            Asked::Names(choice) => {
                if let Some(tab) = self.tab(id) {
                    let r = &mut tab.remote;
                    r.names = choice;
                    for row in &mut r.rows {
                        row.name = choice.decode(&row.entry.name);
                    }
                    r.path_text = choice.decode(&r.path);
                }
            }
        }
    }

    fn open_local(&mut self, id: u64, i: usize) {
        let Some(row) = self.tabs.get(self.active).and_then(|t| t.local.rows.get(i)).cloned() else { return };
        if row.dir {
            self.list_local(id, Some(row.path));
        } else if let Err(e) = native_term_os::shell::open_file(&row.path) {
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
        self.strip(ui, true);
        let Some(tab) = self.tabs.get(self.active) else {
            ui.weak(t!("files-no-tabs"));
            return;
        };
        let id = tab.id;
        for asked in self.head(ui, true) {
            self.remote_asked(id, asked, ui.ctx());
        }
        // the foot, at the very bottom (in line with the local one)
        egui::Panel::bottom("remote-status").frame(egui::Frame::NONE).show(ui, |ui| self.foot(ui, true));
        let tab = &self.tabs[self.active];
        match (tab.remote.up(), &tab.remote.failed) {
            (_, Some(error)) => {
                ui.colored_label(RED, error);
                ui.weak(t!("files-batch"));
                if ui.button(t!("files-reconnect")).clicked() {
                    self.connect(id);
                }
                return;
            }
            (false, None) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t!("files-connecting", host = tab.spec.alias.as_str()));
                });
                return;
            }
            (true, None) => {}
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
            .show(ui, |ui| {
                let tab = &self.tabs[self.active];
                let keyboard = self.remote_focus && self.tree_focus;
                tree(ui, "remote-tree", &roots, &tab.remote_tree, Some(&tab.remote.path), true, keyboard)
            })
            .inner;
        if tree_out.clicked {
            (self.remote_focus, self.tree_focus) = (true, true);
        }
        if let Some(folder) = tree_out.go {
            self.go(id, folder);
        }
        if let Some(folder) = tree_out.toggle {
            self.toggle_remote(id, folder);
        }
        if let Some((folder, dragged)) = tree_out.dropped {
            let files = self.dragged_local(&dragged);
            self.upload_to(id, files, Some(folder));
        }
        let tab = &self.tabs[self.active];
        let lines: Vec<Line> = tab
            .remote
            .rows
            .iter()
            .enumerate()
            .map(|(index, r)| Line {
                key: r.entry.name.clone(),
                index,
                name: r.name.clone(),
                glyph: if r.dir {
                    icons::FOLDER
                } else if r.entry.attrs.is_symlink() {
                    icons::LINK
                } else {
                    icons::DOCUMENT
                },
                dir: r.dir,
                kind: files_list::kind(&r.name, r.dir, r.entry.attrs.is_symlink()),
                kind_text: String::new(),
                size: if r.dir { None } else { r.entry.attrs.size },
                modified: r.entry.attrs.mtime().map(u64::from),
                mode: r.entry.attrs.permissions.map(mode_text),
            })
            .collect();
        let lines = filtered(sorted(lines, tab.remote.sort, true), &tab.remote.filter);
        let keyboard = self.remote_focus && !self.tree_focus;
        let mut remote = std::mem::take(&mut self.tabs[self.active].remote.selected);
        let mut sort = self.tabs[self.active].remote.sort;
        let mut renaming = self.tabs[self.active].remote.renaming.take();
        let side = files_list::Side { salt: "remote-list", remote: true, keyboard };
        let view = self.tabs[self.active].remote.view;
        let out = list(ui, side, &lines, &mut remote, &mut renaming, &mut sort, view);
        {
            let r = &mut self.tabs[self.active].remote;
            r.selected = remote;
            r.sort = sort;
            r.renaming = renaming;
        }
        self.remote_rect = Some(out.response.rect);
        if out.clicked {
            (self.remote_focus, self.tree_focus) = (true, false);
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
        if let Some(dragged) = out.dropped.filter(|d| !d.from_remote) {
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
            Some((_, Action::Permissions)) => self.ask_chmod(id),
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
    /// The sync's dialog (modeless: the window stays in use).
    fn sync_dialog(&mut self, ctx: &egui::Context) {
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
    }

    /// A question of the server's (a password, a passphrase).
    fn question_dialog(&mut self, ctx: &egui::Context) {
        if let Some((tab, question, typed)) = &mut self.question {
            let host = self.tabs.iter().find(|t| t.id == *tab).map(|t| t.spec.alias.clone()).unwrap_or_default();
            let mut done = None;
            let closed = modal(
                ctx,
                "files-question",
                t!("files-question-title", host = host.as_str()),
                crate::icons::LOCK,
                |ui| {
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
                    done = ok_cancel(ui, t!("button-ok"), native_term_skin::Role::Primary).or(enter.then_some(true));
                },
            );
            if closed && done.is_none() {
                done = Some(false);
            }
            if let Some(ok) = done {
                // the field's focus goes with the dialog (else the list's keys stay dead)
                ctx.memory_mut(|m| m.stop_text_input());
                if let Some((_, question, typed)) = self.question.take() {
                    let _ = question.reply.send(ok.then_some(typed));
                }
            }
        }
    }

    /// A new folder's name.
    fn new_folder_dialog(&mut self, ctx: &egui::Context) {
        if let Some((tab, remote, name)) = &mut self.new_folder {
            let (tab, remote) = (*tab, *remote);
            let mut done = None;
            let closed = modal(ctx, "files-new-folder", t!("files-new-folder"), crate::icons::NEW_FOLDER, |ui| {
                let edit = ui.add(egui::TextEdit::singleline(name).hint_text(t!("files-new-folder-hint")));
                edit.request_focus();
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                done = ok_cancel(ui, t!("button-create"), native_term_skin::Role::Primary).or(enter.then_some(true));
            });
            if closed && done.is_none() {
                done = Some(false);
            }
            if let Some(ok) = done {
                ctx.memory_mut(|m| m.stop_text_input());
                let name = self.new_folder.take().map(|(_, _, n)| n).unwrap_or_default();
                if ok && !name.trim().is_empty() {
                    self.mkdir(tab, remote, name);
                }
            }
        }
    }

    /// Deleting, or closing while transfers or edits are going on: asked first.
    fn confirm_dialog(&mut self, ctx: &egui::Context) {
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
        use native_term_skin::{Choice, Message, Notice, Role};
        let skin = crate::looks::skin(&ctx.global_style().visuals);
        let shown = Message::new("files-confirm", &title)
            .icon(crate::icons::DELETE)
            .notice(Notice::Warning)
            .choice(Choice::new(button, Role::Danger))
            .choice(Choice::new(t!("button-cancel"), Role::Plain))
            .show(
                ctx,
                &skin,
                |ui| {
                    ui.label(text);
                    if let Some(w) = warning {
                        ui.colored_label(skin.palette.danger, w);
                    }
                },
                |_| {},
            );
        let done = match shown.pressed {
            Some(i) => Some(i == 0),
            None => shown.closed.then_some(false),
        };
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
        // Ctrl+D keeps the folder shown, Ctrl+B shows the side's bookmarks
        let (keep, bookmarks) = ctx.input(|i| {
            let k = |key| i.events.iter().any(|e| matches!(e, egui::Event::Key { key: k, pressed: true, modifiers, .. } if *k == key && modifiers.command));
            (k(egui::Key::D), k(egui::Key::B))
        });
        if keep {
            self.bookmark_picked(self.remote_focus, files_bookmarks::Picked::Toggle);
        }
        if bookmarks {
            egui::Popup::toggle_id(ctx, files_bookmarks::menu_id(self.remote_focus));
        }
        // (Enter, the arrows, typing: the list's own, see `files_list`)
        let (f5, back, delete, f2) = ctx.input(|i| {
            let k = |key| i.key_pressed(key);
            (k(egui::Key::F5), k(egui::Key::Backspace), k(egui::Key::Delete), k(egui::Key::F2))
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
        } else {
            // the local side's own session (the sides may not be linked)
            let at = self.side_index(false);
            let tab = &self.tabs[at];
            let id = tab.id;
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
                let row = self.tabs[at].local.rows[i].clone();
                self.tabs[at].local.renaming = Some((local_key(&row), row.name));
            }
        }
    }
}

/// What a tree reports back.
struct TreeOut<P> {
    /// A folder clicked or moved to: show it.
    go: Option<P>,
    /// A folder to open or close.
    toggle: Option<P>,
    /// Files from the other side dropped on a folder.
    dropped: Option<(P, Arc<Dragged>)>,
    /// A row clicked (the keys are the tree's now).
    clicked: bool,
}

/// A side's folder tree on the skin's `TreeView`: chevrons open and
/// close, a click or the keys show a folder, the one shown in the side's
/// colour, and files dragged from the other side can be dropped on a
/// folder. `remote`: the server's side (it takes files from the local
/// side, and the other way round).
fn tree<P: Clone + Eq + Hash + std::fmt::Debug>(
    ui: &mut egui::Ui,
    salt: &str,
    roots: &[(String, P)],
    tree: &Tree<P>,
    current: Option<&P>,
    remote: bool,
    keyboard: bool,
) -> TreeOut<P> {
    let look = files_list::Look::of(ui.visuals());
    let palette = crate::looks::skin(ui.visuals()).palette;
    let mut rows = Vec::new();
    for (name, path) in roots {
        lay_out(&mut rows, name, path, 0, tree, look.folder);
    }
    let at = current.and_then(|c| rows.iter().position(|r| &r.key == c));
    let accent = look.side(remote);
    let ink = if remote { look.remote } else { look.local_ink };
    let mut out = TreeOut { go: None, toggle: None, dropped: None, clicked: false };
    let shown = native_term_skin::TreeView::new(salt, &palette).accent(accent).ink(ink).keyboard(keyboard).show(
        ui,
        &rows,
        at,
        |i, response| {
            out.clicked |= response.clicked();
            if response.dnd_hover_payload::<Dragged>().is_some_and(|d| d.from_remote != remote) {
                let painter = response.ctx.layer_painter(response.layer_id);
                painter.rect_stroke(response.rect, 3.0, egui::Stroke::new(1.5_f32, accent), egui::StrokeKind::Inside);
            }
            if let Some(dragged) = response.dnd_release_payload::<Dragged>().filter(|d| d.from_remote != remote) {
                out.dropped = Some((rows[i].key.clone(), dragged));
            }
        },
    );
    out.go = shown.go.map(|i| rows[i].key.clone());
    out.toggle = shown.toggle.map(|i| rows[i].key.clone());
    out
}

/// `path` and, when open, what is under it, as the tree's rows.
fn lay_out<P: Clone + Eq + Hash>(
    rows: &mut Vec<native_term_skin::TreeRow<P>>,
    name: &str,
    path: &P,
    depth: usize,
    tree: &Tree<P>,
    folder: egui::Color32,
) {
    use native_term_skin::Kids;
    let node = tree.nodes.get(path);
    let open = node.is_some_and(|n| n.open);
    let children = node.and_then(|n| n.children.as_ref());
    rows.push(native_term_skin::TreeRow {
        key: path.clone(),
        depth,
        label: name.to_string(),
        icon: Some(((if open { icons::FOLDER_OPEN } else { icons::FOLDER }).to_string(), folder)),
        kids: match children {
            Some(c) if c.is_empty() => Kids::No,
            Some(_) => Kids::Yes,
            None => Kids::Unknown,
        },
        open,
        loading: open && children.is_none(),
    });
    if let (true, Some(children)) = (open, children) {
        for (child_name, child) in children {
            lay_out(rows, child_name, child, depth + 1, tree, folder);
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
    /// The row dragged (its place in the list).
    drag_row: Option<usize>,
    dropped: Option<Arc<Dragged>>,
}

#[path = "files_bookmarks.rs"]
mod files_bookmarks;
#[path = "files_chmod.rs"]
mod files_chmod;
#[path = "files_diff.rs"]
mod files_diff;
#[path = "files_dock.rs"]
mod files_dock;
#[path = "files_list.rs"]
mod files_list;
#[path = "files_mode.rs"]
mod files_mode;
#[path = "files_pane.rs"]
mod files_pane;

/// The rail between the sides (`.rail`).
const RAIL: f32 = 44.0;
use files_list::list;

#[cfg(test)]
#[path = "files_snapshot.rs"]
pub(crate) mod snapshot;

impl crate::window::Ui for FilesWindow {
    fn ui(&mut self, ui: &mut egui::Ui) {
        let skin = crate::skinned::chrome_resizable(ui, &t!("files-window-title"), crate::icons::FOLDER);
        self.take_pending();
        while let Ok(event) = self.rx.try_recv() {
            self.handle(event);
        }
        let ctx = ui.ctx().clone();
        // after the events: a tab that has just connected is ready here
        self.take_pending_uploads(&ctx);
        // files dropped from Explorer onto the server's side: uploaded there
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect()
        });
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
        // the session shown, in the title bar's middle
        let bar = egui::Rect::from_min_size(
            ctx.content_rect().min,
            egui::vec2(ctx.content_rect().width(), native_term_skin::TITLE_BAR),
        );
        self.title_middle(ui, bar);
        let frame = egui::Frame::NONE.fill(skin.palette.page);
        // the status line at the very bottom, the panel of transfers, log
        // and errors above it (always there: empty, it says how to start)
        egui::Panel::bottom("files-status")
            .exact_size(30.0)
            .resizable(false)
            .frame(frame)
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::bottom("files-dock")
            .resizable(true)
            .default_size(224.0)
            .min_size(96.0)
            .max_size(420.0)
            .frame(frame)
            .show(ui, |ui| self.dock(ui));
        let half = (ui.available_width() - RAIL) / 2.0;
        egui::Panel::left("files-local")
            .resizable(true)
            .default_size(half)
            .frame(frame)
            .show(ui, |ui| self.local_side(ui));
        egui::Panel::left("files-rail").exact_size(RAIL).resizable(false).frame(frame).show(ui, |ui| self.rail(ui));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| self.remote_side(ui));
        // its dialogs: each a window of its own
        let parts = FilesPart::ALL.map(|part| (part, part.window()));
        let me = self.me.clone();
        let mut asked = std::mem::take(&mut self.parts_asked);
        crate::part_window::sync(&me, self, &mut asked, &parts);
        self.parts_asked = asked;
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
        selected: Selection::default(),
        sort: NAME_SORT,
        view: files_list::View::Details,
        filter: String::new(),
        space: None,
        renaming: None,
    }
}

/// A list as a side is sorted.
fn sorted(lines: Vec<Line>, sort: Sort, remote: bool) -> Vec<Line> {
    let order = files_list::order(&lines, sort, remote);
    let mut lines: Vec<Option<Line>> = lines.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| lines[i].take()).collect()
}

/// `text` as one word to a POSIX shell: in single quotes, a quote in it
/// closed, escaped and opened again.
fn shell_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Only the lines whose names have `filter` in them (any case).
fn filtered(lines: Vec<Line>, filter: &str) -> Vec<Line> {
    let filter = filter.trim().to_lowercase();
    if filter.is_empty() {
        return lines;
    }
    lines.into_iter().filter(|l| l.name.to_lowercase().contains(&filter)).collect()
}

/// By name, A to Z: how a side starts.
const NAME_SORT: Sort = Sort { column: 0, descending: false };

/// A local row's key (its path, as bytes).
fn local_key(row: &LocalRow) -> Vec<u8> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        row.path.as_os_str().encode_wide().flat_map(u16::to_le_bytes).collect()
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::ffi::OsStrExt;
        row.path.as_os_str().as_bytes().to_vec()
    }
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
        return Ok(native_term_os::shell::drives()
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
            let ours = native_term_os::process::parent_pid(helper)
                .is_some_and(|p| p != 0 && p == ssh_pid.load(Ordering::SeqCst));
            if !ours {
                let _ = conn.send(&None::<String>);
                continue;
            }
            let saved = target
                .as_ref()
                .filter(|t| t.answers(&prompt))
                .and_then(|t| native_term_os::credentials::read(&t.name).ok().flatten())
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
                let time = native_term_os::time::local_time_of_day(unix_now());
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

/// A dialog of the files window's, on the skin. Whether it was given up
/// (its close button, Escape).
fn modal(ctx: &egui::Context, id: &str, title: String, icon: char, body: impl FnOnce(&mut egui::Ui)) -> bool {
    let skin = crate::looks::skin(&ctx.global_style().visuals);
    native_term_skin::Modal::new(id, &title).icon(icon).show(ctx, &skin, body).closed
}

/// The row of buttons at a files dialog's bottom: the one it is for and
/// Cancel. True for the first, false for Cancel.
fn ok_cancel(ui: &mut egui::Ui, ok: String, role: native_term_skin::Role) -> Option<bool> {
    let skin = crate::looks::skin(ui.visuals());
    let choices = [
        native_term_skin::Choice::new(ok, role),
        native_term_skin::Choice::new(t!("button-cancel"), native_term_skin::Role::Plain),
    ];
    ui.add_space(8.0);
    native_term_skin::footer(ui, &skin, |_| {}, &choices).map(|i| i == 0)
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
    fn a_folder_is_one_word_to_the_shell() {
        assert_eq!(shell_quoted("/srv/my files"), "'/srv/my files'");
        assert_eq!(shell_quoted("/tmp/it's"), r"'/tmp/it'\''s'");
    }

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
