//! The main window: the session tree, the open sessions, and the dialogs
//! that edit sessions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use native_term_app::actions::{CloseSet, Closing, SessionCommand};
use native_term_app::tab_menu::MenuRequest;
use native_term_app::{t, Core, SessionView, State};
use native_term_config::ops::{Editor, HostDraft};
use native_term_config::write::Writer;
use native_term_config::SessionTree;

use crate::dialogs::{
    ConfirmCloseMixed, ConfirmDelete, ConfirmForget, DropChoice, DropDialog, FolderDialog, HostDialog, Outcome,
};
use crate::icons;
use crate::import_dialog::ImportDialog;
use crate::key_dialog::KeyDialog;
use crate::options_dialog::{OptionsDialog, OptionsTarget};
use crate::plink_dialog::PlinkDialog;
use crate::send_dialog::SendDialog;
use crate::send_line::SendLine;
use crate::server_sessions::ServerSessionsDialog;
use crate::tab_list::TabList;
use crate::terminal_profile::ProfileSetup;
use crate::tree_view::{Activity, TreeAction, TreeView};
use crate::Setup;

/// `state.db` setting: the docked window stays out.
const PINNED_SETTING: &str = "dock_pinned";

enum Dialog {
    Host(Box<HostDialog>),
    Plink(Box<PlinkDialog>),
    Folder(FolderDialog),
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
}

/// Opening at least this many hosts that forward the ssh-agent is pointed out.
const AGENT_NOTICE_MIN: usize = 3;

/// What the right side shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Sessions,
    Tabs,
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
) -> Option<native_term_win::watch::FolderWatcher> {
    let dir = folders_dir(ssh_dir);
    if dir.starts_with(ssh_dir) {
        return None;
    }
    let (flag, wake) = (std::sync::Arc::clone(flag), ctx.clone());
    native_term_win::watch::FolderWatcher::start(&dir, false, move || {
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
    /// Keeps the `~/.ssh` watcher alive.
    _watcher: Option<native_term_win::watch::FolderWatcher>,
    /// And the one on the folder files, when they live elsewhere.
    folders_watcher: Option<native_term_win::watch::FolderWatcher>,
    /// "Move session folders": the new path being typed.
    folders_move: Option<String>,
    ssh_changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    loaded_from: Fingerprint,
    view_right: View,
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
    let home_ssh = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".ssh"));
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

impl App {
    pub fn new(ctx: &egui::Context, setup: Setup) -> App {
        let Setup { options, install, shim, core, data_dir, data_source, mut notices } = setup;
        let mut profile = ProfileSetup::new(install, shim, data_dir.join("backups"));
        if let Some(core) = &core {
            core.set_audit_dir(data_dir.join("audit"));
        }
        notices.extend(profile.fix_moved());
        if let Some(core) = &core {
            crate::dock::set_pinned(core.setting(PINNED_SETTING).as_deref() == Some("1"));
            apply_theme(ctx, core.setting(THEME_SETTING).as_deref());
            let repaint = ctx.clone();
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
        let watcher = native_term_win::watch::FolderWatcher::start(&options.ssh_dir, true, move || {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
            wake.request_repaint();
        })
        .ok();
        let folders_watcher = watch_folders(&options.ssh_dir, &ssh_changed, ctx);
        let loaded_from = fingerprint(&options.ssh_dir, &tree);
        let first_run = core.as_ref().is_some_and(|c| c.setting(crate::wizard::DONE_SETTING).is_none());
        let keys = crate::shortcut_ui::ShortcutUi::new(ctx, core.as_ref(), &profile.settings_json());
        App {
            core,
            _watcher: watcher,
            folders_watcher,
            folders_move: None,
            ssh_changed,
            loaded_from,
            view_right: View::Sessions,
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
            view: TreeView::default(),
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
            sync_roots: native_term_win::cloud::sync_roots(),
            putty_sessions: native_term_config::putty::has_sessions(),
            wizard: first_run.then(|| crate::wizard::Wizard::new(ctx)),
            securecrt: native_term_app::import::securecrt_config_path(),
        }
    }

    fn reload(&mut self) {
        self.tree = SessionTree::load(&self.ssh_dir);
        self.generation += 1;
        self.loaded_from = fingerprint(&self.ssh_dir, &self.tree);
        publish_hosts(&self.tree, self.core.as_ref());
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
                self.view.focus_search();
            }
            Command::AllTabs => {
                if global {
                    crate::shell::show_tabs();
                }
                self.view_right = View::Tabs;
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
                let dialog = PlinkDialog::new_session(file, &label).with_data_dir(&self.data_dir);
                self.dialog = Some(Dialog::Plink(Box::new(dialog)));
            }
            TreeAction::Edit(alias) => {
                if let Some((_, host)) = self.tree.find(&alias) {
                    self.dialog = Some(match &host.plink {
                        Some(session) => {
                            Dialog::Plink(Box::new(PlinkDialog::edit(session).with_data_dir(&self.data_dir)))
                        }
                        None => {
                            let folder = self.folder_persistent(&host.file);
                            let account = self
                                .editor
                                .effective(&alias)
                                .ok()
                                .and_then(|e| native_term_config::password::target(&e, None));
                            let (color, scheme) = self.folder_look(&host.file);
                            let set = self.folder_credential(&host.file);
                            let dialog = HostDialog::edit(&alias, &HostDraft::from_host(host))
                                .with_folder_default(folder)
                                .with_folder_look(color, scheme)
                                .with_credentials(set, crate::credential_sets::names())
                                .with_password(account);
                            Dialog::Host(Box::new(dialog))
                        }
                    });
                }
            }
            TreeAction::Options(alias) => {
                if let Some((_, host)) = self.tree.find(&alias) {
                    match self.editor.host_options(host) {
                        Ok(values) => {
                            let effective = self.editor.effective(&alias).unwrap_or_default();
                            let target = OptionsTarget::Host(alias.clone());
                            let dialog =
                                OptionsDialog::new(target, host.label(), &values, effective, self.editor.ssh());
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
                        let target = OptionsTarget::Folder(file);
                        let dialog = OptionsDialog::new(target, &label, &values, effective, self.editor.ssh());
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
                let _ = std::process::Command::new("explorer.exe").arg(&current).spawn();
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
                let _ = std::process::Command::new("explorer.exe").arg(&self.data_dir).spawn();
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
    fn dropped(&mut self, alias: &str, session: &str, paths: Vec<PathBuf>, text: &str) {
        let can_upload = self.tree.find(alias).is_some_and(|(_, host)| host.plink.is_none());
        // A drop ends with the mouse button released over the tab, having
        // gone down in another window (Explorer). Text that arrives
        // without that was typed or pasted, and is never acted on without
        // asking, however the question was answered before.
        let dropped_by_mouse = native_term_platform::windows_terminal::menu::since_drag_release()
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
            self.dropped(&alias, &session, paths, &text);
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
            MenuRequest::Rename(alias) => match self.tree.find(&alias) {
                Some((_, host)) => {
                    let folder = self.folder_persistent(&host.file);
                    let dialog = HostDialog::edit(&alias, &HostDraft::from_host(host)).with_folder_default(folder);
                    self.dialog = Some(Dialog::Host(Box::new(dialog)));
                }
                None => self.notices.push(t!("notice-not-saved", alias = alias.as_str())),
            },
            // handled above
            MenuRequest::Files { .. } | MenuRequest::Dropped { .. } => {}
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
        let facts = crate::wizard::Facts {
            terminal: self.profile.terminal_text(),
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
                    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
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

    fn show_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.as_mut() else { return };
        let done = match dialog {
            Dialog::CredentialSets(d) => matches!(d.show(ctx), Outcome::Cancel),
            Dialog::Host(d) => match d.show(ctx) {
                Outcome::Open => false,
                Outcome::Cancel => true,
                Outcome::Submit(draft) => {
                    let result = match (&d.alias, &d.file) {
                        (Some(alias), _) => match self.tree.find(alias) {
                            Some((_, host)) => self.editor.update_host(host, &draft).map_err(|e| e.to_string()),
                            None => Err(t!("error-host-gone", alias = alias.as_str())),
                        },
                        (None, Some(file)) => {
                            self.editor.create_host(&self.tree, file, &draft).map(|_| ()).map_err(|e| e.to_string())
                        }
                        (None, None) => Ok(()),
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
            Dialog::Plink(d) => match d.show(ctx) {
                Outcome::Open => false,
                Outcome::Cancel => true,
                Outcome::Submit(()) => {
                    let result = match (d.alias.clone(), d.file.clone()) {
                        (Some(alias), _) => match self.tree.find(&alias) {
                            Some((_, host)) => d
                                .session(&alias)
                                .and_then(|s| self.editor.update_plink(host, &s).map_err(|e| e.to_string())),
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
                                self.editor.add_plink(&file, &s).map_err(|e| e.to_string())
                            })
                        }
                        (None, None) => Ok(()),
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
                    let result = match &d.target {
                        OptionsTarget::Host(alias) => match self.tree.find(alias) {
                            Some((_, host)) => self.editor.set_host_options(host, &values).map_err(|e| e.to_string()),
                            None => Err(t!("error-host-gone", alias = alias.as_str())),
                        },
                        OptionsTarget::Folder(file) => match self.editor.set_folder_options(file, &values) {
                            Ok(own) => {
                                if !own.is_empty() {
                                    self.notices.push(t!("folder-options-own-tag", hosts = own.join(", ")));
                                }
                                Ok(())
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
    fn right_panel(&mut self, ui: &mut egui::Ui) {
        let Some(core) = self.core.clone() else { return };
        let open = core.sessions().iter().filter(|s| s.state.is_open()).count();
        ui.horizontal(|ui| {
            let sessions = egui::RichText::new(t!("sessions-heading", count = open)).heading();
            ui.selectable_value(&mut self.view_right, View::Sessions, sessions);
            let tabs = egui::RichText::new(icons::with(icons::TABS, t!("view-tabs"))).heading();
            ui.selectable_value(&mut self.view_right, View::Tabs, tabs).on_hover_text(t!("view-tabs-hint"));
        });
        match self.view_right {
            View::Sessions => {
                core.want_all_tabs(false);
                self.sessions_panel(ui, &core);
            }
            View::Tabs => {
                ui.separator();
                self.tab_list.show(ui, &core);
            }
        }
    }

    fn sessions_panel(&mut self, ui: &mut egui::Ui, core: &Core) {
        let core = core.clone();
        let sessions = core.sessions();
        ui.horizontal(|ui| {
            if ui.small_button(t!("sessions-clear-finished")).clicked() {
                core.clear_finished();
            }
            let unlocated = core.unlocated();
            if unlocated > 0
                && ui
                    .small_button(t!("sessions-locate", count = unlocated))
                    .on_hover_text(t!("sessions-locate-hint"))
                    .clicked()
            {
                core.locate();
            }
            // logged in, or kept in tmux on the server (sent there through it)
            let tmux = self.tmux_hosts();
            let reachable = sessions.iter().filter(|s| s.state == State::Connected || tmux.contains(&s.alias)).count();
            if ui.add_enabled(reachable > 0, egui::Button::new(t!("sessions-send-many")).small()).clicked()
                && self.dialog.is_none()
            {
                self.dialog = Some(Dialog::Send(Box::new(self.send_dialog(&core, &[]))));
            }
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
        ui.separator();
        if sessions.is_empty() {
            ui.label(t!("sessions-empty"));
            return;
        }
        let mut save = None;
        let mut send = None;
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
                match session_card(ui, &core, s, typed, tmux.contains(&s.alias)) {
                    Some(CardAction::Save(target)) => save = Some(target),
                    Some(CardAction::Send) => send = Some(s.id.clone()),
                    None => {}
                }
                ui.add_space(6.0);
            }
        });
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
            f.hosts.iter().map(move |h| crate::shell::HostEntry {
                alias: h.alias().to_string(),
                label: h.label().to_string(),
                hostname: h.target().to_string(),
                folder: folder.clone(),
                on_login: f.nt(h, "onlogin").map(str::to_string),
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
    let (preference, title_bar) = match setting {
        Some("light") => (egui::ThemePreference::Light, egui::SystemTheme::Light),
        Some("dark") => (egui::ThemePreference::Dark, egui::SystemTheme::Dark),
        _ => (egui::ThemePreference::System, egui::SystemTheme::SystemDefault),
    };
    ctx.set_theme(preference);
    *THEME.lock().unwrap_or_else(|e| e.into_inner()) = Some(preference);
    // the window's own title bar follows too
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(title_bar));
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
                }
            }
        });
    });
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
            if ui.selectable_label(setting.is_none(), t!("language-system")).clicked() {
                core.set_language_setting(None);
            }
            for (id, native) in &languages {
                let id = id.to_string();
                if ui.selectable_label(setting.as_deref() == Some(id.as_str()), *native).clicked() {
                    core.set_language_setting(Some(&id));
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
fn session_card(
    ui: &mut egui::Ui,
    core: &Core,
    s: &SessionView,
    typed: Option<native_term_app::quick::QuickTarget>,
    tmux: bool,
) -> Option<CardAction> {
    let mut action = None;
    let color = state_color(ui, &s.state);
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
                let time = native_term_win::local_time_of_day(since);
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
    action
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
        }
        for (command, global) in self.keys.take(ctx) {
            self.run_shortcut(command, global);
        }
        self.reload_if_changed();
        for request in crate::shell::take_requests() {
            self.handle_menu_request(request);
        }
        if crate::shell::take_show_tabs() {
            self.view_right = View::Tabs;
            self.tab_list.focus_search();
        }
        egui::Panel::top("status").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.toggle_value(&mut self.show_settings, icons::with(icons::SETTINGS, t!("settings-toggle")));
                if let Some(edge) = crate::dock::docked_edge() {
                    let mut pinned = crate::dock::pinned();
                    let toggle = ui
                        .toggle_value(&mut pinned, icons::with(icons::PIN, t!("dock-pin")))
                        .on_hover_text(t!("dock-pin-hint", edge = edge.name()));
                    if toggle.changed() {
                        crate::dock::set_pinned(pinned);
                        if let Some(core) = &self.core {
                            core.set_setting(PINNED_SETTING, if pinned { "1" } else { "0" });
                        }
                    }
                }
                if ui.button(icons::with(icons::IMPORT, t!("import-securecrt-button"))).clicked()
                    && self.dialog.is_none()
                {
                    self.dialog =
                        Some(Dialog::Import(Box::new(ImportDialog::new(self.ssh_dir.clone(), self.data_dir.clone()))));
                }
                if self.putty_sessions
                    && ui.button(icons::with(icons::IMPORT, t!("import-putty-button"))).clicked()
                    && self.dialog.is_none()
                {
                    let dialog = ImportDialog::putty(self.ssh_dir.clone(), self.data_dir.clone(), &self.egui_ctx);
                    self.dialog = Some(Dialog::Import(Box::new(dialog)));
                }
            });
            if self.show_settings {
                ui.group(|ui| {
                    self.profile.settings_ui(ui, &mut self.notices);
                    if let Some(core) = &self.core {
                        ui.separator();
                        let mut auto = core.auto_reconnect();
                        if ui.checkbox(&mut auto, t!("auto-reconnect-setting")).changed() {
                            core.set_auto_reconnect(auto);
                        }
                        let mut close = core.close_on_exit();
                        let response = ui
                            .checkbox(&mut close, t!("close-on-exit-setting"))
                            .on_hover_text(t!("close-on-exit-hint"));
                        if response.changed() {
                            core.set_setting(native_term_app::CLOSE_ON_EXIT_SETTING, if close { "1" } else { "0" });
                        }
                        let mut ask = self.remembered_drop().is_none();
                        let response = ui.checkbox(&mut ask, t!("drop-ask-setting")).on_hover_text(t!("drop-ask-hint"));
                        if response.changed() && ask {
                            core.set_setting("drop.action", "");
                        }
                        language_choice(ui, core);
                        theme_choice(ui, core);
                    }
                    ui.separator();
                    self.agent.settings_ui(ui, &self.ssh_dir, self.core.as_ref());
                    if ui.small_button(t!("button-check-again")).clicked() {
                        self.agent.refresh(&self.ssh_dir, ui.ctx());
                    }
                    ui.separator();
                    self.keys.settings_ui(ui, self.core.as_ref());
                    let sets = ui.button(t!("cred-sets-button")).on_hover_text(t!("cred-sets-intro"));
                    if sets.clicked() && self.dialog.is_none() {
                        let dialog = crate::credential_sets::CredentialSetsDialog::new(&self.tree);
                        self.dialog = Some(Dialog::CredentialSets(Box::new(dialog)));
                    }
                    ui.separator();
                    self.folders_ui(ui);
                    self.data_dir_ui(ui);
                    ui.separator();
                    if ui.button(t!("wizard-open")).clicked() && self.wizard.is_none() {
                        self.wizard = Some(crate::wizard::Wizard::new(ui.ctx()));
                    }
                });
            }
            self.profile.banner(ui, &mut self.notices);
            self.agent.banner(ui, self.core.as_ref(), &mut self.show_settings);
            self.storage.banner(ui);
            if !self.notices.is_empty() {
                let mut clear = false;
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x9a, 0x1a), self.notices.join("  ·  "));
                    clear = ui.small_button("×").clicked();
                });
                if clear {
                    self.notices.clear();
                }
            }
        });
        let recent = self.recent();
        // the best state per host, for the dots in the tree
        let mut activity: HashMap<String, Activity> = HashMap::new();
        for s in self.core.as_ref().map(|c| c.sessions()).unwrap_or_default() {
            if let Some(a) = Activity::of(&s.state) {
                let best = activity.entry(s.alias).or_insert(a);
                *best = (*best).max(a);
            }
        }
        let mut actions = Vec::new();
        let no_group_send = self.no_group_send();
        egui::Panel::left("tree").resizable(true).default_size(320.0).size_range(220.0..=640.0).show_inside(ui, |ui| {
            if let Some(core) = &self.core {
                egui::Panel::bottom("send-line").show_inside(ui, |ui| {
                    ui.add_space(4.0);
                    self.send_line.show(ui, core, &no_group_send);
                    ui.add_space(2.0);
                });
            }
            actions = self.view.show(ui, &self.tree, self.generation, &recent, &activity);
        });
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
        egui::CentralPanel::default().show_inside(ui, |ui| self.right_panel(ui));
        self.show_dialog(ctx);
        self.show_wizard(ctx);
    }
}
