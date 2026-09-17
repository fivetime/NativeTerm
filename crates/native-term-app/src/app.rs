//! The main window: the session tree, the open sessions, and the dialogs
//! that edit sessions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use native_term_app::{t, Core, SessionView, State};
use native_term_config::ops::{Editor, HostDraft};
use native_term_config::write::Writer;
use native_term_config::SessionTree;

use crate::dialogs::{ConfirmDelete, ConfirmForget, FolderDialog, HostDialog, Outcome};
use crate::import_dialog::ImportDialog;
use crate::key_dialog::KeyDialog;
use crate::options_dialog::OptionsDialog;
use crate::send_dialog::SendDialog;
use crate::terminal_profile::ProfileSetup;
use crate::icons;
use crate::tab_list::TabList;
use crate::tree_view::{Activity, TreeAction, TreeView};
use crate::Setup;

/// `state.db` setting: the docked window stays out.
const PINNED_SETTING: &str = "dock_pinned";

enum Dialog {
    Host(HostDialog),
    Folder(FolderDialog),
    Delete(ConfirmDelete),
    Forget(ConfirmForget),
    Key(Box<KeyDialog>),
    Options(Box<OptionsDialog>),
    Import(Box<ImportDialog>),
    Send(Box<SendDialog>),
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

fn fingerprint(ssh_dir: &Path, tree: &SessionTree) -> Fingerprint {
    let mut files: Vec<PathBuf> = vec![ssh_dir.join("config")];
    files.extend(tree.folders().map(|f| f.file.clone()));
    if let Ok(entries) = std::fs::read_dir(ssh_dir.join("config.d")) {
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
    ssh_changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    loaded_from: Fingerprint,
    view_right: View,
    tab_list: TabList,
    agent: crate::agent::AgentCheck,
    tree: SessionTree,
    /// Bumped on every reload.
    generation: u64,
    ssh_dir: PathBuf,
    data_dir: PathBuf,
    editor: Editor,
    view: TreeView,
    dialog: Option<Dialog>,
    profile: ProfileSetup,
    show_settings: bool,
    notices: Vec<String>,
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
    if home_ssh.as_deref().is_some_and(|h| h.to_string_lossy().eq_ignore_ascii_case(&ssh_dir.to_string_lossy())) {
        Editor::new(ssh_dir, writer, ssh)
    } else {
        Editor::for_directory(ssh_dir, writer, ssh)
    }
}

impl App {
    pub fn new(ctx: &egui::Context, setup: Setup) -> App {
        let Setup { options, install, shim, core, data_dir, mut notices } = setup;
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
            let send = move |id: &str| {
                crate::shell::send_to(id);
                repaint.request_repaint();
            };
            if let Err(e) = core.start_tab_menu(send) {
                notices.push(t!("notice-tab-menu-unavailable", error = e.to_string()));
            }
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
        let loaded_from = fingerprint(&options.ssh_dir, &tree);
        let first_run = core.as_ref().is_some_and(|c| c.setting(crate::wizard::DONE_SETTING).is_none());
        App {
            core,
            _watcher: watcher,
            ssh_changed,
            loaded_from,
            view_right: View::Sessions,
            tab_list: TabList::default(),
            agent: {
                let check = crate::agent::AgentCheck::default();
                check.refresh(&options.ssh_dir, ctx);
                check
            },
            tree,
            generation: 0,
            editor: editor_for(&options.ssh_dir, &data_dir),
            data_dir,
            ssh_dir: options.ssh_dir,
            view: TreeView::default(),
            dialog: None,
            profile,
            show_settings: false,
            notices,
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
        self.core
            .as_ref()
            .and_then(|c| c.registry())
            .and_then(|r| r.recent(20).ok())
            .map(|list| list.into_iter().map(|u| u.alias).collect())
            .unwrap_or_default()
    }

    fn folder_label(&self, file: &Path) -> String {
        self.tree
            .folders()
            .find(|f| f.file == file)
            .map(|f| if f.name.is_empty() { t!("tree-main-config") } else { f.label().to_string() })
            .unwrap_or_else(|| file.display().to_string())
    }

    fn handle(&mut self, action: TreeAction) {
        match action {
            TreeAction::Open(hosts, target) => {
                if let (Some(core), false) = (&self.core, hosts.is_empty()) {
                    core.open(&hosts, target);
                    if hosts.len() >= AGENT_NOTICE_MIN {
                        let aliases: Vec<String> = hosts.iter().map(|h| h.alias.clone()).collect();
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
                self.dialog = Some(Dialog::Host(HostDialog::new_host(file, &label)));
            }
            TreeAction::Edit(alias) => {
                if let Some((_, host)) = self.tree.find(&alias) {
                    self.dialog = Some(Dialog::Host(HostDialog::edit(&alias, &HostDraft::from_host(host))));
                }
            }
            TreeAction::Options(alias) => {
                if let Some((_, host)) = self.tree.find(&alias) {
                    match self.editor.host_options(host) {
                        Ok(values) => {
                            let effective = self.editor.effective(&alias).unwrap_or_default();
                            let dialog = OptionsDialog::new(&alias, host.label(), &values, effective, self.editor.ssh());
                            self.dialog = Some(Dialog::Options(Box::new(dialog)));
                        }
                        Err(e) => self.notices.push(e.to_string()),
                    }
                }
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

    fn show_wizard(&mut self, ctx: &egui::Context) {
        use crate::wizard::WizardAction;
        // a dialog the wizard opened comes first; the wizard waits behind it
        if self.dialog.is_some() {
            return;
        }
        let Some(wizard) = self.wizard.as_mut() else { return };
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
            can_open_tabs: self.core.is_some(),
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
                    self.dialog = Some(Dialog::Import(Box::new(ImportDialog::new(self.ssh_dir.clone(), self.data_dir.clone()))));
                }
                WizardAction::ImportPutty => {
                    let dialog = ImportDialog::putty(self.ssh_dir.clone(), self.data_dir.clone(), &self.tree);
                    self.dialog = Some(Dialog::Import(Box::new(dialog)));
                }
                WizardAction::CreateKey => {
                    if let Some(core) = &self.core {
                        let path = self.ssh_dir.join("id_ed25519").display().to_string();
                        if let Err(e) = core.terminal().open_tool(&t!("key-create-tab"), &["--create-key".into(), path]) {
                            self.notices.push(e.to_string());
                        }
                    }
                }
                WizardAction::InstallKeys => {
                    let hosts: Vec<(String, String)> =
                        self.tree.hosts().map(|(_, h)| (h.alias().to_string(), h.label().to_string())).collect();
                    if !hosts.is_empty() {
                        self.dialog = Some(Dialog::Key(Box::new(KeyDialog::new(hosts, &self.ssh_dir))));
                    }
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
            Dialog::Host(d) => match d.show(ctx) {
                Outcome::Open => false,
                Outcome::Cancel => true,
                Outcome::Submit(draft) => {
                    let result = match (&d.alias, &d.file) {
                        (Some(alias), _) => match self.tree.find(alias) {
                            Some((_, host)) => self.editor.update_host(host, &draft).map_err(|e| e.to_string()),
                            None => Err(t!("error-host-gone", alias = alias.as_str())),
                        },
                        (None, Some(file)) => self.editor.create_host(&self.tree, file, &draft).map(|_| ()).map_err(|e| e.to_string()),
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
                    let result = match self.tree.find(&d.alias) {
                        Some((_, host)) => self.editor.set_host_options(host, &values).map_err(|e| e.to_string()),
                        None => Err(t!("error-host-gone", alias = d.alias.as_str())),
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
            let logged_in = sessions.iter().filter(|s| s.state == State::Connected).count();
            if ui.add_enabled(logged_in > 0, egui::Button::new(t!("sessions-send-many")).small()).clicked() && self.dialog.is_none() {
                self.dialog = Some(Dialog::Send(Box::new(SendDialog::new(&core, &[], &self.data_dir))));
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
                    for s in waiting.iter().filter(|s| !s.locked) {
                        core.close(&s.id);
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
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for s in &sessions {
                // typed by hand, not in the tree: offer to save it
                let typed = self
                    .tree
                    .find(&s.alias)
                    .is_none()
                    .then(|| native_term_app::quick::from_destination(&s.alias))
                    .flatten();
                match session_card(ui, &core, s, typed) {
                    Some(CardAction::Save(target)) => save = Some(target),
                    Some(CardAction::Send) => send = Some(s.id.clone()),
                    None => {}
                }
                ui.add_space(6.0);
            }
        });
        if let (Some(id), None) = (send, &self.dialog) {
            self.dialog = Some(Dialog::Send(Box::new(SendDialog::new(&core, &[id], &self.data_dir))));
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
            self.dialog = Some(Dialog::Host(HostDialog::new_host_from(main, &folder, &draft)));
        }
    }
}

/// The saved hosts, for the floating button's search.
fn publish_hosts(tree: &SessionTree, core: Option<&Core>) {
    if let Some(core) = core {
        core.set_host_labels(tree.hosts().map(|(_, h)| (h.alias().to_string(), h.label().to_string())).collect());
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
        State::LoginFailed(_) | State::Disconnected(_) | State::Failed(_) => egui::Color32::from_rgb(0xd0, 0x3a, 0x3a),
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
                    ui.weak(t!("session-not-located"));
                }
                None => {}
            }
            ui.add_space(12.0);
            let open = s.state.is_open();
            if ui.add_enabled(s.location.is_some(), egui::Button::new(t!("button-focus")).small()).clicked() {
                core.focus(&s.id);
            }
            let label = if s.state == State::Waiting { t!("button-connect") } else { t!("button-reconnect") };
            if ui.add_enabled(open && s.linked && s.state.can_connect(), egui::Button::new(label).small()).clicked() {
                core.connect(&s.id);
            }
            let live = matches!(s.state, State::Connecting | State::Connected);
            if ui.add_enabled(open && s.linked && live, egui::Button::new(t!("button-disconnect")).small()).clicked() {
                core.disconnect(&s.id);
            }
            if ui.add_enabled(open && !s.locked, egui::Button::new(t!("button-close")).small()).clicked() {
                core.close(&s.id);
            }
            let lock = if s.locked { icons::with(icons::UNLOCK, t!("session-unlock")) } else { icons::with(icons::LOCK, t!("session-lock")) };
            if ui.add_enabled(open, egui::Button::new(lock).small()).on_hover_text(t!("session-lock-hint")).clicked() {
                core.set_locked(&s.id, !s.locked);
            }
            let ready = s.state == State::Connected && s.linked;
            if ui.add_enabled(ready, egui::Button::new(icons::with(icons::SEND, t!("session-send"))).small()).clicked() {
                action = Some(CardAction::Send);
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
    fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        if let Some(core) = &self.core {
            self.notices.extend(core.take_notices());
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            self.view.focus_search();
        }
        self.reload_if_changed();
        if let Some(id) = crate::shell::take_send_to() {
            if let (Some(core), None) = (&self.core, &self.dialog) {
                self.dialog = Some(Dialog::Send(Box::new(SendDialog::new(core, &[id], &self.data_dir))));
            }
        }
        if crate::shell::take_show_tabs() {
            self.view_right = View::Tabs;
            self.tab_list.focus_search();
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::T)) {
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
                if ui.button(icons::with(icons::IMPORT, t!("import-securecrt-button"))).clicked() && self.dialog.is_none() {
                    self.dialog = Some(Dialog::Import(Box::new(ImportDialog::new(self.ssh_dir.clone(), self.data_dir.clone()))));
                }
                if self.putty_sessions && ui.button(icons::with(icons::IMPORT, t!("import-putty-button"))).clicked() && self.dialog.is_none() {
                    let dialog = ImportDialog::putty(self.ssh_dir.clone(), self.data_dir.clone(), &self.tree);
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
                        language_choice(ui, core);
                        theme_choice(ui, core);
                    }
                    ui.separator();
                    self.agent.settings_ui(ui, &self.ssh_dir, self.core.as_ref());
                    if ui.small_button(t!("button-check-again")).clicked() {
                        self.agent.refresh(&self.ssh_dir, ui.ctx());
                    }
                    ui.separator();
                    if ui.button(t!("wizard-open")).clicked() && self.wizard.is_none() {
                        self.wizard = Some(crate::wizard::Wizard::new(ui.ctx()));
                    }
                });
            }
            self.profile.banner(ui, &mut self.notices);
            self.agent.banner(ui, self.core.as_ref(), &mut self.show_settings);
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
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(320.0)
            .size_range(220.0..=640.0)
            .show_inside(ui, |ui| actions = self.view.show(ui, &self.tree, self.generation, &recent, &activity));
        for action in actions {
            self.handle(action);
        }
        egui::CentralPanel::default().show_inside(ui, |ui| self.right_panel(ui));
        self.show_dialog(ctx);
        self.show_wizard(ctx);
    }
}
