//! The main window: a rail of icons for its pages (the session tree,
//! the hosts used lately, the open sessions, all tabs, sending commands,
//! importing), a header, the page, a bar below, and the dialogs that
//! edit sessions. Laid out after the design the person brought
//! (`layout.rs`; `docs/ARCHITECTURE.md`, "The main window").

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use native_term_app::actions::{CloseSet, Closing, SessionCommand};
use native_term_app::tab_menu::MenuRequest;
use native_term_app::{t, Core, SessionView, State};
use native_term_config::ops::{Editor, HostDraft};
use native_term_config::session_log::LogSettings;
use native_term_config::write::Writer;
use native_term_config::SessionTree;

use crate::dialogs::{
    ConfirmCloseMixed, ConfirmDelete, ConfirmForget, DropChoice, DropDialog, FolderDialog, HostDialog, Outcome,
    TagDialog,
};
use crate::icons;
use crate::import_dialog::ImportDialog;
use crate::key_dialog::KeyDialog;
use crate::layout::{self, Kind, Room};
use crate::looks::Tones;
use crate::options_dialog::{OptionsDialog, OptionsTarget};
use crate::plink_dialog::PlinkDialog;
use crate::send_dialog::SendDialog;
use crate::send_line::SendLine;
use crate::server_sessions::ServerSessionsDialog;
use crate::tab_list::TabList;
use crate::terminal_profile::ProfileSetup;
use crate::tree_view::{Activity, Chosen, Scope, Shown, TreeAction, TreeView};
use crate::Setup;

/// `state.db` setting: the docked window stays out.
const PINNED_SETTING: &str = "dock_pinned";

enum Dialog {
    Host(Box<HostDialog>),
    Plink(Box<PlinkDialog>),
    Folder(FolderDialog),
    Tag(TagDialog),
    Delete(ConfirmDelete),
    Forget(ConfirmForget),
    Key(Box<KeyDialog>),
    CloseMixed(ConfirmCloseMixed),
    Options(Box<OptionsDialog>),
    Import(Box<ImportDialog>),
    Send(Box<SendDialog>),
    Drop(Box<DropDialog>),
    ServerSessions(Box<ServerSessionsDialog>),
    CredentialSets(Box<crate::credential_sets::CredentialSetsDialog>),
    Cleanup(Box<crate::cleanup::CleanupDialog>),
}

/// Opening at least this many hosts that forward the ssh-agent is pointed out.
const AGENT_NOTICE_MIN: usize = 3;

/// What the window shows: the rail's icons, from its top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    /// The session tree, and what is chosen in it.
    Tree,
    /// The hosts used lately, the latest first.
    Recent,
    /// The open sessions.
    Sessions,
    /// Every tab of the terminal, the person's own too.
    Tabs,
    /// A command to the active session, or to all of them.
    Send,
    /// Sessions from SecureCRT and PuTTY.
    Import,
}

impl Page {
    const ALL: [Page; 6] = [Page::Tree, Page::Recent, Page::Sessions, Page::Tabs, Page::Send, Page::Import];

    fn icon(self) -> char {
        match self {
            Page::Tree => icons::TREE,
            Page::Recent => icons::HISTORY,
            Page::Sessions => icons::CONNECT,
            Page::Tabs => icons::TABS,
            Page::Send => icons::SEND,
            Page::Import => icons::IMPORT,
        }
    }

    fn title(self) -> String {
        match self {
            Page::Tree => t!("page-tree"),
            Page::Recent => t!("page-recent"),
            Page::Sessions => t!("page-sessions"),
            Page::Tabs => t!("view-tabs"),
            Page::Send => t!("page-send"),
            Page::Import => t!("page-import"),
        }
    }

    /// A line about it, under its title (`hosts`: how many are saved).
    fn about(self, hosts: usize) -> String {
        match self {
            Page::Tree => t!("page-tree-about", count = hosts),
            Page::Recent => t!("page-recent-about"),
            Page::Sessions => t!("page-sessions-about"),
            Page::Tabs => t!("page-tabs-about"),
            Page::Send => t!("page-send-about"),
            Page::Import => t!("page-import-about"),
        }
    }
}

/// The settings' pages, in the window the rail's gear opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsPage {
    General,
    Look,
    Terminal,
    Keys,
    Shortcuts,
    Data,
}

impl SettingsPage {
    const ALL: [SettingsPage; 6] = [
        SettingsPage::General,
        SettingsPage::Look,
        SettingsPage::Terminal,
        SettingsPage::Keys,
        SettingsPage::Shortcuts,
        SettingsPage::Data,
    ];

    fn title(self) -> String {
        match self {
            SettingsPage::General => icons::with(icons::SETTINGS, t!("settings-general")),
            SettingsPage::Look => icons::with(icons::SUN, t!("theme-label")),
            SettingsPage::Terminal => icons::with(icons::TERMINAL, t!("settings-terminal-page")),
            SettingsPage::Keys => icons::with(icons::KEY, t!("settings-keys")),
            SettingsPage::Shortcuts => icons::with(icons::LIST, t!("keys-title")),
            SettingsPage::Data => icons::with(icons::FOLDER, t!("settings-data")),
        }
    }
}

/// What the session tree was loaded from: the config files with their
/// modification time and size.
type Fingerprint = Vec<(PathBuf, Option<std::time::SystemTime>, u64)>;

/// `state.db` setting: where folder files live, when moved from `config.d`.
pub const FOLDERS_SETTING: &str = "folders_dir";

/// The moved folder location (from the setting), for every editor this
/// process makes.
static FOLDERS_DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Where folder files live: the moved location, or `config.d`.
pub(crate) fn folders_dir(ssh_dir: &Path) -> PathBuf {
    FOLDERS_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| ssh_dir.join("config.d"))
}

fn set_folders_dir(dir: Option<PathBuf>) {
    *FOLDERS_DIR.lock().unwrap_or_else(|e| e.into_inner()) = dir;
}

/// Watches the folder files when they live outside `~/.ssh` (that folder
/// has its own watcher).
fn watch_folders(
    ssh_dir: &Path,
    flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ctx: &egui::Context,
) -> Option<native_term_os::watch::FolderWatcher> {
    let dir = folders_dir(ssh_dir);
    if dir.starts_with(ssh_dir) {
        return None;
    }
    let (flag, wake) = (std::sync::Arc::clone(flag), ctx.clone());
    native_term_os::watch::FolderWatcher::start(&dir, false, move || {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
        wake.request_repaint();
    })
    .ok()
}

fn fingerprint(ssh_dir: &Path, tree: &SessionTree) -> Fingerprint {
    let mut files: Vec<PathBuf> = vec![ssh_dir.join("config")];
    files.extend(tree.folders().map(|f| f.file.clone()));
    if let Ok(entries) = std::fs::read_dir(folders_dir(ssh_dir)) {
        files.extend(entries.filter_map(Result::ok).map(|e| e.path()));
    }
    files.sort();
    files.dedup();
    files
        .into_iter()
        .map(|f| {
            let meta = std::fs::metadata(&f).ok();
            let modified = meta.as_ref().and_then(|m| m.modified().ok());
            let len = meta.map_or(0, |m| m.len());
            (f, modified, len)
        })
        .collect()
}

pub struct App {
    core: Option<Core>,
    /// Says this data directory is ours for as long as NativeTerm runs.
    _data_lock: Option<native_term_app::data_lock::DataLock>,
    /// Keeps the `~/.ssh` watcher alive.
    _watcher: Option<native_term_os::watch::FolderWatcher>,
    /// And the one on the folder files, when they live elsewhere.
    folders_watcher: Option<native_term_os::watch::FolderWatcher>,
    /// "Move session folders": the new path being typed.
    folders_move: Option<String>,
    ssh_changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    loaded_from: Fingerprint,
    page: Page,
    settings_page: SettingsPage,
    tab_list: TabList,
    agent: crate::agent::AgentCheck,
    storage: crate::storage::StorageCheck,
    /// For background checks that repaint when done.
    egui_ctx: egui::Context,
    /// How the data directory was chosen.
    data_source: &'static str,
    /// "Change data directory": the new path being typed.
    data_move: Option<String>,
    tree: SessionTree,
    /// Bumped on every reload.
    generation: u64,
    ssh_dir: PathBuf,
    data_dir: PathBuf,
    editor: Editor,
    view: TreeView,
    send_line: SendLine,
    /// The recent hosts, read from `state.db` at most every 2 s (the tree
    /// asks on every frame, and scrolling draws many).
    recent_cache: std::cell::RefCell<(Option<std::time::Instant>, Vec<String>)>,
    /// Tests: `NATIVETERM_OPEN_FILES=<alias>[,<alias>…]` opens those
    /// hosts' files once the tree is there.
    open_files: Option<String>,
    /// Hosts of folders marked "No group send", per tree generation.
    no_group_cache: std::cell::RefCell<(u64, std::collections::HashSet<String>)>,
    /// Hosts kept in tmux on the server, per tree generation.
    tmux_cache: std::cell::RefCell<(u64, std::collections::HashSet<String>)>,
    /// Keyboard shortcuts: in the window and global.
    keys: crate::shortcut_ui::ShortcutUi,
    dialog: Option<Dialog>,
    profile: ProfileSetup,
    show_settings: bool,
    notices: Vec<String>,
    /// Short messages in the corner: what an action just did.
    toasts: native_term_app::toast::Toasts,
    /// What was written about each host, by its `NativeTermId`
    /// (`notes.rs`); changes go to `state.db` and `notes.toml` at once.
    notes: std::collections::BTreeMap<String, native_term_app::registry::Note>,
    /// Bumped when a note changes, so the search stops using its cache.
    notes_generation: u64,
    /// The tags there are (`state.db`): what the chips above the tree
    /// offer, and what a host's tags are chosen among.
    tags: Vec<String>,
    /// The systems' pictures, as made for what is chosen so far.
    logos: std::cell::RefCell<crate::logos::Logos>,
    /// OneDrive / Dropbox folders on this computer, for the wizard.
    sync_roots: Vec<(String, PathBuf)>,
    /// PuTTY has saved sessions (checked at start).
    putty_sessions: bool,
    wizard: Option<crate::wizard::Wizard>,
    /// SecureCRT's configuration folder, if found (checked at start).
    securecrt: Option<PathBuf>,
}

/// The user's own `~/.ssh` is edited with ssh's defaults; any other
/// directory (tests, demos) with `-F` and absolute includes.
pub(crate) fn editor_for(ssh_dir: &Path, data_dir: &Path) -> Editor {
    let writer = Writer::new(data_dir.join("backups"));
    let program = native_term_session::ssh_program();
    let ssh = program.as_path();
    let home_ssh = native_term_os::home::ssh_dir();
    let editor =
        if home_ssh.as_deref().is_some_and(|h| h.to_string_lossy().eq_ignore_ascii_case(&ssh_dir.to_string_lossy())) {
            Editor::new(ssh_dir, writer, ssh)
        } else {
            Editor::for_directory(ssh_dir, writer, ssh)
        };
    let dir = folders_dir(ssh_dir);
    if dir == ssh_dir.join("config.d") {
        editor
    } else {
        editor.with_folders_dir(&dir)
    }
}

/// What the terminal is called when no core says: the platform's own.
fn default_terminal_name() -> String {
    if cfg!(windows) {
        "Windows Terminal".into()
    } else {
        t!("terminal-none")
    }
}

/// Whether PuTTY has saved sessions to import (its registry key).
fn putty_has_sessions() -> bool {
    #[cfg(windows)]
    {
        native_term_config::putty::has_sessions()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

impl App {
    pub fn new(ctx: &egui::Context, setup: Setup) -> App {
        #[cfg(windows)]
        let install = setup.install.clone();
        let Setup { options, _lock, shim, core, data_dir, data_source, mut notices, .. } = setup;
        #[cfg(windows)]
        let mut profile = ProfileSetup::new(install, shim.clone(), data_dir.join("backups"));
        #[cfg(not(windows))]
        let mut profile = ProfileSetup::new(shim.clone(), data_dir.join("backups"));
        if let Some(core) = &core {
            core.set_audit_dir(data_dir.join("audit"));
            core.set_data_dir(data_dir.to_path_buf());
        }
        notices.extend(profile.fix_moved());
        // the program folder may have moved: put our path right in what we
        // wrote (the tab profile above, the ssh config here)
        let repaired = editor_for(&options.ssh_dir, &data_dir).repair_paths(&shim);
        if !repaired.is_empty() {
            notices.push(t!(
                "config-paths-repaired",
                count = repaired.lines,
                old = repaired.was.first().map(|p| p.display().to_string()).unwrap_or_default()
            ));
        }
        if let Some(core) = &core {
            crate::dock::set_pinned(core.setting(PINNED_SETTING).as_deref() == Some("1"));
            native_term_os::appearance::refresh();
            apply_theme(ctx, core.setting(THEME_SETTING).as_deref());
            crate::looks::Preset::from_setting(core.setting(crate::looks::SETTING).as_deref()).choose(ctx);
            sync_terminal_look(core);
            watch_appearance(ctx.clone());
            let repaint = ctx.clone();
            let woken = ctx.clone();
            native_term_app::toast::wake_with(move || woken.request_repaint());
            let ctx = ctx.clone();
            core.set_repaint(move || ctx.request_repaint());
            let ask = move |request| {
                crate::shell::ask(request);
                repaint.request_repaint();
            };
            if let Err(e) = core.start_tab_menu(ask) {
                notices.push(t!("notice-tab-menu-unavailable", error = e.to_string()));
            }
        }
        if let Some(dir) = core.as_ref().and_then(|c| c.setting(FOLDERS_SETTING)).filter(|d| !d.is_empty()) {
            let dir = PathBuf::from(dir);
            if !dir.is_dir() {
                notices.push(t!("folders-missing", path = dir.display().to_string()));
            }
            set_folders_dir(Some(dir));
        }
        let tree = SessionTree::load(&options.ssh_dir);
        publish_hosts(&tree, core.as_ref());
        // changes made elsewhere (an editor, a sync tool) show up by themselves
        let ssh_changed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&ssh_changed);
        let wake = ctx.clone();
        let watcher = native_term_os::watch::FolderWatcher::start(&options.ssh_dir, true, move || {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
            wake.request_repaint();
        })
        .ok();
        let folders_watcher = watch_folders(&options.ssh_dir, &ssh_changed, ctx);
        let loaded_from = fingerprint(&options.ssh_dir, &tree);
        // the notes live in state.db and travel in notes.toml
        let (notes, note_problem) = match core.as_ref().and_then(Core::registry) {
            Some(registry) => native_term_app::notes::open(registry, &data_dir),
            None => (Default::default(), None),
        };
        if let Some(problem) = note_problem {
            notices.push(t!("notice-notes", error = problem));
        }
        let tags = core.as_ref().and_then(Core::registry).and_then(|r| r.tags().ok()).unwrap_or_default();
        let checks = core.as_ref().and_then(|c| c.setting(CHECKS_SETTING)).as_deref() == Some("on");
        crate::dialogs::set_known_tags(&tags);
        let first_run = core.as_ref().is_some_and(|c| c.setting(crate::wizard::DONE_SETTING).is_none());
        let keys = crate::shortcut_ui::ShortcutUi::new(ctx, core.as_ref(), &profile.settings_json());
        App {
            core,
            _data_lock: _lock,
            _watcher: watcher,
            folders_watcher,
            folders_move: None,
            ssh_changed,
            loaded_from,
            page: Page::Tree,
            settings_page: SettingsPage::General,
            tab_list: TabList::default(),
            agent: {
                let check = crate::agent::AgentCheck::default();
                check.refresh(&options.ssh_dir, ctx);
                check
            },
            storage: {
                let check = crate::storage::StorageCheck::default();
                check.refresh(&options.ssh_dir, &data_dir, ctx);
                check
            },
            egui_ctx: ctx.clone(),
            data_source,
            data_move: None,
            tree,
            generation: 0,
            editor: editor_for(&options.ssh_dir, &data_dir),
            data_dir,
            ssh_dir: options.ssh_dir,
            view: TreeView::with_checks(checks),
            send_line: SendLine::default(),
            recent_cache: std::cell::RefCell::new((None, Vec::new())),
            open_files: std::env::var("NATIVETERM_OPEN_FILES").ok().filter(|a| !a.is_empty()),
            no_group_cache: std::cell::RefCell::new((u64::MAX, std::collections::HashSet::new())),
            tmux_cache: std::cell::RefCell::new((u64::MAX, std::collections::HashSet::new())),
            keys,
            dialog: None,
            profile,
            show_settings: false,
            notices,
            toasts: Default::default(),
            notes,
            notes_generation: 0,
            tags,
            logos: Default::default(),
            sync_roots: native_term_os::cloud::sync_roots(),
            putty_sessions: putty_has_sessions(),
            wizard: first_run.then(|| crate::wizard::Wizard::new(ctx)),
            securecrt: native_term_app::import::securecrt_config_path(),
        }
    }

    /// The favorite hosts as Windows Terminal profiles, when the user
    /// asked for that; nothing otherwise.
    #[cfg(windows)]
    fn terminal_favorites(&self) -> Vec<native_term_platform::windows_terminal::profile::Favorite> {
        let on = self.core.as_ref().and_then(|c| c.setting(crate::terminal_profile::FAVORITES_SETTING)).as_deref()
            == Some("1");
        if !on {
            return Vec::new();
        }
        self.tree
            .folders()
            .flat_map(|folder| folder.hosts.iter().map(move |host| (folder, host)))
            .filter(|(_, host)| host.favorite())
            .filter_map(|(folder, host)| {
                let look = native_term_config::appearance::for_host(folder, host);
                Some(native_term_platform::windows_terminal::profile::Favorite {
                    id: host.id()?.to_string(),
                    alias: host.alias().to_string(),
                    label: host.label().to_string(),
                    tab_color: look.tab_color,
                    color_scheme: look.color_scheme,
                })
            })
            .collect()
    }

    /// Write the favorites into the fragment when they changed.
    #[cfg(windows)]
    fn refresh_terminal_favorites(&mut self) {
        let favorites = self.terminal_favorites();
        if let Some(problem) = self.profile.set_favorites(favorites) {
            self.notices.push(problem);
        }
    }

    /// No fragment to write them into here.
    #[cfg(not(windows))]
    fn refresh_terminal_favorites(&mut self) {}

    fn reload(&mut self) {
        self.tree = SessionTree::load(&self.ssh_dir);
        self.generation += 1;
        self.loaded_from = fingerprint(&self.ssh_dir, &self.tree);
        publish_hosts(&self.tree, self.core.as_ref());
        self.refresh_terminal_favorites();
        self.storage.refresh(&self.ssh_dir, &self.data_dir, &self.egui_ctx);
    }

    /// Reload if a config file really changed (ssh itself writes
    /// `known_hosts` in the same folder).
    fn reload_if_changed(&mut self) {
        if !self.ssh_changed.swap(false, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        if fingerprint(&self.ssh_dir, &self.tree) != self.loaded_from {
            self.reload();
        }
    }

    fn recent(&self) -> Vec<String> {
        const FRESH: std::time::Duration = std::time::Duration::from_secs(2);
        let mut cache = self.recent_cache.borrow_mut();
        if cache.0.is_none_or(|at| at.elapsed() >= FRESH) {
            let list = self
                .core
                .as_ref()
                .and_then(|c| c.registry())
                .and_then(|r| r.recent(20).ok())
                .map(|list| list.into_iter().map(|u| u.alias).collect())
                .unwrap_or_default();
            *cache = (Some(std::time::Instant::now()), list);
        }
        cache.1.clone()
    }

    /// A serial line can be held by one session only: requests for a line
    /// an open session already uses are dropped with a notice naming it.
    fn without_taken_ports(&mut self, hosts: Vec<native_term_app::HostRequest>) -> Vec<native_term_app::HostRequest> {
        let line_of = |tree: &SessionTree, alias: &str| {
            let (_, host) = tree.find(alias)?;
            let serial = host.plink.as_ref()?.serial.as_ref()?;
            Some(serial.line.to_uppercase())
        };
        let open: Vec<SessionView> = self
            .core
            .as_ref()
            .map(|c| c.sessions().into_iter().filter(|s| s.state.is_open()).collect())
            .unwrap_or_default();
        let mut taken: HashMap<String, String> =
            open.iter().filter_map(|s| line_of(&self.tree, &s.alias).map(|line| (line, s.label.clone()))).collect();
        let mut kept = Vec::new();
        for request in hosts {
            match line_of(&self.tree, &request.alias) {
                Some(line) => match taken.get(&line) {
                    Some(owner) => {
                        self.notices.push(t!("notice-port-taken", line = line.as_str(), label = owner.as_str()))
                    }
                    None => {
                        taken.insert(line, request.label.clone());
                        kept.push(request);
                    }
                },
                None => kept.push(request),
            }
        }
        kept
    }

    fn folder_label(&self, file: &Path) -> String {
        self.tree
            .folders()
            .find(|f| f.file == file)
            .map(|f| if f.name.is_empty() { t!("tree-main-config") } else { f.label().to_string() })
            .unwrap_or_else(|| file.display().to_string())
    }

    /// Hosts in folders marked "No group send".
    /// A shortcut's command; `global`: pressed while another program was in
    /// front (NativeTerm's window comes out first where the command shows
    /// something in it).
    fn run_shortcut(&mut self, command: native_term_app::shortcuts::Command, global: bool) {
        use native_term_app::shortcuts::Command;
        let active = || self.core.as_ref().and_then(Core::active_session);
        match command {
            Command::ShowNativeTerm | Command::SearchHosts => {
                if global || command == Command::ShowNativeTerm {
                    crate::shell::show_main();
                }
                // (the search field is the tree's and the recent hosts')
                if !matches!(self.page, Page::Tree | Page::Recent) {
                    self.page = Page::Tree;
                }
                self.view.focus_search();
            }
            Command::AllTabs => {
                if global {
                    crate::shell::show_tabs();
                }
                self.page = Page::Tabs;
                self.tab_list.focus_search();
            }
            Command::SendToActive | Command::SendToSeveral => {
                let Some(core) = self.core.clone() else { return };
                if self.dialog.is_some() {
                    self.notices.push(t!("notice-dialog-open"));
                    return;
                }
                let chosen = match command {
                    Command::SendToActive => match active() {
                        Some(s) => vec![s.id],
                        None => {
                            self.notices.push(t!("keys-no-active"));
                            return;
                        }
                    },
                    _ => Vec::new(),
                };
                crate::shell::show_main();
                self.dialog = Some(Dialog::Send(Box::new(self.send_dialog(&core, &chosen))));
            }
            Command::FilesForActive => match active() {
                Some(s) => self.open_files(&s.alias, Some(&s.id)),
                None => self.notices.push(t!("keys-no-active")),
            },
        }
    }

    /// The send dialog, with the hosts kept in tmux (their sessions can be
    /// sent to through tmux on the server when not logged in).
    fn send_dialog(&self, core: &Core, chosen: &[String]) -> SendDialog {
        let route = crate::send_dialog::TmuxRoute {
            ssh: self.editor.ssh().to_path_buf(),
            config: self.editor.config().map(Path::to_path_buf),
            hosts: self.tmux_hosts(),
        };
        SendDialog::new(core, chosen, &self.data_dir, &self.no_group_send()).with_tmux(route, chosen)
    }

    /// The hosts (aliases) whose sessions run in tmux on the server.
    fn tmux_hosts(&self) -> std::collections::HashSet<String> {
        use native_term_config::persistent::{for_host, Persistence};
        let mut cache = self.tmux_cache.borrow_mut();
        if cache.0 != self.generation {
            let hosts = self
                .tree
                .hosts()
                .filter(|(f, h)| h.plink.is_none() && for_host(f, h) == Some(Persistence::Tmux))
                .map(|(_, h)| h.alias().to_string())
                .collect();
            *cache = (self.generation, hosts);
        }
        cache.1.clone()
    }

    fn no_group_send(&self) -> std::collections::HashSet<String> {
        let mut cache = self.no_group_cache.borrow_mut();
        if cache.0 != self.generation {
            let hosts =
                self.tree.hosts().filter(|(f, _)| f.no_group_send()).map(|(_, h)| h.alias().to_string()).collect();
            *cache = (self.generation, hosts);
        }
        cache.1.clone()
    }

    /// A folder's tab color and color scheme defaults.
    fn folder_look(&self, file: &Path) -> (Option<String>, Option<String>) {
        use native_term_config::appearance::{COLOR_SCHEME, TAB_COLOR};
        let Some(folder) = self.tree.folders().find(|f| f.file == file) else { return (None, None) };
        (folder.defaults.get(TAB_COLOR).map(str::to_string), folder.defaults.get(COLOR_SCHEME).map(str::to_string))
    }

    /// A folder's `NativeTermCredential` default.
    fn folder_credential(&self, file: &Path) -> Option<String> {
        let folder = self.tree.folders().find(|f| f.file == file)?;
        folder.defaults.get(native_term_config::password::KEY).map(str::to_string)
    }

    /// A folder's `NativeTermPersistent` default.
    fn folder_persistent(&self, file: &Path) -> Option<String> {
        let folder = self.tree.folders().find(|f| f.file == file)?;
        folder.defaults.get(native_term_config::persistent::KEY).map(str::to_string)
    }

    /// Sessions of the last run whose tabs never turned up: Terminal was
    /// restarted without them, or it restored them from a program folder
    /// that has moved, in which case those tabs could not start at all.
    /// Opening them again is one click.
    fn lost_banner(&mut self, ui: &mut egui::Ui) {
        let Some(core) = self.core.clone() else { return };
        let lost = core.lost_at_start();
        if lost.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            let text = match self.profile.moved {
                true => t!("lost-banner-moved", count = lost.len()),
                false => t!("lost-banner", count = lost.len()),
            };
            ui.colored_label(egui::Color32::from_rgb(0xd0, 0x9a, 0x1a), text);
            if ui.button(t!("lost-reopen")).clicked() {
                self.handle(TreeAction::Open(lost, native_term_platform::Target::Recent));
                core.forget_lost(true);
            }
            if ui.small_button(t!("lost-ignore")).clicked() {
                core.forget_lost(false);
            }
        });
    }

    fn handle(&mut self, action: TreeAction) {
        match action {
            TreeAction::Open(hosts, target) => {
                let hosts = self.without_taken_ports(hosts);
                if let (Some(core), false) = (&self.core, hosts.is_empty()) {
                    core.open(&hosts, target);
                    // only ssh hosts can forward the agent
                    let ssh: Vec<String> = hosts
                        .iter()
                        .filter(|h| self.tree.find(&h.alias).is_none_or(|(_, e)| e.plink.is_none()))
                        .map(|h| h.alias.clone())
                        .collect();
                    if ssh.len() >= AGENT_NOTICE_MIN {
                        let aliases = ssh;
                        let (checker, core) = (self.editor.checker(), core.clone());
                        std::thread::spawn(move || {
                            let forwarding = checker.forwarding_agent(&aliases);
                            if forwarding.len() >= AGENT_NOTICE_MIN {
                                let names: Vec<&str> = forwarding.iter().map(String::as_str).take(5).collect();
                                let text = t!(
                                    "notice-agent-forwarding",
                                    count = forwarding.len(),
                                    total = aliases.len(),
                                    names = names.join(", ")
                                );
                                core.add_notice(text);
                            }
                        });
                    }
                }
            }
            TreeAction::Reload => self.reload(),
            TreeAction::Checks(on) => {
                if let Some(core) = &self.core {
                    core.set_setting(CHECKS_SETTING, if on { "on" } else { "off" });
                }
            }
            TreeAction::RenameTag(tag) => {
                let hosts = self.notes.values().filter(|note| note.has_tag(&tag)).count();
                self.dialog = Some(Dialog::Tag(TagDialog::rename(&tag, hosts)));
            }
            TreeAction::DeleteTag(tag) => {
                let hosts = self.notes.values().filter(|note| note.has_tag(&tag)).count();
                self.dialog = Some(Dialog::Tag(TagDialog::delete(&tag, hosts)));
            }
            TreeAction::NewFolder => self.dialog = Some(Dialog::Folder(FolderDialog::new_folder())),
            TreeAction::RenameFolder(file) => {
                let label = self.folder_label(&file);
                self.dialog = Some(Dialog::Folder(FolderDialog::rename(file, &label)));
            }
            TreeAction::NewHost(file) => {
                let label = self.folder_label(&file);
                let folder = self.folder_persistent(&file);
                let (color, scheme) = self.folder_look(&file);
                let set = self.folder_credential(&file);
                let dialog = HostDialog::new_host(file, &label)
                    .with_folder_default(folder)
                    .with_folder_look(color, scheme)
                    .with_credentials(set, crate::credential_sets::names());
                self.dialog = Some(Dialog::Host(Box::new(dialog)));
            }
            TreeAction::NewPlink(file) => {
                let label = self.folder_label(&file);
                let folder_log = self.tree.folders().find(|f| f.file == file).and_then(LogSettings::of_folder);
                let dialog = PlinkDialog::new_session(file, &label).with_log(None, folder_log, &self.data_dir);
                self.dialog = Some(Dialog::Plink(Box::new(dialog)));
            }
            TreeAction::Edit(alias) => {
                if let Some((tree_folder, host)) = self.tree.find(&alias) {
                    self.dialog = Some(match &host.plink {
                        Some(session) => Dialog::Plink(Box::new(
                            PlinkDialog::edit(session)
                                .with_log(LogSettings::own(host), LogSettings::of_folder(tree_folder), &self.data_dir)
                                .with_note(self.note_of(host)),
                        )),
                        None => {
                            let folder = self.folder_persistent(&host.file);
                            let account = self
                                .editor
                                .effective(&alias)
                                .ok()
                                .and_then(|e| native_term_config::password::target(&e, None));
                            let (color, scheme) = self.folder_look(&host.file);
                            let set = self.folder_credential(&host.file);
                            let said =
                                self.core.as_ref().and_then(|c| c.servers().get(&alias).and_then(|known| known.os));
                            let dialog = HostDialog::edit(&alias, &HostDraft::from_host(host))
                                .with_system_said(said)
                                .with_folder_default(folder)
                                .with_folder_look(color, scheme)
                                .with_credentials(set, crate::credential_sets::names())
                                .with_note(self.note_of(host))
                                .with_password(account);
                            Dialog::Host(Box::new(dialog))
                        }
                    });
                }
            }
            TreeAction::Options(alias) => {
                if let Some((folder, host)) = self.tree.find(&alias) {
                    match self.editor.host_options(host) {
                        Ok(values) => {
                            let effective = self.editor.effective(&alias).unwrap_or_default();
                            let target = OptionsTarget::Host(alias.clone());
                            let log = crate::log_page::LogPage::for_session(
                                LogSettings::own(host),
                                LogSettings::of_folder(folder),
                                &self.data_dir,
                            );
                            let dialog =
                                OptionsDialog::new(target, host.label(), &values, effective, self.editor.ssh())
                                    .with_log(log);
                            self.dialog = Some(Dialog::Options(Box::new(dialog)));
                        }
                        Err(e) => self.notices.push(e.to_string()),
                    }
                }
            }
            TreeAction::FolderOptions(file) => {
                if !self.editor.folder_options_supported() {
                    self.notices.push(t!("folder-options-unsupported"));
                    return;
                }
                match self.editor.folder_options(&file) {
                    Ok(values) => {
                        // what the folder's hosts get now, from its first host
                        let first = self
                            .tree
                            .folders()
                            .find(|f| f.file == file)
                            .and_then(|f| f.hosts.iter().find(|h| h.plink.is_none()));
                        let effective = first.and_then(|h| self.editor.effective(h.alias()).ok()).unwrap_or_default();
                        let label = self.folder_label(&file);
                        let log = crate::log_page::LogPage::for_folder(
                            self.tree.folders().find(|f| f.file == file).and_then(LogSettings::of_folder),
                            &self.data_dir,
                        );
                        let target = OptionsTarget::Folder(file);
                        let dialog =
                            OptionsDialog::new(target, &label, &values, effective, self.editor.ssh()).with_log(log);
                        self.dialog = Some(Dialog::Options(Box::new(dialog)));
                    }
                    Err(e) => self.notices.push(e.to_string()),
                }
            }
            TreeAction::Files(alias) => self.open_files(&alias, None),
            TreeAction::ServerSessions(alias) => {
                if let Some((folder, host)) = self.tree.find(&alias) {
                    let on_login = folder.nt(host, "onlogin").map(str::to_string);
                    let ssh = self.editor.ssh().to_path_buf();
                    let config = self.editor.config().map(Path::to_path_buf);
                    let request = native_term_app::HostRequest {
                        on_login,
                        ..native_term_app::HostRequest::new(&alias, host.label())
                    };
                    let data_dir = self.data_dir.clone();
                    let dialog =
                        ServerSessionsDialog::new(&self.egui_ctx, host.label(), vec![request], ssh, config, data_dir);
                    self.dialog = Some(Dialog::ServerSessions(Box::new(dialog)));
                }
            }
            TreeAction::FolderServerSessions(name, hosts) => {
                let ssh = self.editor.ssh().to_path_buf();
                let config = self.editor.config().map(Path::to_path_buf);
                let data_dir = self.data_dir.clone();
                let dialog = ServerSessionsDialog::new(&self.egui_ctx, &name, hosts, ssh, config, data_dir);
                self.dialog = Some(Dialog::ServerSessions(Box::new(dialog)));
            }
            TreeAction::FolderTabColor(file, value) => {
                if let Err(e) = self.editor.set_folder_tab_color(&file, value.as_deref()) {
                    self.notices.push(e.to_string());
                }
                self.reload();
            }
            TreeAction::FolderColorScheme(file, value) => {
                if let Err(e) = self.editor.set_folder_color_scheme(&file, value.as_deref()) {
                    self.notices.push(e.to_string());
                }
                self.reload();
            }
            TreeAction::FolderNoGroupSend(file, on) => {
                if let Err(e) = self.editor.set_folder_no_group_send(&file, on) {
                    self.notices.push(e.to_string());
                }
                self.reload();
            }
            TreeAction::FolderCredential(file, value) => {
                if let Err(e) = self.editor.set_folder_credential(&file, value.as_deref()) {
                    self.notices.push(e.to_string());
                }
                self.reload();
            }
            TreeAction::CredentialSets => {
                if self.dialog.is_none() {
                    let dialog = crate::credential_sets::CredentialSetsDialog::new(&self.tree);
                    self.dialog = Some(Dialog::CredentialSets(Box::new(dialog)));
                }
            }
            TreeAction::FolderPersistent(file, value) => {
                if let Err(e) = self.editor.set_folder_persistent(&file, value.as_deref()) {
                    self.notices.push(e.to_string());
                }
                self.reload();
            }
            TreeAction::Favorite(alias, on) => {
                let result = match self.tree.find(&alias) {
                    Some((_, host)) => self.editor.set_favorite(host, on).map_err(|e| e.to_string()),
                    None => Err(t!("error-host-gone", alias = alias.as_str())),
                };
                if let Err(e) = result {
                    self.notices.push(e);
                }
                self.reload();
            }
            TreeAction::InstallKey(hosts) => {
                // keys are for ssh hosts only
                let hosts: Vec<(String, String)> = hosts
                    .into_iter()
                    .filter(|(alias, _)| self.tree.find(alias).is_none_or(|(_, h)| h.plink.is_none()))
                    .collect();
                if !hosts.is_empty() {
                    self.dialog = Some(Dialog::Key(Box::new(KeyDialog::new(hosts, &self.ssh_dir))));
                }
            }
            TreeAction::ForgetKey(alias) => {
                if let Some((_, host)) = self.tree.find(&alias) {
                    let names = Editor::host_key_names(host);
                    self.dialog = Some(Dialog::Forget(ConfirmForget::new(&alias, names)));
                }
            }
            TreeAction::Delete(alias) => {
                if let Some((_, host)) = self.tree.find(&alias) {
                    self.dialog = Some(Dialog::Delete(ConfirmDelete::new(&alias, host.label())));
                }
            }
            TreeAction::Move(alias, to) => {
                let result = match self.tree.find(&alias) {
                    Some((_, host)) => self.editor.move_host(host, &to).map_err(|e| e.to_string()),
                    None => Err(t!("error-host-gone", alias = alias.as_str())),
                };
                if let Err(e) = result {
                    self.notices.push(t!("notice-move-failed", alias = alias.as_str(), error = e));
                }
                self.reload();
            }
        }
    }

    /// Where the folder files are, and moving them (e.g. into a synced folder).
    fn folders_ui(&mut self, ui: &mut egui::Ui) {
        let current = self.editor.folders_dir();
        ui.horizontal_wrapped(|ui| {
            ui.label(t!("folders-current", path = current.display().to_string()));
            if ui.small_button(t!("wizard-open-folder")).clicked() {
                let _ = native_term_os::shell::open_folder(&current);
            }
        });
        let Some(path) = self.folders_move.as_mut() else {
            if ui.button(t!("folders-change")).clicked() {
                self.folders_move = Some(String::new());
            }
            return;
        };
        let mut chosen = None;
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(path).hint_text(r"D:\OneDrive\ssh-folders").desired_width(320.0));
            if ui.add_enabled(!path.trim().is_empty(), egui::Button::new(t!("folders-move"))).clicked() {
                chosen = Some(PathBuf::from(path.trim()));
            }
            cancel = ui.button(t!("button-cancel")).clicked();
        });
        ui.weak(t!("folders-move-note"));
        if let Some(new) = chosen {
            // a folder with sessions in it already (synced from another
            // computer) is used as it is, never copied onto
            let done = if native_term_config::ops::holds_folders(&new) {
                self.adopt_folders_at(new)
            } else {
                self.move_folders_to(new)
            };
            if done {
                self.folders_move = None;
            }
        } else if cancel {
            self.folders_move = None;
        }
    }

    /// The session folders are at `new` now: a per-machine setting, and
    /// the tree follows them there.
    fn folders_now_at(&mut self, new: &Path) {
        if let Some(core) = &self.core {
            core.set_setting(FOLDERS_SETTING, &new.display().to_string());
        }
        set_folders_dir(Some(new.to_path_buf()));
        self.folders_watcher = watch_folders(&self.ssh_dir, &self.ssh_changed, &self.egui_ctx);
        self.reload();
    }

    /// Copy the session folders to `new` and use them there (Settings and
    /// the wizard's sync step); says how it went. `true` when it worked.
    fn move_folders_to(&mut self, new: PathBuf) -> bool {
        let current = self.editor.folders_dir();
        match self.editor.move_folders(&new) {
            Ok(count) => {
                self.folders_now_at(&new);
                let (path, old) = (new.display().to_string(), current.display().to_string());
                self.notices.push(t!("folders-moved", count = count, path = path, old = old));
                true
            }
            Err(e) => {
                self.notices.push(t!("folders-move-failed", error = e.to_string()));
                false
            }
        }
    }

    /// Use the session folders already in `new` (synced from another
    /// computer); this computer's own stay where they were.
    fn adopt_folders_at(&mut self, new: PathBuf) -> bool {
        let current = self.editor.folders_dir();
        match self.editor.adopt_folders(&new) {
            Ok(count) => {
                self.folders_now_at(&new);
                let (path, old) = (new.display().to_string(), current.display().to_string());
                self.notices.push(t!("folders-adopted", count = count, path = path, old = old));
                true
            }
            Err(e) => {
                self.notices.push(t!("folders-move-failed", error = e.to_string()));
                false
            }
        }
    }

    fn data_dir_ui(&mut self, ui: &mut egui::Ui) {
        use native_term_app::data_dir::{self, Pointer};
        let pointer = data_dir::pointer_for(self.data_source);
        ui.horizontal_wrapped(|ui| {
            ui.label(t!("data-dir-current", path = self.data_dir.display().to_string(), source = self.data_source));
            if ui.small_button(t!("wizard-open-folder")).clicked() {
                let _ = native_term_os::shell::open_folder(&self.data_dir);
            }
        });
        if pointer == Pointer::Fixed {
            ui.weak(t!("data-dir-fixed"));
            return;
        }
        let Some(path) = self.data_move.as_mut() else {
            if ui.button(t!("data-dir-change")).clicked() {
                self.data_move = Some(String::new());
            }
            return;
        };
        let mut done = false;
        let mut chosen = None;
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(path).hint_text(r"D:\Sync\NativeTerm").desired_width(320.0));
            if ui.button(t!("data-dir-move")).clicked() {
                chosen = Some(PathBuf::from(path.trim()));
            }
            if ui.button(t!("button-cancel")).clicked() {
                done = true;
            }
        });
        ui.weak(t!("data-dir-move-note"));
        if let Some(new) = chosen {
            done = self.move_data(&new);
        }
        if done {
            self.data_move = None;
        }
    }

    /// Copy the data to `new` and point the next start at it; says how it
    /// went in a notice. `true` when it worked.
    fn move_data(&mut self, new: &Path) -> bool {
        use native_term_app::data_dir;
        let pointer = data_dir::pointer_for(self.data_source);
        let result = data_dir::check_target(&self.data_dir, new).and_then(|()| {
            let core = self.core.as_ref().ok_or_else(|| "state.db isn't open".to_string())?;
            let copied =
                data_dir::copy_data(&self.data_dir, new, |db| core.copy_state_to(db)).map_err(|e| e.to_string())?;
            let program_dir = std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(Path::to_path_buf))
                .ok_or_else(|| "the program folder isn't known".to_string())?;
            data_dir::set_pointer(pointer, &program_dir, new).map_err(|e| e.to_string())?;
            Ok(copied)
        });
        match result {
            Ok(copied) => {
                self.notices.push(t!(
                    "data-dir-moved",
                    count = copied,
                    path = new.display().to_string(),
                    old = self.data_dir.display().to_string()
                ));
                true
            }
            Err(e) => {
                self.notices.push(t!("data-dir-move-failed", error = e));
                false
            }
        }
    }

    /// A host's files (SFTP) in the files window; from a terminal tab
    /// (`session`) kept in tmux, the server's side starts in the tab's
    /// folder.
    fn open_files(&mut self, alias: &str, session: Option<&String>) {
        let Some((folder, host)) = self.tree.find(alias) else {
            self.notices.push(t!("notice-not-saved", alias = alias));
            return;
        };
        if host.plink.is_some() {
            self.notices.push(t!("notice-files-ssh-only", label = host.label()));
            return;
        }
        // `NativeTermFileEncoding` (host or folder): how the server names files
        let names = folder.nt(host, "fileencoding").and_then(native_term_sftp::Names::from_label).unwrap_or_default();
        let tmux = native_term_config::persistent::for_host(folder, host)
            == Some(native_term_config::persistent::Persistence::Tmux);
        let tmux_session = session.filter(|_| tmux).map(|id| native_term_config::persistent::session_name(alias, id));
        crate::files_window::open(crate::files_window::Spec {
            alias: alias.to_string(),
            label: host.label().to_string(),
            ssh: self.editor.ssh().to_path_buf(),
            config: self.editor.config().map(Path::to_path_buf),
            names,
            shim: native_term_app::default_shim_path().unwrap_or_default(),
            tmux_session,
            memory: self.core.clone(),
            credential: native_term_config::password::set_for_host(folder, host).map(str::to_string),
        });
    }

    /// Files dropped into a tab: upload them, or send the text Terminal
    /// pasted (the client held it back). The answer can be remembered,
    /// and then this happens without asking.
    /// `certain`: Windows told us this was a file drop (our own window),
    /// so it needs no guessing.
    fn dropped(&mut self, alias: &str, session: &str, paths: Vec<PathBuf>, text: &str, certain: bool) {
        let can_upload = self.tree.find(alias).is_some_and(|(_, host)| host.plink.is_none());
        // A drop on a Terminal tab ends with the mouse button released
        // over it, having gone down in another window (Explorer). Text
        // that arrives without that was typed or pasted, and is never
        // acted on without asking, however the question was answered
        // before.
        let dropped_by_mouse = certain
            || self
                .core
                .as_ref()
                .and_then(|c| c.tab_menu_since_drag_release())
                .is_some_and(|since| since < std::time::Duration::from_millis(2000));
        match self.remembered_drop().filter(|_| dropped_by_mouse) {
            Some(DropChoice::Upload) if can_upload => {
                self.upload_dropped(alias, session, &paths);
                return;
            }
            Some(DropChoice::Text) => {
                self.send_dropped_text(session, text);
                return;
            }
            _ => {}
        }
        if self.dialog.is_some() {
            // a dialog is already up: the text goes to the session, which
            // is what would have happened without us
            self.send_dropped_text(session, text);
            return;
        }
        let label = self
            .core
            .as_ref()
            .and_then(|c| c.sessions().into_iter().find(|s| s.id == session).map(|s| s.label))
            .unwrap_or_else(|| alias.to_string());
        self.dialog = Some(Dialog::Drop(Box::new(DropDialog {
            alias: alias.to_string(),
            session: session.to_string(),
            label,
            paths,
            text: text.to_string(),
            remember: false,
            can_upload,
        })));
    }

    /// The remembered answer for dropped files, if there is one.
    fn remembered_drop(&self) -> Option<DropChoice> {
        match self.core.as_ref()?.setting("drop.action")?.as_str() {
            "upload" => Some(DropChoice::Upload),
            "text" => Some(DropChoice::Text),
            _ => None,
        }
    }

    /// Uploads dropped files to the session's folder (the files window,
    /// which starts at the tab's folder for a tmux session).
    fn upload_dropped(&mut self, alias: &str, session: &str, paths: &[PathBuf]) {
        self.open_files(alias, Some(&session.to_string()));
        crate::files_window::upload_into(alias, paths);
    }

    /// Sends the text Terminal pasted to the session after all. It goes
    /// with the marker the client strips (`nt_drop.c`), so that the names
    /// are not read as another drop.
    fn send_dropped_text(&mut self, session: &str, text: &str) {
        if let Some(core) = &self.core {
            let marked = format!("\u{1b}_nt\u{1b}\\{text}");
            let report = core.send_text(&[session.to_string()], &marked, false);
            if !report.failed.is_empty() || !report.skipped.is_empty() {
                self.notices.push(t!("notice-drop-not-sent"));
            }
        }
    }

    /// What the tab menu asked for: it has no dialogs of its own.
    fn handle_menu_request(&mut self, request: native_term_app::tab_menu::MenuRequest) {
        use native_term_app::tab_menu::MenuRequest;
        // the files window is a window of its own: no dialog in the way
        if let MenuRequest::Files { alias, session } = &request {
            self.open_files(alias, Some(session));
            return;
        }
        if let MenuRequest::Dropped { alias, session, paths, text } = request {
            self.dropped(&alias, &session, paths, &text, false);
            return;
        }
        if let MenuRequest::Find(question) = request {
            match &self.core {
                Some(core) => crate::find_window::asked(question, core.clone()),
                None => native_term_app::find::answer(question.ticket, None),
            }
            return;
        }
        if let MenuRequest::PasteQuotation(ticket) = request {
            // a window of its own too, whatever dialog is open here
            match &self.core {
                Some(core) => crate::quotation_window::open(ticket, core.clone()),
                None => native_term_app::quotation::answer(ticket, None),
            }
            return;
        }
        if let MenuRequest::TabTitle { ticket, current } = request {
            crate::tab_title_window::open(ticket, current);
            return;
        }
        if let MenuRequest::SetUser { alias, user } = request {
            // (whatever dialog is open: nothing of it is asked)
            self.set_user(&alias, &user);
            return;
        }
        if let MenuRequest::Password { ticket, question } = request {
            // a window of its own too, whatever dialog is open here
            crate::password_window::open(ticket, question);
            return;
        }
        if self.dialog.is_some() {
            self.notices.push(t!("notice-dialog-open"));
            return;
        }
        match request {
            MenuRequest::Send(id) => {
                if let Some(core) = &self.core {
                    self.dialog = Some(Dialog::Send(Box::new(self.send_dialog(core, &[id]))));
                }
            }
            MenuRequest::SendAny => {
                if let Some(core) = &self.core {
                    self.dialog = Some(Dialog::Send(Box::new(self.send_dialog(core, &[]))));
                }
            }
            MenuRequest::Rename(alias) => match self.tree.find(&alias) {
                Some((_, host)) => {
                    let folder = self.folder_persistent(&host.file);
                    let said = self.core.as_ref().and_then(|c| c.servers().get(&alias).and_then(|known| known.os));
                    let dialog = HostDialog::edit(&alias, &HostDraft::from_host(host))
                        .with_system_said(said)
                        .with_folder_default(folder)
                        .with_note(self.note_of(host));
                    self.dialog = Some(Dialog::Host(Box::new(dialog)));
                }
                None => self.notices.push(t!("notice-not-saved", alias = alias.as_str())),
            },
            // handled above
            MenuRequest::Files { .. }
            | MenuRequest::Dropped { .. }
            | MenuRequest::PasteQuotation(_)
            | MenuRequest::TabTitle { .. }
            | MenuRequest::Password { .. }
            | MenuRequest::SetUser { .. }
            | MenuRequest::Find(_) => {}
            MenuRequest::ConfirmClose(ids) => {
                let labels = self
                    .core
                    .as_ref()
                    .map(|c| {
                        c.sessions()
                            .into_iter()
                            .filter(|s| ids.contains(&s.id))
                            .map(|s| (s.label, s.location.is_some_and(|l| l.mixed)))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.dialog = Some(Dialog::CloseMixed(ConfirmCloseMixed::new(ids, labels)));
            }
        }
    }

    fn show_wizard(&mut self, ctx: &egui::Context) {
        use crate::wizard::WizardAction;
        // a dialog the wizard opened comes first; the wizard waits behind it
        if self.dialog.is_some() {
            return;
        }
        let Some(wizard) = self.wizard.as_mut() else { return };
        let folders_dir = self.editor.folders_dir();
        let core = self.core.as_ref();
        let facts = crate::wizard::Facts {
            terminal_name: core.map_or(default_terminal_name(), |c| c.terminal_name().to_string()),
            has_profile: core.map_or(cfg!(windows), Core::has_profile),
            terminal: match core {
                Some(c) if !c.has_profile() => t!("terminal-found"),
                _ => self.profile.terminal_text(),
            },
            terminal_problem: match core {
                Some(c) if !c.has_profile() => None,
                _ => self.profile.terminal_problem(),
            },
            profile: self.profile.describe(),
            profile_usable: self.profile.status.usable(),
            agent: self.agent.status(),
            securecrt: self.securecrt.clone(),
            putty: self.putty_sessions,
            hosts: self.tree.hosts().count(),
            public_keys: crate::key_dialog::public_keys(&self.ssh_dir).len(),
            ssh_dir: &self.ssh_dir,
            data_dir: &self.data_dir,
            data_movable: native_term_app::data_dir::pointer_for(self.data_source)
                != native_term_app::data_dir::Pointer::Fixed,
            can_open_tabs: self.core.is_some(),
            folders_dir: &folders_dir,
            sync_roots: &self.sync_roots,
        };
        let actions = wizard.show(ctx, &facts);
        for action in actions {
            match action {
                WizardAction::InstallProfile => {
                    if let Err(e) = self.profile.install_now() {
                        self.notices.push(t!("profile-install-failed", error = e));
                    }
                }
                WizardAction::OpenSettings => self.show_settings = true,
                WizardAction::ImportSecureCrt => {
                    self.dialog =
                        Some(Dialog::Import(Box::new(ImportDialog::new(self.ssh_dir.clone(), self.data_dir.clone()))));
                }
                WizardAction::ImportPutty => {
                    let dialog = ImportDialog::putty(self.ssh_dir.clone(), self.data_dir.clone(), &self.egui_ctx);
                    self.dialog = Some(Dialog::Import(Box::new(dialog)));
                }
                WizardAction::CreateKey => {
                    if let Some(core) = &self.core {
                        let path = self.ssh_dir.join("id_ed25519").display().to_string();
                        if let Err(e) = core.terminal().open_tool(&t!("key-create-tab"), &["--create-key".into(), path])
                        {
                            self.notices.push(e.to_string());
                        }
                    }
                }
                WizardAction::InstallKeys => {
                    let hosts: Vec<(String, String)> = self
                        .tree
                        .hosts()
                        .filter(|(_, h)| h.plink.is_none())
                        .map(|(_, h)| (h.alias().to_string(), h.label().to_string()))
                        .collect();
                    if !hosts.is_empty() {
                        self.dialog = Some(Dialog::Key(Box::new(KeyDialog::new(hosts, &self.ssh_dir))));
                    }
                }
                WizardAction::MoveFolders(path) => {
                    self.move_folders_to(path);
                }
                WizardAction::AdoptFolders(path) => {
                    self.adopt_folders_at(path);
                }
                WizardAction::MoveData(path) => {
                    self.move_data(&path);
                }
                WizardAction::OpenFolder(path) => {
                    let _ = native_term_os::shell::open_folder(&path);
                }
                WizardAction::Done => {
                    if let Some(core) = &self.core {
                        core.set_setting(crate::wizard::DONE_SETTING, "1");
                    }
                    self.wizard = None;
                }
            }
        }
    }

    /// What was written about a host, if anything.
    fn note_of(&self, host: &native_term_config::tree::HostEntry) -> Option<&native_term_app::registry::Note> {
        self.notes.get(host.id()?)
    }

    /// Keep a note: in `state.db` and in `notes.toml`. The host needs a
    /// stable id for it, which is written into its block if it hasn't one.
    fn save_note(&mut self, alias: &str, note: native_term_app::registry::Note) {
        let Some(registry) = self.core.as_ref().and_then(Core::registry) else { return };
        let Some((_, host)) = self.tree.find(alias) else { return };
        let had_id = host.id().is_some();
        let id = match self.editor.ensure_id(host) {
            Ok(id) => id,
            Err(e) => {
                self.notices.push(t!("notice-notes", error = e.to_string()));
                return;
            }
        };
        // nothing was written and there is nothing kept: leave it alone
        if note.is_empty() && !self.notes.contains_key(&id) {
            return;
        }
        if let Err(e) = native_term_app::notes::save(registry, &self.data_dir, &mut self.notes, &id, note) {
            self.notices.push(t!("notice-notes", error = e.to_string()));
        }
        self.tags_changed();
        if !had_id {
            self.reload();
        }
    }

    /// The host `alias` is logged in to as `user` from now on (the password
    /// window's user name, with "Save password"): its own `User`.
    fn set_user(&mut self, alias: &str, user: &str) {
        let Some((_, host)) = self.tree.find(alias) else { return };
        if host.plink.is_some() || host.user.as_deref() == Some(user) {
            return;
        }
        let mut draft = HostDraft::from_host(host);
        draft.user = Some(user.to_string());
        match self.editor.update_host(host, &draft) {
            Ok(()) => {
                self.notices.push(t!("notice-user-changed", alias = alias, user = user));
                self.reload();
            }
            Err(e) => self.notices.push(t!("notice-user-not-changed", alias = alias, error = e.to_string())),
        }
    }

    /// The notes changed, and the tags there are may have with them:
    /// read again, for the chips and the dialogs.
    fn tags_changed(&mut self) {
        self.notes_generation += 1;
        if let Some(registry) = self.core.as_ref().and_then(Core::registry) {
            match registry.tags() {
                Ok(tags) => self.tags = tags,
                Err(e) => self.notices.push(t!("notice-notes", error = e.to_string())),
            }
        }
        crate::dialogs::set_known_tags(&self.tags);
    }

    /// A tag called something else (`new`) or deleted, everywhere it is;
    /// the chip that was on for it is on for what it is called now.
    fn retag(&mut self, old: &str, new: Option<&str>) -> Result<(), String> {
        let Some(registry) = self.core.as_ref().and_then(Core::registry) else { return Ok(()) };
        native_term_app::notes::retag(registry, &self.data_dir, &mut self.notes, old, new)
            .map_err(|e| e.to_string())?;
        self.tags_changed();
        if self.view.filter == crate::tree_view::Filter::Tag(old.to_string()) {
            self.view.filter = match new {
                Some(new) => crate::tree_view::Filter::Tag(new.to_string()),
                None => crate::tree_view::Filter::All,
            };
        }
        Ok(())
    }

    fn show_dialog(&mut self, ctx: &egui::Context) {
        if let Some(Dialog::Tag(d)) = self.dialog.as_mut() {
            // (apart from the others: what it does is the window's to do)
            match d.show(ctx) {
                Outcome::Open => {}
                Outcome::Cancel => self.dialog = None,
                Outcome::Submit(name) => {
                    let tag = d.tag.clone();
                    match self.retag(&tag, name.as_deref()) {
                        Ok(()) => self.dialog = None,
                        Err(e) => {
                            if let Some(Dialog::Tag(d)) = self.dialog.as_mut() {
                                d.error = Some(e);
                            }
                        }
                    }
                }
            }
            return;
        }
        let Some(dialog) = self.dialog.as_mut() else { return };
        let done = match dialog {
            Dialog::Tag(_) => false,
            Dialog::CredentialSets(d) => matches!(d.show(ctx), Outcome::Cancel),
            Dialog::Cleanup(d) => {
                let mut actions = Vec::new();
                let done = matches!(d.show(ctx, &self.profile.status, &mut actions), Outcome::Cancel);
                for action in actions {
                    match action {
                        crate::cleanup::CleanupAction::RemoveFragment => {
                            if let Err(e) = self.profile.remove_fragment() {
                                d.message = Some(t!("profile-remove-failed", error = e));
                            }
                        }
                    }
                }
                done
            }
            Dialog::Host(d) => match d.show(ctx) {
                Outcome::Open => false,
                Outcome::Cancel => true,
                Outcome::Submit(draft) => {
                    let note = d.note_now();
                    let result = match (&d.alias, &d.file) {
                        (Some(alias), _) => match self.tree.find(alias) {
                            Some((_, host)) => self
                                .editor
                                .update_host(host, &draft)
                                .map(|()| Some(alias.clone()))
                                .map_err(|e| e.to_string()),
                            None => Err(t!("error-host-gone", alias = alias.as_str())),
                        },
                        (None, Some(file)) => {
                            self.editor.create_host(&self.tree, file, &draft).map(Some).map_err(|e| e.to_string())
                        }
                        (None, None) => Ok(None),
                    };
                    match result {
                        Ok(alias) => {
                            // the tree has to hold the new host before its
                            // note can be kept by its id
                            if let Some(alias) = alias {
                                self.reload();
                                self.save_note(&alias, note);
                            }
                            true
                        }
                        Err(e) => {
                            d.error = Some(e);
                            false
                        }
                    }
                }
            },
            Dialog::Plink(d) => match d.show(ctx) {
                Outcome::Open => false,
                Outcome::Cancel => true,
                Outcome::Submit(()) => {
                    let note = d.note_now();
                    let result = match (d.alias.clone(), d.file.clone()) {
                        (Some(alias), _) => match self.tree.find(&alias) {
                            Some((_, host)) => d.session(&alias).and_then(|s| {
                                self.editor
                                    .update_plink(host, &s)
                                    .map(|()| Some(alias.clone()))
                                    .map_err(|e| e.to_string())
                            }),
                            None => Err(t!("error-host-gone", alias = alias.as_str())),
                        },
                        (None, Some(file)) => {
                            let folder = self
                                .tree
                                .folders()
                                .find(|f| f.file == file)
                                .map(|f| f.name.clone())
                                .unwrap_or_default();
                            let name =
                                native_term_config::alias::unique(&d.name_base(), &folder, &self.tree.taken_aliases());
                            d.session(&name).and_then(|mut s| {
                                s.id = Some(native_term_config::new_id());
                                self.editor.add_plink(&file, &s).map(|()| Some(name)).map_err(|e| e.to_string())
                            })
                        }
                        (None, None) => Ok(None),
                    };
                    match result {
                        Ok(alias) => {
                            if let Some(alias) = alias {
                                self.reload();
                                self.save_note(&alias, note);
                            }
                            true
                        }
                        Err(e) => {
                            d.error = Some(e);
                            false
                        }
                    }
                }
            },
            Dialog::Folder(d) => match d.show(ctx) {
                Outcome::Open => false,
                Outcome::Cancel => true,
                Outcome::Submit(name) => {
                    let result = match &d.file {
                        Some(file) => self.editor.rename_folder(file, &name),
                        None => self.editor.create_folder(&name).map(|_| ()),
                    };
                    match result {
                        Ok(()) => true,
                        Err(e) => {
                            d.error = Some(e.to_string());
                            false
                        }
                    }
                }
            },
            Dialog::Import(d) => match d.show(ctx, &self.tree) {
                Outcome::Open => false,
                Outcome::Cancel => {
                    self.dialog = None;
                    return;
                }
                Outcome::Submit(()) => true,
            },
            Dialog::ServerSessions(d) => {
                let Some(core) = self.core.clone() else {
                    self.dialog = None;
                    return;
                };
                if let Outcome::Cancel = d.show(ctx, &core) {
                    self.dialog = None;
                }
                return;
            }
            Dialog::Send(d) => {
                let Some(core) = self.core.clone() else {
                    self.dialog = None;
                    return;
                };
                if let Outcome::Cancel = d.show(ctx, &core) {
                    self.dialog = None;
                }
                return;
            }
            Dialog::Key(d) => {
                let Some(core) = self.core.clone() else {
                    self.dialog = None;
                    return;
                };
                if let Outcome::Cancel = d.show(ctx, &core) {
                    self.dialog = None;
                }
                return;
            }
            Dialog::Forget(d) => match d.show(ctx) {
                Outcome::Open => false,
                Outcome::Cancel => true,
                Outcome::Submit(()) => {
                    let result = match self.tree.find(&d.alias) {
                        Some((_, host)) => self.editor.forget_host_keys(host).map_err(|e| e.to_string()),
                        None => Err(t!("error-host-gone", alias = d.alias.as_str())),
                    };
                    match result {
                        Ok(removed) => {
                            self.notices.push(if removed.is_empty() {
                                t!("forget-none", alias = d.alias.as_str())
                            } else {
                                t!("forget-done", names = removed.join(", "))
                            });
                            true
                        }
                        Err(e) => {
                            d.error = Some(e);
                            false
                        }
                    }
                }
            },
            Dialog::Options(d) => match d.show(ctx) {
                Outcome::Open => false,
                Outcome::Cancel => true,
                Outcome::Submit(values) => {
                    let log = d.log_result();
                    let result = match &d.target {
                        OptionsTarget::Host(alias) => match self.tree.find(alias) {
                            Some((_, host)) => self
                                .editor
                                .set_host_options(host, &values)
                                .and_then(|()| match &log {
                                    Some(log) => self.editor.set_host_log(host, log.as_ref()),
                                    None => Ok(()),
                                })
                                .map_err(|e| e.to_string()),
                            None => Err(t!("error-host-gone", alias = alias.as_str())),
                        },
                        OptionsTarget::Folder(file) => match self.editor.set_folder_options(file, &values) {
                            Ok(own) => {
                                if !own.is_empty() {
                                    self.notices.push(t!("folder-options-own-tag", hosts = own.join(", ")));
                                }
                                match &log {
                                    Some(log) => {
                                        self.editor.set_folder_log(file, log.as_ref()).map_err(|e| e.to_string())
                                    }
                                    None => Ok(()),
                                }
                            }
                            Err(e) => Err(e.to_string()),
                        },
                    };
                    match result {
                        Ok(()) => true,
                        Err(e) => {
                            d.error = Some(e);
                            false
                        }
                    }
                }
            },
            Dialog::Drop(d) => match d.show(ctx) {
                Outcome::Open => false,
                Outcome::Cancel => true,
                Outcome::Submit((choice, remember)) => {
                    if remember {
                        if let Some(core) = &self.core {
                            let value = match choice {
                                DropChoice::Upload => "upload",
                                DropChoice::Text => "text",
                            };
                            core.set_setting("drop.action", value);
                        }
                    }
                    let (alias, session, paths, text) =
                        (d.alias.clone(), d.session.clone(), d.paths.clone(), d.text.clone());
                    match choice {
                        DropChoice::Upload => self.upload_dropped(&alias, &session, &paths),
                        DropChoice::Text => self.send_dropped_text(&session, &text),
                    }
                    true
                }
            },
            Dialog::CloseMixed(d) => match d.show(ctx) {
                Outcome::Open => false,
                Outcome::Cancel => true,
                Outcome::Submit(()) => {
                    if let Some(core) = &self.core {
                        core.close_ids(&d.ids);
                    }
                    true
                }
            },
            Dialog::Delete(d) => match d.show(ctx) {
                Outcome::Open => false,
                Outcome::Cancel => true,
                Outcome::Submit(()) => {
                    let result = match self.tree.find(&d.alias) {
                        Some((_, host)) => self.editor.delete_host(host).map_err(|e| e.to_string()),
                        None => Ok(()),
                    };
                    match result {
                        Ok(()) => true,
                        Err(e) => {
                            d.error = Some(e);
                            false
                        }
                    }
                }
            },
        };
        if done {
            self.dialog = None;
            self.reload();
        }
    }

    /// The right side: open sessions, or every tab.
    /// The rail of icons at the window's left: the pages from its top,
    /// and from its bottom the settings, light or dark, and (docked)
    /// the pin.
    fn rail(&mut self, ui: &mut egui::Ui, tones: &Tones) {
        let open = self.core.as_ref().map_or(0, |c| c.sessions().iter().filter(|s| s.state.is_open()).count());
        let docked = crate::dock::docked_edge();
        let frame = egui::Frame::new().fill(tones.rail).inner_margin(egui::Margin::symmetric(0, 12));
        fn rail<'a>(tones: &Tones, icon: char, hint: &'a str, active: bool) -> layout::Rail<'a> {
            layout::Rail { icon, hint, active, count: 0, near: tones.rail_near }
        }
        // (the line at its side is the rail's own)
        let whole = ui.max_rect();
        egui::Panel::left("rail")
            .exact_size(layout::RAIL)
            .resizable(false)
            .show_separator_line(false)
            .frame(frame)
            .show_inside(ui, |ui| {
                // the design's gaps, or less in a low window
                const LOGO: f32 = 44.0;
                let below = if docked.is_some() { 3 } else { 2 };
                let buttons = Page::ALL.len() + below;
                let gap = layout::rail_gap(ui.available_height() - LOGO - 12.0, buttons);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, gap);
                // the program's own sign (`p-2 rounded-xl`, `mb-1`)
                let (at, logo) = ui.allocate_exact_size(egui::vec2(layout::RAIL, LOGO), egui::Sense::click());
                let tile = egui::Rect::from_center_size(at.center(), egui::vec2(40.0, 40.0));
                if logo.hovered() {
                    // (`hover:bg-blue-500/10`)
                    ui.painter().rect_filled(tile, 12.0, tones.tile.fill);
                }
                let sign = egui::FontId::proportional(24.0);
                ui.painter().text(tile.center(), egui::Align2::CENTER_CENTER, icons::TERMINAL, sign, tones.accent);
                // the terminal itself: its window in front, or a new
                // one with the person's own shell
                if logo.on_hover_text(t!("rail-terminal")).clicked() {
                    if let Some(core) = &self.core {
                        core.show_terminal();
                    }
                }
                for page in Page::ALL {
                    let count = if page == Page::Sessions { open } else { 0 };
                    // (what the page is, and a line about it)
                    let title = format!("{}\n{}", page.title(), page.about(self.tree.hosts().count()));
                    let button = layout::Rail { count, ..rail(tones, page.icon(), &title, self.page == page) };
                    if layout::rail_button(ui, tones, button).clicked() {
                        self.page = page;
                        match page {
                            Page::Tree | Page::Recent => self.view.focus_search(),
                            Page::Tabs => self.tab_list.focus_search(),
                            _ => {}
                        }
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    // (`space-y-3`)
                    ui.spacing_mut().item_spacing.y = gap.min(12.0);
                    let settings = t!("settings-toggle");
                    if layout::rail_button(ui, tones, rail(tones, icons::SETTINGS, &settings, self.show_settings))
                        .clicked()
                    {
                        self.show_settings = !self.show_settings;
                    }
                    let (icon, hint) = other_theme(ui);
                    let button = layout::Rail { near: tones.sun, ..rail(tones, icon, &hint, false) };
                    if layout::rail_button(ui, tones, button).clicked() {
                        self.change_theme(ui);
                    }
                    if let Some(edge) = docked {
                        let pinned = crate::dock::pinned();
                        let hint = t!("dock-pin-hint", edge = edge.name());
                        if layout::rail_button(ui, tones, rail(tones, icons::PIN, &hint, pinned)).clicked() {
                            crate::dock::set_pinned(!pinned);
                            if let Some(core) = &self.core {
                                core.set_setting(PINNED_SETTING, if pinned { "0" } else { "1" });
                            }
                        }
                    }
                });
            });
        let side = (whole.left() + layout::RAIL).round() - 0.5;
        ui.painter().vline(side, whole.y_range(), egui::Stroke::new(1.0_f32, tones.rail_line));
    }

    /// From light to dark, or from dark to light: the setting, and the
    /// terminal with it.
    fn change_theme(&mut self, ui: &egui::Ui) {
        let theme = if ui.visuals().dark_mode { "light" } else { "dark" };
        if let Some(core) = &self.core {
            core.set_setting(THEME_SETTING, theme);
            sync_terminal_look(core);
        }
        apply_theme(ui.ctx(), Some(theme));
    }

    /// Where a new host goes: the folder that is chosen, the chosen
    /// host's, or the main config.
    fn new_host_file(&self) -> PathBuf {
        let chosen = match self.view.chosen(&self.tree) {
            Chosen::Folder(path) => self.view.folder(&self.tree, &path).and_then(|folder| folder.file),
            Chosen::Host(alias) => self.tree.find(&alias).map(|(_, host)| host.file.clone()),
            _ => None,
        };
        chosen.unwrap_or_else(|| self.editor.main_config())
    }

    /// The header: what the page is, and for the tree what is done to
    /// all of it (the design's "Expand All", "Collapse All" and its blue
    /// "New").
    fn header(&mut self, ui: &mut egui::Ui, tones: &Tones) -> Vec<TreeAction> {
        let mut actions = Vec::new();
        // (the design's widths are the whole window's)
        let short = ui.available_width() + layout::RAIL < layout::SHORT_HEADER;
        let (maximized, focused) = ui.input(|i| {
            let window = i.viewport();
            (window.maximized.unwrap_or(false), window.focused.unwrap_or(true))
        });
        let frame = layout::bar(tones, egui::Margin::symmetric(layout::TITLE_PAD, 0));
        egui::Panel::top("header").exact_size(layout::TITLE_BAR).resizable(false).frame(frame).show_inside(ui, |ui| {
            // the window is taken by the header: what is put into it
            // afterwards is over this, and is what it is
            let bar = ui.max_rect().expand2(egui::vec2(f32::from(layout::TITLE_PAD), 0.0));
            let taken = ui.interact(bar, ui.id().with("title-bar"), egui::Sense::click_and_drag());
            if taken.double_clicked() {
                self.title_double_click(ui, maximized);
            } else if taken.drag_started_by(egui::PointerButton::Primary) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            let mut caption = None;
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if cfg!(target_os = "macos") {
                    caption = layout::caption_dots(ui, tones, focused);
                    ui.add_space(6.0);
                }
                layout::header_tile(ui, tones, self.page.icon());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if !cfg!(target_os = "macos") {
                        caption = layout::caption_buttons(ui, tones, maximized);
                    }
                    if self.page == Page::Tree {
                        self.header_buttons(ui, tones, short, &mut actions);
                    }
                    ui.add_space(4.0);
                    // the title has what the buttons leave
                    layout::header_title(ui, tones, &self.page.title(), ui.available_width());
                });
            });
            match caption {
                Some(layout::Caption::Minimize) => {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                Some(layout::Caption::Maximize | layout::Caption::Restore) => {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                }
                Some(layout::Caption::Close) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                None => {}
            }
        });
        actions
    }

    /// Two clicks on the header: what the system does for them where it
    /// says (macOS: to fill the screen, to go to the Dock, or nothing),
    /// else the window is maximized, or is again what it was.
    fn title_double_click(&self, ui: &egui::Ui, maximized: bool) {
        let system = crate::window::main_handle().is_some_and(native_term_os::dock::title_double_click);
        if !system {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
    }

    /// The window's edges, where the system gives it none (Windows and
    /// the Linux desktops: the title bar went, and the frame with it):
    /// a line around it, and along it the bands it is taken by to be
    /// made larger or smaller. Over everything else, so that what is
    /// under a band is not pressed with it. Not while the window fills
    /// the screen.
    fn window_frame(&self, ui: &egui::Ui, tones: &Tones) {
        if cfg!(target_os = "macos") {
            return;
        }
        let filling = ui.input(|i| {
            let window = i.viewport();
            window.maximized.unwrap_or(false) || window.fullscreen.unwrap_or(false)
        });
        if filling {
            return;
        }
        let ctx = ui.ctx();
        let whole = ctx.content_rect();
        let over = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("window-frame"));
        let line = egui::Stroke::new(1.0_f32, tones.line);
        ctx.layer_painter(over).rect_stroke(whole, 0.0, line, egui::StrokeKind::Inside);
        let band = layout::FRAME_BAND;
        let bands = [
            ("n", egui::Rect::from_min_max(whole.min, egui::pos2(whole.right(), whole.top() + band))),
            ("s", egui::Rect::from_min_max(egui::pos2(whole.left(), whole.bottom() - band), whole.max)),
            ("w", egui::Rect::from_min_max(whole.min, egui::pos2(whole.left() + band, whole.bottom()))),
            ("e", egui::Rect::from_min_max(egui::pos2(whole.right() - band, whole.top()), whole.max)),
        ];
        for (name, rect) in bands {
            egui::Area::new(egui::Id::new(("window-frame", name)))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .constrain(false)
                .show(ctx, |ui| {
                    let (_, edge) = ui.allocate_exact_size(rect.size(), egui::Sense::drag());
                    let to = edge.hover_pos().and_then(|at| layout::frame_hit(at - whole.min, whole.size()));
                    let Some(to) = to else { return };
                    ui.ctx().set_cursor_icon(layout::frame_cursor(to));
                    if edge.drag_started_by(egui::PointerButton::Primary) {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::BeginResize(to));
                    }
                });
        }
    }

    /// The tree's buttons, from the right: a narrow window has their
    /// signs only.
    fn header_buttons(&mut self, ui: &mut egui::Ui, tones: &Tones, short: bool, actions: &mut Vec<TreeAction>) {
        let button = |ui: &mut egui::Ui, kind: Kind, icon: char, text: String| {
            if short {
                layout::button(ui, tones, kind, Room::Header, Some(icon), "").on_hover_text(text)
            } else {
                let icon = (kind == Kind::Primary).then_some(icon);
                layout::button(ui, tones, kind, Room::Header, icon, &text)
            }
        };
        let new = button(ui, Kind::Primary, icons::PLUS_CIRCLE, t!("header-new"));
        egui::Popup::menu(&new).show(|ui| {
            if ui.button(icons::with(icons::HOST, t!("menu-new-host"))).clicked() {
                actions.push(TreeAction::NewHost(self.new_host_file()));
                ui.close();
            }
            if ui.button(icons::with(icons::NETWORK, t!("menu-new-plink"))).clicked() {
                actions.push(TreeAction::NewPlink(self.new_host_file()));
                ui.close();
            }
            if ui.button(icons::with(icons::NEW_FOLDER, t!("header-new-folder"))).clicked() {
                actions.push(TreeAction::NewFolder);
                ui.close();
            }
        });
        let reload = layout::button(ui, tones, Kind::Plain, Room::Header, Some(icons::REFRESH), "");
        if reload.on_hover_text(t!("tree-reload-hint")).clicked() {
            actions.push(TreeAction::Reload);
        }
    }

    /// Whether there is something to say above the page.
    fn warns(&self) -> bool {
        self.profile.warns()
            || self.agent.warns(self.core.as_ref())
            || self.storage.warns()
            || self.core.as_ref().is_some_and(|core| !core.lost_at_start().is_empty())
            || !self.notices.is_empty()
    }

    /// What is wrong, and what was just said, under the header.
    fn banners(&mut self, ui: &mut egui::Ui, tones: &Tones) {
        if !self.warns() {
            return;
        }
        let frame = layout::bar(tones, egui::Margin::symmetric(20, 10));
        egui::Panel::top("notices").frame(frame).show_inside(ui, |ui| {
            self.profile.banner(ui, &mut self.notices);
            let mut settings = false;
            self.agent.banner(ui, self.core.as_ref(), &mut settings);
            if settings {
                self.show_settings = true;
                self.settings_page = SettingsPage::Keys;
            }
            self.storage.banner(ui, &self.ssh_dir, &self.data_dir);
            self.lost_banner(ui);
            if !self.notices.is_empty() {
                let mut clear = false;
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(tones.busy.text, self.notices.join("  ·  "));
                    clear = ui.small_button(icons::CLEAR.to_string()).clicked();
                });
                if clear {
                    self.notices.clear();
                }
            }
        });
    }

    /// The bar below the page (`h-9`): how much there is, and at its
    /// end what is open, in which terminal.
    fn footer(&self, ui: &mut egui::Ui, tones: &Tones) {
        let frame = layout::bar(tones, egui::Margin::symmetric(16, 0));
        let hosts = self.tree.hosts().count();
        let folders = self.tree.folders().filter(|f| !f.name.is_empty()).count();
        let selected = self.view.selected();
        let open = self.core.as_ref().map_or(0, |c| c.sessions().iter().filter(|s| s.state.is_open()).count());
        let terminal = self.core.as_ref().map_or(default_terminal_name(), |c| c.terminal_name().to_string());
        egui::Panel::bottom("footer").exact_size(layout::FOOTER).resizable(false).frame(frame).show_inside(ui, |ui| {
            ui.horizontal_centered(|ui| {
                layout::footer_count(ui, tones, icons::LAYERS, &t!("footer-hosts"), hosts);
                layout::footer_count(ui, tones, icons::FOLDER, &t!("footer-folders"), folders);
                if selected > 0 {
                    layout::footer_count(ui, tones, icons::ACCEPT, &t!("footer-selected"), selected);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let text = t!("footer-open", count = open, terminal = terminal.as_str());
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).size(layout::SMALL).color(tones.weak)).truncate(),
                    );
                    layout::dot(ui, if open > 0 { tones.alive } else { tones.near });
                });
            });
        });
    }

    /// The tree (or the hosts used lately), what is chosen in it at its
    /// right, the bar below it. A narrow window has what is chosen under
    /// the tree, and only while something is.
    fn tree_page(
        &mut self,
        ui: &mut egui::Ui,
        tones: &Tones,
        recent: &[String],
        activity: &HashMap<String, Activity>,
    ) -> Vec<TreeAction> {
        let narrow = ui.available_width() < layout::NARROW - layout::RAIL;
        let chosen = self.view.chosen(&self.tree);
        let written = crate::tree_view::Written { notes: &self.notes, generation: self.notes_generation };
        let servers = self.core.as_ref().map(Core::servers).unwrap_or_default();
        let servers = &*servers;
        let logos = &self.logos;
        let about = crate::properties::About { tree: &self.tree, view: &self.view, activity, written, servers, logos };
        let frame = egui::Frame::new().fill(tones.bar);
        let mut asked = Vec::new();
        if narrow {
            self.footer(ui, tones);
            if chosen != Chosen::Nothing {
                egui::Panel::bottom("properties-under")
                    .resizable(true)
                    .default_size(260.0)
                    .size_range(160.0..=420.0)
                    .frame(frame)
                    .show_inside(ui, |ui| asked = crate::properties::show(ui, &about, &chosen));
            }
        } else {
            egui::Panel::right("properties")
                .resizable(true)
                .default_size(layout::PROPERTIES)
                .size_range(260.0..=480.0)
                .frame(frame)
                .show_inside(ui, |ui| asked = crate::properties::show(ui, &about, &chosen));
            self.footer(ui, tones);
        }
        let scope = if self.page == Page::Recent { Scope::Recent } else { Scope::Tree };
        let tags = &self.tags;
        let servers = self.core.as_ref().map(Core::servers).unwrap_or_default();
        let servers = &*servers;
        let generation = self.generation;
        let shown = Shown { tree: &self.tree, generation, recent, activity, written, tags, servers, scope };
        let page = egui::Frame::new().fill(tones.page);
        let mut actions =
            egui::CentralPanel::default().frame(page).show_inside(ui, |ui| self.view.show(ui, &shown)).inner;
        for asked in asked {
            match asked {
                crate::properties::Asked::Tree(action) => actions.push(action),
                crate::properties::Asked::Nothing => self.view.choose_nothing(),
            }
        }
        actions
    }

    /// A page that is not the tree: the bar below, and the page with
    /// room around it.
    fn page(&mut self, ui: &mut egui::Ui, tones: &Tones, inside: impl FnOnce(&mut App, &mut egui::Ui)) {
        self.footer(ui, tones);
        let frame = egui::Frame::new().fill(tones.page).inner_margin(16);
        egui::CentralPanel::default().frame(frame).show_inside(ui, |ui| inside(self, ui));
    }

    /// Every tab of the terminal; a tab of ours here can take files as
    /// its own tab does.
    fn tabs_page(&mut self, ui: &mut egui::Ui) {
        let Some(core) = self.core.clone() else { return };
        if let Some((session, paths)) = self.tab_list.show(ui, &core) {
            let alias = core.sessions().into_iter().find(|s| s.id == session).map(|s| s.alias);
            if let Some(alias) = alias {
                let text = paths_as_text(&paths);
                self.dropped(&alias, &session, paths, &text, true);
            }
        }
    }

    /// A command to the session in front or to all that are logged in,
    /// the dialog that chooses among them, and what was sent in this run.
    fn send_page(&mut self, ui: &mut egui::Ui, tones: &Tones) {
        let Some(core) = self.core.clone() else { return };
        let no_group_send = self.no_group_send();
        ui.spacing_mut().item_spacing.y = 12.0;
        layout::card(ui, tones, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            self.send_line.show(ui, &core, &no_group_send);
        });
        let tmux = self.tmux_hosts();
        let reachable =
            core.sessions().iter().filter(|s| s.state == State::Connected || tmux.contains(&s.alias)).count();
        ui.add_enabled_ui(reachable > 0, |ui| {
            let several =
                layout::button(ui, tones, Kind::Plain, Room::Panel, Some(icons::SEND), &t!("sessions-send-many"));
            if several.clicked() && self.dialog.is_none() {
                self.dialog = Some(Dialog::Send(Box::new(self.send_dialog(&core, &[]))));
            }
        });
        layout::caption(ui, tones, &t!("send-history"));
        if self.send_line.history().is_empty() {
            ui.label(egui::RichText::new(t!("send-history-none")).size(layout::SMALL).color(tones.weak));
            return;
        }
        let mut again = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            // the latest first
            for line in self.send_line.history().iter().rev() {
                let text = egui::RichText::new(line).monospace().color(tones.text);
                let row = ui.add(egui::Button::new(text).frame(false).truncate());
                if row.on_hover_text(t!("send-history-hint")).clicked() {
                    again = Some(line.clone());
                }
            }
        });
        if let Some(line) = again {
            self.send_line.take(&line);
        }
    }

    /// Where sessions can be taken from.
    fn import_page(&mut self, ui: &mut egui::Ui, tones: &Tones) {
        ui.spacing_mut().item_spacing.y = 12.0;
        let source = |ui: &mut egui::Ui, icon: char, name: &str, about: String, button: String| {
            layout::card(ui, tones, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    layout::tile(ui, 48.0, icon, None, (tones.bar, tones.line, tones.accent));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let clicked =
                            layout::button(ui, tones, Kind::Primary, Room::Panel, Some(icons::IMPORT), &button)
                                .clicked();
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.spacing_mut().item_spacing.y = 3.0;
                            ui.add_space(4.0);
                            let name = egui::RichText::new(name).size(layout::MIDDLE).color(tones.text);
                            ui.add(egui::Label::new(name).truncate());
                            let line = egui::RichText::new(about.as_str()).size(layout::SMALL).color(tones.weak);
                            ui.add(egui::Label::new(line).truncate()).on_hover_text(about.as_str());
                        });
                        clicked
                    })
                    .inner
                })
                .inner
            })
        };
        let securecrt = match &self.securecrt {
            Some(path) => t!("import-found", path = path.display().to_string()),
            None => t!("import-securecrt-none"),
        };
        if source(ui, icons::LOCK, "SecureCRT", securecrt, t!("import-button")) && self.dialog.is_none() {
            let dialog = ImportDialog::new(self.ssh_dir.clone(), self.data_dir.clone());
            self.dialog = Some(Dialog::Import(Box::new(dialog)));
        }
        if self.putty_sessions
            && source(ui, icons::NETWORK, "PuTTY", t!("import-putty-about"), t!("import-button"))
            && self.dialog.is_none()
        {
            let dialog = ImportDialog::putty(self.ssh_dir.clone(), self.data_dir.clone(), &self.egui_ctx);
            self.dialog = Some(Dialog::Import(Box::new(dialog)));
        }
        let ssh = t!("import-openssh", dir = self.ssh_dir.display().to_string());
        ui.label(egui::RichText::new(ssh).size(layout::SMALL).color(tones.weak));
    }

    /// The settings, in a window of their own over the main one: their
    /// pages at its left, the page chosen beside them.
    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let tones = crate::looks::tones(&ctx.global_style().visuals);
        // as large as the main window leaves it, in its middle
        let around = ctx.content_rect();
        let size =
            egui::vec2((around.width() - 64.0).clamp(320.0, 720.0), (around.height() - 120.0).clamp(240.0, 520.0));
        const PAGES: f32 = 168.0;
        let mut open = true;
        egui::Window::new(icons::with(icons::SETTINGS, t!("settings-toggle")))
            .id(egui::Id::new("settings-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size(size)
            .show(ctx, |ui| {
                ui.set_min_height(size.y);
                egui::Panel::left("settings-pages")
                    .exact_size(PAGES)
                    .resizable(false)
                    .frame(egui::Frame::new().inner_margin(egui::Margin { right: 12, ..egui::Margin::ZERO }))
                    .show_inside(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        for page in SettingsPage::ALL {
                            let on = self.settings_page == page;
                            let row = egui::Button::selectable(on, page.title());
                            let width = ui.available_width();
                            if ui.add_sized([width, 30.0], row).clicked() {
                                self.settings_page = page;
                            }
                        }
                        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| about(ui, &tones));
                    });
                let page = egui::Frame::new().inner_margin(egui::Margin { left: 16, ..egui::Margin::ZERO });
                egui::CentralPanel::default().frame(page).show_inside(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("settings-page").auto_shrink([false, false]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        self.settings_page(ui);
                    });
                });
            });
        if !open {
            self.show_settings = false;
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        match self.settings_page {
            SettingsPage::General => {
                let Some(core) = self.core.clone() else { return };
                language_choice(ui, &core);
                let mut auto = core.auto_reconnect();
                if ui.checkbox(&mut auto, t!("auto-reconnect-setting")).changed() {
                    core.set_auto_reconnect(auto);
                }
                let mut close = core.close_on_exit();
                let response =
                    ui.checkbox(&mut close, t!("close-on-exit-setting")).on_hover_text(t!("close-on-exit-hint"));
                if response.changed() {
                    core.set_setting(native_term_app::CLOSE_ON_EXIT_SETTING, if close { "1" } else { "0" });
                }
                let mut ask = self.remembered_drop().is_none();
                let response = ui.checkbox(&mut ask, t!("drop-ask-setting")).on_hover_text(t!("drop-ask-hint"));
                if response.changed() && ask {
                    core.set_setting("drop.action", "");
                }
                let mut ask = native_term_app::quotation::prompts(&core);
                let response = ui.checkbox(&mut ask, t!("quote-ask-setting")).on_hover_text(t!("quote-ask-hint"));
                if response.changed() {
                    native_term_app::quotation::set_prompts(&core, ask);
                }
                lookup_choice(ui, &core);
                ui.separator();
                if ui.button(t!("wizard-open")).clicked() && self.wizard.is_none() {
                    self.wizard = Some(crate::wizard::Wizard::new(ui.ctx()));
                    self.show_settings = false;
                }
            }
            SettingsPage::Look => {
                let Some(core) = self.core.clone() else { return };
                theme_choice(ui, &core);
                look_choice(ui, &core);
                let mut cards = core.setting(native_term_app::tab_menu::HOVER_SETTING).as_deref() != Some("off");
                if ui.checkbox(&mut cards, t!("tabs-hover-setting")).on_hover_text(t!("tabs-hover-hint")).changed() {
                    core.set_setting(native_term_app::tab_menu::HOVER_SETTING, if cards { "on" } else { "off" });
                }
                let mut switcher = core.ctrl_tab();
                if ui
                    .checkbox(&mut switcher, t!("tabs-switcher-setting"))
                    .on_hover_text(t!("tabs-switcher-hint"))
                    .changed()
                {
                    core.set_ctrl_tab(switcher);
                    sync_terminal_look(&core);
                }
            }
            SettingsPage::Terminal => {
                let before = self.profile.favorites_on(self.core.as_ref());
                self.profile.settings_ui(ui, self.core.as_ref(), &mut self.notices);
                if self.profile.favorites_on(self.core.as_ref()) != before {
                    self.refresh_terminal_favorites();
                }
            }
            SettingsPage::Keys => {
                self.agent.settings_ui(ui, &self.ssh_dir, self.core.as_ref());
                if ui.small_button(t!("button-check-again")).clicked() {
                    self.agent.refresh(&self.ssh_dir, ui.ctx());
                }
                ui.separator();
                let sets = ui.button(t!("cred-sets-button")).on_hover_text(t!("cred-sets-intro"));
                if sets.clicked() && self.dialog.is_none() {
                    let dialog = crate::credential_sets::CredentialSetsDialog::new(&self.tree);
                    self.dialog = Some(Dialog::CredentialSets(Box::new(dialog)));
                }
            }
            SettingsPage::Shortcuts => self.keys.settings_ui(ui, self.core.as_ref()),
            SettingsPage::Data => {
                self.folders_ui(ui);
                self.data_dir_ui(ui);
                ui.separator();
                // before the program folder is deleted
                if ui.button(t!("cleanup-button")).on_hover_text(t!("cleanup-intro")).clicked() && self.dialog.is_none()
                {
                    let cleanup = crate::cleanup::Cleanup::new(
                        &self.ssh_dir,
                        &self.editor.folders_dir(),
                        self.profile.shim_path(),
                        self.data_dir.clone(),
                    );
                    let dialog = crate::cleanup::CleanupDialog::new(cleanup);
                    self.dialog = Some(Dialog::Cleanup(Box::new(dialog)));
                }
            }
        }
    }

    fn sessions_panel(&mut self, ui: &mut egui::Ui, core: &Core) {
        let core = core.clone();
        let sessions = core.sessions();
        let tones = crate::looks::tones(ui.visuals());
        // nothing open: what the page is for, and where sessions come from
        if sessions.is_empty() {
            layout::empty(ui, &tones, icons::CONNECT, &t!("sessions-empty"));
            return;
        }
        ui.horizontal(|ui| {
            let button = |ui: &mut egui::Ui, icon: char, text: String| {
                layout::button(ui, &tones, Kind::Plain, Room::Header, Some(icon), &text)
            };
            if button(ui, icons::CLEAR, t!("sessions-clear-finished")).clicked() {
                core.clear_finished();
            }
            let unlocated = core.unlocated();
            if unlocated > 0
                && button(ui, icons::SEARCH, t!("sessions-locate", count = unlocated))
                    .on_hover_text(t!("sessions-locate-hint"))
                    .clicked()
            {
                core.locate();
            }
            // logged in, or kept in tmux on the server (sent there through it)
            let tmux = self.tmux_hosts();
            let reachable = sessions.iter().filter(|s| s.state == State::Connected || tmux.contains(&s.alias)).count();
            ui.add_enabled_ui(reachable > 0, |ui| {
                if button(ui, icons::SEND, t!("sessions-send-many")).clicked() && self.dialog.is_none() {
                    self.dialog = Some(Dialog::Send(Box::new(self.send_dialog(&core, &[]))));
                }
            });
        });
        // after a restart or a Terminal restore: reconnect all, some, or none
        let waiting: Vec<&SessionView> = sessions.iter().filter(|s| s.state == State::Waiting && s.linked).collect();
        if !waiting.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(
                    egui::Color32::from_rgb(0xd0, 0x9a, 0x1a),
                    t!("sessions-restored-waiting", count = waiting.len()),
                );
                if ui.button(t!("sessions-connect-all")).clicked() {
                    core.connect_all(waiting.iter().map(|s| s.id.clone()).collect());
                }
                if ui.button(t!("sessions-close-all")).clicked() {
                    if let Closing::Confirm(ids) = core.close_sessions(&CloseSet::Waiting) {
                        crate::shell::ask(MenuRequest::ConfirmClose(ids));
                    }
                }
                ui.weak(t!("sessions-one-by-one"));
            });
        }
        ui.add_space(4.0);
        let mut save = None;
        let mut send = None;
        let mut dropped: Option<(String, String, Vec<PathBuf>)> = None;
        let tmux = self.tmux_hosts();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for s in &sessions {
                // typed by hand, not in the tree: offer to save it
                let typed = self
                    .tree
                    .find(&s.alias)
                    .is_none()
                    .then(|| native_term_app::quick::from_destination(&s.alias))
                    .flatten();
                let (card, rect) = session_card(ui, &core, s, typed, tmux.contains(&s.alias));
                match card {
                    Some(CardAction::Save(target)) => save = Some(target),
                    Some(CardAction::Send) => send = Some(s.id.clone()),
                    None => {}
                }
                // files dragged from Explorer onto a session: the same
                // question as dropping them on its tab
                if s.state.is_open() {
                    if let Some(at) = hovering_files(ui.ctx()).filter(|at| rect.contains(*at)) {
                        let _ = at;
                        let accent = ui.visuals().selection.bg_fill;
                        ui.painter().rect_stroke(
                            rect,
                            6.0,
                            egui::Stroke::new(2.0_f32, accent),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            t!("drop-on-session", label = s.label.as_str()),
                            egui::TextStyle::Body.resolve(ui.style()),
                            ui.visuals().strong_text_color(),
                        );
                    }
                    if let Some((paths, at)) = dropped_files(ui.ctx()) {
                        if rect.contains(at) {
                            dropped = Some((s.alias.clone(), s.id.clone(), paths));
                        }
                    }
                }
                ui.add_space(6.0);
            }
        });
        if let Some((alias, session, paths)) = dropped {
            let text = paths_as_text(&paths);
            self.dropped(&alias, &session, paths, &text, true);
        }
        if let (Some(id), None) = (send, &self.dialog) {
            self.dialog = Some(Dialog::Send(Box::new(self.send_dialog(&core, &[id]))));
        }
        if let (Some(target), None) = (save, &self.dialog) {
            let draft = HostDraft {
                label: target.host.clone(),
                hostname: target.host,
                user: target.user,
                port: target.port,
                ..HostDraft::default()
            };
            let main = self.editor.main_config();
            let folder = self.folder_label(&main);
            self.dialog = Some(Dialog::Host(Box::new(HostDialog::new_host_from(main, &folder, &draft))));
        }
    }
}

/// Where the pointer is while files are dragged over NativeTerm's own
/// window (`None`: nothing is being dragged).
fn hovering_files(ctx: &egui::Context) -> Option<egui::Pos2> {
    ctx.input(|i| {
        (!i.raw.hovered_files.is_empty()).then(|| i.pointer.interact_pos().or_else(|| i.pointer.latest_pos()))
    })
    .flatten()
}

/// Files just dropped on NativeTerm's own window, and where.
fn dropped_files(ctx: &egui::Context) -> Option<(Vec<PathBuf>, egui::Pos2)> {
    ctx.input(|i| {
        let paths: Vec<PathBuf> = i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect();
        let at = i.pointer.interact_pos().or_else(|| i.pointer.latest_pos());
        (!paths.is_empty()).then(|| at.map(|at| (paths, at))).flatten()
    })
}

/// The paths as a terminal would paste them (quoted when they hold a
/// space), for the "type the names" answer.
fn paths_as_text(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| {
            let text = p.display().to_string();
            if text.contains(' ') {
                format!("\"{text}\"")
            } else {
                text
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The saved hosts, for the floating button's search.
fn publish_hosts(tree: &SessionTree, core: Option<&Core>) {
    if let Some(core) = core {
        core.set_host_labels(tree.hosts().map(|(_, h)| (h.alias().to_string(), h.label().to_string())).collect());
        let looks = tree.hosts().map(|(f, h)| (h.alias().to_string(), native_term_config::appearance::for_host(f, h)));
        core.set_host_looks(looks.collect());
    }
    let hosts = tree
        .folders()
        .flat_map(|f| {
            let folder = if f.name.is_empty() { String::new() } else { f.label().to_string() };
            let no_group_send = f.no_group_send();
            f.hosts.iter().map(move |h| crate::shell::HostEntry {
                alias: h.alias().to_string(),
                label: h.label().to_string(),
                hostname: h.target().to_string(),
                folder: folder.clone(),
                on_login: f.nt(h, "onlogin").map(str::to_string),
                no_group_send,
            })
        })
        .collect();
    crate::shell::set_hosts(hosts);
}

/// `state.db` setting: `light`, `dark`, or absent for the system's.
pub const THEME_SETTING: &str = "theme";

/// The chosen theme, for the other window.
pub static THEME: std::sync::Mutex<Option<egui::ThemePreference>> = std::sync::Mutex::new(None);

pub fn apply_theme(ctx: &egui::Context, setting: Option<&str>) {
    let (preference, title_bar) = match theme_for(setting, system_dark()) {
        Some(false) => (egui::ThemePreference::Light, egui::SystemTheme::Light),
        Some(true) => (egui::ThemePreference::Dark, egui::SystemTheme::Dark),
        None => (egui::ThemePreference::System, egui::SystemTheme::SystemDefault),
    };
    ctx.set_theme(preference);
    *THEME.lock().unwrap_or_else(|e| e.into_inner()) = Some(preference);
    // the window's own title bar follows too
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(title_bar));
}

/// Dark or light: what the setting says, else what the system is known to
/// be (`None`: the toolkit follows the system). The setting first: read
/// the other way round, "dark" on a light desktop stayed light.
fn theme_for(setting: Option<&str>, system_dark: Option<bool>) -> Option<bool> {
    match setting {
        Some("light") => Some(false),
        Some("dark") => Some(true),
        _ => system_dark,
    }
}

/// What "system" means for the window: on Windows the toolkit knows
/// (and follows changes as they happen); elsewhere it is what
/// `native-term-os` read from the desktop, and light where it could
/// read nothing (the toolkit knows nothing on X11 or Wayland either,
/// and would fall back to dark; a desktop with no preference set, GNOME
/// by default, is light).
fn system_dark() -> Option<bool> {
    if cfg!(windows) {
        None
    } else {
        Some(native_term_os::appearance::cached().dark.unwrap_or(false))
    }
}

/// Set when the desktop's look changed since the last frame.
static APPEARANCE_CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// No desktop tells every program when its look changes (Windows tells
/// the toolkit; on Linux it is a portal signal at best): look every few
/// seconds, and let the next frame apply it. Windows keeps its own way.
fn watch_appearance(ctx: egui::Context) {
    if cfg!(windows) {
        return;
    }
    std::thread::Builder::new()
        .name("appearance".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(3));
            if native_term_os::appearance::refresh() {
                APPEARANCE_CHANGED.store(true, std::sync::atomic::Ordering::Relaxed);
                ctx.request_repaint();
            }
        })
        .ok();
}

/// How the terminal's windows should look, where NativeTerm writes their
/// configuration (WezTerm): what the desktop says, with the theme
/// setting (`light`/`dark`, or nothing) over it, drawn by the GPU the
/// `terminal.gpu` setting picks.
pub fn terminal_look(
    setting: Option<&str>,
    switcher: bool,
    gpu: native_term_wezterm::Gpu,
) -> native_term_wezterm::Look {
    let desktop = native_term_os::appearance::cached();
    let dark = match setting {
        Some("light") => Some(false),
        Some("dark") => Some(true),
        // Windows: WezTerm follows the system theme itself, at once
        // (WM_SETTINGCHANGE, then its configuration again with
        // get_appearance), where NativeTerm read the registry only at
        // start: a switch to light left the windows dark
        _ if cfg!(windows) => None,
        _ => desktop.dark,
    };
    let fonts = native_term_os::fonts::terminal_families(desktop.monospace.as_deref());
    native_term_wezterm::Look {
        dark,
        fonts,
        switcher,
        button_icons: native_term_os::icons::window_icons(desktop.icon_theme.as_deref())
            .named()
            .into_iter()
            .map(|(name, path)| (name, path.to_string_lossy().into_owned()))
            .collect(),
        titlebar: desktop_titlebar(&desktop, dark),
        ui_font: desktop.ui_font.clone(),
        accent: desktop.accent,
        tab_icon: native_term_os::icons::terminal_icon(desktop.icon_theme.as_deref())
            .map(|p| p.to_string_lossy().into_owned()),
        titlebar_double_click: desktop.titlebar_double_click,
        button_layout: desktop.button_layout,
        gpu,
        pane_menu: Some(pane_menu(None)),
    }
}

/// The menu of a right click in a pane, in the person's language, a
/// selection looked up with `lookup` (the `terminal.lookup_url` setting;
/// `None`: the default).
pub fn pane_menu(lookup: Option<&str>) -> native_term_wezterm::PaneMenu {
    native_term_wezterm::PaneMenu {
        copy: t!("panemenu-copy"),
        paste: t!("panemenu-paste"),
        copy_paste: t!("panemenu-copy-paste"),
        paste_quotation: t!("panemenu-paste-quotation"),
        open_selection: t!("panemenu-open-selection"),
        open_url: t!("panemenu-open-url"),
        lookup: t!("panemenu-lookup"),
        find: t!("panemenu-find"),
        select_all: t!("panemenu-select-all"),
        print: t!("panemenu-print"),
        clear: t!("tabmenu-clear"),
        lookup_url: native_term_wezterm::PaneMenu::lookup_url(lookup),
    }
}

/// The title bar as the desktop's toolkit has it, where Chrome would take
/// it from (native_term_os::titlebar): GTK's drawing, rendered once per
/// theme into the cache folder; Qt's palette colours.
fn desktop_titlebar(
    desktop: &native_term_os::appearance::Appearance,
    dark: Option<bool>,
) -> Option<native_term_os::titlebar::Titlebar> {
    use native_term_os::titlebar;
    if !cfg!(all(unix, not(target_os = "macos"))) {
        return None;
    }
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    if titlebar::toolkit(&var("XDG_CURRENT_DESKTOP"), &var("DESKTOP_SESSION")) == titlebar::Toolkit::Qt {
        // Qt's: the palette's colours (read with the rest of the desktop's
        // look, so a new colour scheme is noticed), the buttons the icon
        // theme's. Chrome asks Qt itself; NativeTerm reads KDE's
        // `kdeglobals`, which a Qt desktop that is not KDE may not have
        // (deepin): then GTK's, which such a desktop themes too
        if let Some(palette) = desktop.palette.clone() {
            return Some(palette);
        }
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))?;
    let key = format!("{:?}|{:?}", desktop.gtk_theme, desktop.icon_theme);
    let mut bar = titlebar::read(&cache.join("nativeterm").join("titlebar"), &key, dark.unwrap_or(false))?;
    if titlebar::toolkit(&var("XDG_CURRENT_DESKTOP"), &var("DESKTOP_SESSION")) == titlebar::Toolkit::Qt {
        // whatever the colours, the frame on a Qt desktop is Chrome's
        // own, not the GTK theme's decoration
        bar.edge = None;
        bar.chrome_frame = true;
    }
    Some(bar)
}

/// The terminal's windows follow the theme and the switcher setting
/// too, where NativeTerm writes their configuration.
fn sync_terminal_look(core: &Core) {
    if let Some(wezterm) = core.terminal().as_any().downcast_ref::<native_term_wezterm::WezTerm>() {
        let gpu = native_term_wezterm::Gpu::from_setting(core.setting(native_term_wezterm::Gpu::SETTING).as_deref());
        let mut look = terminal_look(core.setting(THEME_SETTING).as_deref(), core.ctrl_tab(), gpu);
        look.pane_menu = Some(pane_menu(core.setting(native_term_wezterm::PaneMenu::LOOKUP_SETTING).as_deref()));
        wezterm.set_look(&look);
    }
}

fn theme_choice(ui: &mut egui::Ui, core: &Core) {
    let setting = core.setting(THEME_SETTING).filter(|s| !s.is_empty());
    let options = [(None, t!("theme-system")), (Some("light"), t!("theme-light")), (Some("dark"), t!("theme-dark"))];
    let shown = options.iter().find(|(v, _)| *v == setting.as_deref()).map(|(_, n)| n.clone()).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(t!("theme-label"));
        egui::ComboBox::from_id_salt("theme").selected_text(shown).show_ui(ui, |ui| {
            for (value, name) in &options {
                if ui.selectable_label(setting.as_deref() == *value, name.as_str()).clicked() {
                    core.set_setting(THEME_SETTING, value.unwrap_or(""));
                    apply_theme(ui.ctx(), *value);
                    sync_terminal_look(core);
                }
            }
        });
    });
}

/// What this is, which build, and what it does not do. The last part
/// is the point of the section: a tool that drives your terminal and
/// holds your passwords should say plainly that it sends nothing
/// anywhere (the README says it too).
fn about(ui: &mut egui::Ui, tones: &Tones) {
    // (from the settings' lower end: what is said last is the uppermost)
    ui.label(egui::RichText::new(t!("about-no-telemetry")).size(layout::TINY).color(tones.weak));
    ui.label(egui::RichText::new(t!("about-title", version = env!("CARGO_PKG_VERSION"))).color(tones.text));
}

/// The theme that is not the one shown: its sign, and what changing to
/// it is called (the design's sun in the dark, its moon in the light).
/// Whether the tree's rows have their checkboxes (`on`; off otherwise).
const CHECKS_SETTING: &str = "tree.checks";

fn other_theme(ui: &egui::Ui) -> (char, String) {
    match ui.visuals().dark_mode {
        true => (icons::SUN, t!("rail-light")),
        false => (icons::MOON, t!("rail-dark")),
    }
}

/// The look on top of light and dark (see `looks.rs`).
fn look_choice(ui: &mut egui::Ui, core: &Core) {
    use crate::looks::Preset;
    let chosen = Preset::from_setting(core.setting(crate::looks::SETTING).as_deref());
    ui.horizontal(|ui| {
        ui.label(t!("theme-look-label"));
        egui::ComboBox::from_id_salt("look").selected_text(chosen.label()).show_ui(ui, |ui| {
            for preset in Preset::ALL {
                if ui.selectable_label(preset == chosen, preset.label()).clicked() {
                    core.set_setting(crate::looks::SETTING, preset.setting());
                    preset.choose(ui.ctx());
                }
            }
        });
    })
    .response
    .on_hover_text(t!("theme-look-hint"));
}

/// What "Lookup Selection" (the pane's menu) searches with: one of the
/// searches known by name, or an address of the person's own, `%s` where
/// what is looked up goes. Shown where the terminal has that menu.
fn lookup_choice(ui: &mut egui::Ui, core: &Core) {
    use native_term_wezterm::PaneMenu;
    if core.terminal().as_any().downcast_ref::<native_term_wezterm::WezTerm>().is_none() {
        return;
    }
    let setting = core.setting(PaneMenu::LOOKUP_SETTING);
    let named = PaneMenu::lookup_name(setting.as_deref());
    // the person's own, as they type it: the setting's while they don't
    let typing = egui::Id::new("lookup-own");
    let typed: Option<String> = ui.data(|d| d.get_temp(typing));
    let own = typed.is_some() || named.is_none();
    let set = |url: &str| {
        core.set_setting(PaneMenu::LOOKUP_SETTING, url);
        sync_terminal_look(core);
    };
    ui.horizontal(|ui| {
        ui.label(t!("lookup-label"));
        // (as the person knows them)
        let label = |name: &str| if name == "Baidu" { t!("lookup-baidu") } else { name.to_string() };
        let shown = if own { t!("lookup-own") } else { label(named.unwrap_or_default()) };
        egui::ComboBox::from_id_salt("lookup").selected_text(shown).show_ui(ui, |ui| {
            for (name, url) in PaneMenu::LOOKUPS {
                if ui.selectable_label(!own && named == Some(name), label(name)).clicked() {
                    ui.data_mut(|d| d.remove::<String>(typing));
                    set(url);
                }
            }
            if ui.selectable_label(own, t!("lookup-own")).clicked() && !own {
                let start = PaneMenu::lookup_url(setting.as_deref());
                ui.data_mut(|d| d.insert_temp(typing, start));
            }
        });
    })
    .response
    .on_hover_text(t!("lookup-hint"));
    if own {
        let mut url = typed.unwrap_or_else(|| PaneMenu::lookup_url(setting.as_deref()));
        let edit =
            ui.add(egui::TextEdit::singleline(&mut url).hint_text("https://…?q=%s").desired_width(f32::INFINITY));
        let edit = edit.on_hover_text(t!("lookup-own-hint"));
        if edit.changed() {
            ui.data_mut(|d| d.insert_temp(typing, url.clone()));
        }
        // taken when the person is done with the field
        if edit.lost_focus() && !url.trim().is_empty() && setting.as_deref() != Some(url.trim()) {
            set(url.trim());
        }
    }
}

/// Language: the system's, or one of NativeTerm's.
fn language_choice(ui: &mut egui::Ui, core: &Core) {
    let setting = core.language_setting();
    let languages = native_term_app::i18n::available();
    let name = |id: &str| languages.iter().find(|(l, _)| *l == id).map(|(_, n)| n.to_string());
    let shown = match &setting {
        Some(id) => name(id).unwrap_or_else(|| id.clone()),
        None => format!("{} ({})", t!("language-system"), name(&native_term_app::i18n::current()).unwrap_or_default()),
    };
    ui.horizontal(|ui| {
        ui.label(t!("language-label"));
        egui::ComboBox::from_id_salt("language").selected_text(shown).show_ui(ui, |ui| {
            // (the terminal's menus are written in the language too)
            if ui.selectable_label(setting.is_none(), t!("language-system")).clicked() {
                core.set_language_setting(None);
                sync_terminal_look(core);
            }
            for (id, native) in &languages {
                let id = id.to_string();
                if ui.selectable_label(setting.as_deref() == Some(id.as_str()), *native).clicked() {
                    core.set_language_setting(Some(&id));
                    sync_terminal_look(core);
                }
            }
        });
    });
}

fn state_color(ui: &egui::Ui, state: &State) -> egui::Color32 {
    match state {
        State::Connected => egui::Color32::from_rgb(0x2e, 0xa0, 0x43),
        State::Opening | State::Connecting | State::Detached | State::Waiting => {
            egui::Color32::from_rgb(0xd0, 0x9a, 0x1a)
        }
        State::LoginFailed(_) | State::Unreachable(_) | State::Disconnected(_) | State::Failed(_) => {
            egui::Color32::from_rgb(0xd0, 0x3a, 0x3a)
        }
        _ => ui.visuals().weak_text_color(),
    }
}

enum CardAction {
    Save(native_term_app::quick::QuickTarget),
    Send,
}

/// One open session as a card: name and state, then where its tab is and
/// what can be done.
/// One session in the list; also where it is, so files can be dropped
/// on it.
fn session_card(
    ui: &mut egui::Ui,
    core: &Core,
    s: &SessionView,
    typed: Option<native_term_app::quick::QuickTarget>,
    tmux: bool,
) -> (Option<CardAction>, egui::Rect) {
    let mut action = None;
    let color = state_color(ui, &s.state);
    let frame =
        egui::Frame::group(ui.style()).corner_radius(6.0).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 4.5, color);
                if s.locked {
                    ui.label(icons::LOCK.to_string()).on_hover_text(t!("session-locked"));
                }
                let label = ui.strong(&s.label);
                if s.label != s.alias {
                    label.on_hover_text(&s.alias);
                }
                let mut state = s.state.describe();
                if s.attempt > 1 && s.state.is_open() {
                    state.push_str(&format!(" · {}", t!("session-attempt", n = s.attempt)));
                }
                if let Some(n) = s.auto_retry {
                    state.push_str(&format!(" · {}", t!("session-auto-reconnect", n = n)));
                }
                ui.colored_label(color, state);
                // tmux's own status bar is off in NativeTerm's sessions: this says
                // the shell lives on the server instead
                if tmux {
                    ui.weak(t!("session-kept-on-server")).on_hover_text(t!("session-kept-on-server-hint"));
                }
                if let Some(since) = s.quiet_since {
                    let time = native_term_os::time::local_time_of_day(since);
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x9a, 0x1a), t!("session-quiet", time = time))
                        .on_hover_text(t!("session-quiet-hint"));
                }
                if let Some(name) = &s.renamed_to {
                    ui.weak(t!("session-renamed", name = name.as_str())).on_hover_text(t!("session-renamed-hint"));
                }
            });
            ui.horizontal_wrapped(|ui| {
                match &s.location {
                    Some(l) => {
                        let tab = l.tab_index + 1;
                        let mut text = t!("session-location", window = l.window_number, tab = tab);
                        if l.selected {
                            text.push_str(&format!(" · {}", t!("session-selected")));
                        }
                        if l.mixed {
                            text.push_str(&format!(" · {}", t!("session-split")));
                        }
                        let response = ui.weak(text);
                        if l.title != s.label {
                            response.on_hover_text(t!("session-current-title", title = l.title.as_str()));
                        }
                    }
                    None if s.state.is_open() => {
                        let text = match s.last_position {
                            Some((window, tab)) => {
                                let tab = tab + 1;
                                t!("session-not-located-hint", window = window, tab = tab)
                            }
                            None => t!("session-not-located"),
                        };
                        ui.weak(text);
                    }
                    None => {}
                }
                ui.add_space(12.0);
                let button = |ui: &mut egui::Ui, command: SessionCommand, text: String| {
                    ui.add_enabled(command.applies(s), egui::Button::new(text).small())
                };
                if button(ui, SessionCommand::Focus, t!("button-focus")).clicked() {
                    core.run(&s.id, SessionCommand::Focus);
                }
                let label = if s.state == State::Waiting { t!("button-connect") } else { t!("button-reconnect") };
                if button(ui, SessionCommand::Connect, label).clicked() {
                    core.run(&s.id, SessionCommand::Connect);
                }
                if button(ui, SessionCommand::Disconnect, t!("button-disconnect")).clicked() {
                    core.run(&s.id, SessionCommand::Disconnect);
                }
                if button(ui, SessionCommand::Close, t!("button-close")).clicked() {
                    core.run(&s.id, SessionCommand::Close);
                }
                let lock = if s.locked {
                    icons::with(icons::UNLOCK, t!("session-unlock"))
                } else {
                    icons::with(icons::LOCK, t!("session-lock"))
                };
                if button(ui, SessionCommand::ToggleLock, lock).on_hover_text(t!("session-lock-hint")).clicked() {
                    core.run(&s.id, SessionCommand::ToggleLock);
                }
                // a session kept in tmux can be sent to through it, logged in or not
                let send = ui.add_enabled(
                    SessionCommand::Send.applies(s) || tmux,
                    egui::Button::new(icons::with(icons::SEND, t!("session-send"))).small(),
                );
                if send.clicked() {
                    action = Some(CardAction::Send);
                }
                if SessionCommand::SendBreak.offered(s)
                    && button(ui, SessionCommand::SendBreak, t!("button-break"))
                        .on_hover_text(t!("session-break-hint"))
                        .clicked()
                {
                    core.run(&s.id, SessionCommand::SendBreak);
                }
                if let Some(target) = typed {
                    let button = egui::Button::new(icons::with(icons::SAVE, t!("quick-save"))).small();
                    if ui.add(button).on_hover_text(t!("quick-save-hint")).clicked() {
                        action = Some(CardAction::Save(target));
                    }
                }
            });
        });
    (action, frame.response.rect)
}

impl crate::window::Ui for App {
    fn on_exit(&mut self) {
        if let Some(core) = &self.core {
            if core.close_on_exit() {
                core.close_all();
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        if let Some(core) = &self.core {
            self.notices.extend(core.take_notices());
            if APPEARANCE_CHANGED.swap(false, std::sync::atomic::Ordering::Relaxed) {
                apply_theme(ctx, core.setting(THEME_SETTING).as_deref());
                crate::looks::Preset::from_setting(core.setting(crate::looks::SETTING).as_deref()).choose(ctx);
                sync_terminal_look(core);
            }
        }
        for (command, global) in self.keys.take(ctx) {
            self.run_shortcut(command, global);
        }
        self.reload_if_changed();
        for request in crate::shell::take_requests() {
            self.handle_menu_request(request);
        }
        if crate::shell::take_show_tabs() {
            self.page = Page::Tabs;
            self.tab_list.focus_search();
        }
        let tones = crate::looks::tones(ui.visuals());
        self.rail(ui, &tones);
        let mut actions = self.header(ui, &tones);
        self.banners(ui, &tones);
        let recent = self.recent();
        // the best state per host, for the marks in the tree
        let mut activity: HashMap<String, Activity> = HashMap::new();
        for s in self.core.as_ref().map(|c| c.sessions()).unwrap_or_default() {
            if let Some(a) = Activity::of(&s.state) {
                let best = activity.entry(s.alias).or_insert(a);
                *best = (*best).max(a);
            }
        }
        // (all the terminal's tabs are asked for while they are shown)
        if self.page != Page::Tabs {
            if let Some(core) = &self.core {
                core.want_all_tabs(false);
            }
        }
        match self.page {
            Page::Tree | Page::Recent => actions.extend(self.tree_page(ui, &tones, &recent, &activity)),
            Page::Sessions => self.page(ui, &tones, |app, ui| {
                if let Some(core) = app.core.clone() {
                    app.sessions_panel(ui, &core);
                }
            }),
            Page::Tabs => self.page(ui, &tones, |app, ui| app.tabs_page(ui)),
            Page::Send => self.page(ui, &tones, |app, ui| app.send_page(ui, &tones)),
            Page::Import => self.page(ui, &tones, |app, ui| app.import_page(ui, &tones)),
        }
        for action in actions {
            self.handle(action);
        }
        if let Some(aliases) = self.open_files.take_if(|a| a.split(',').all(|a| self.tree.find(a).is_some())) {
            // and `NATIVETERM_OPEN_FILES_SESSION=<id>`: the first as from that terminal tab's menu
            let session = std::env::var("NATIVETERM_OPEN_FILES_SESSION").ok().filter(|s| !s.is_empty());
            for (i, alias) in aliases.split(',').enumerate() {
                self.open_files(alias, session.as_ref().filter(|_| i == 0));
            }
        }
        self.settings_window(ctx);
        self.window_frame(ui, &tones);
        self.show_dialog(ctx);
        self.show_wizard(ctx);
        // over everything else, in the corner
        self.toasts.show(ctx, native_term_os::desktop::animations());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dropped files can also be typed into the session as names; a path
    /// with a space needs its quotes, as a shell expects them.
    #[test]
    fn dropped_paths_as_a_command_line() {
        let paths = [PathBuf::from(r"C:\tools\id_ed25519.pub"), PathBuf::from(r"C:\My Files\notes.txt")];
        assert_eq!(paths_as_text(&paths), r#"C:\tools\id_ed25519.pub "C:\My Files\notes.txt""#);
        assert_eq!(paths_as_text(&[]), "");
    }

    /// The theme setting decides light or dark for the terminal's windows
    /// too; without one, the desktop does (whatever it could say).
    #[test]
    fn the_terminal_follows_the_theme_setting() {
        // what the person chose, whatever the desktop is
        assert_eq!(theme_for(Some("dark"), Some(false)), Some(true));
        assert_eq!(theme_for(Some("light"), Some(true)), Some(false));
        assert_eq!(theme_for(None, Some(true)), Some(true));
        assert_eq!(theme_for(Some(""), Some(false)), Some(false));
        assert_eq!(theme_for(None, None), None, "the toolkit follows the system");
        assert_eq!(terminal_look(Some("light"), false, Default::default()).dark, Some(false));
        assert_eq!(terminal_look(Some("dark"), false, Default::default()).dark, Some(true));
        // (on Windows WezTerm asks the system itself)
        let desktop = native_term_os::appearance::cached();
        let dark = desktop.dark.filter(|_| !cfg!(windows));
        assert_eq!(terminal_look(None, false, Default::default()).dark, dark);
        assert_eq!(terminal_look(Some(""), false, Default::default()).dark, dark);
        assert_eq!(
            terminal_look(None, false, Default::default()).fonts,
            native_term_os::fonts::terminal_families(desktop.monospace.as_deref())
        );
        assert!(terminal_look(None, true, Default::default()).switcher);
    }
}
