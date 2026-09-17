//! The main window: the session tree, the open sessions, and the dialogs
//! that edit sessions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use native_term_app::{t, Core, SessionView, State};
use native_term_config::ops::{Editor, HostDraft};
use native_term_config::write::Writer;
use native_term_config::SessionTree;

use crate::dialogs::{ConfirmDelete, FolderDialog, HostDialog, Outcome};
use crate::import_dialog::ImportDialog;
use crate::terminal_profile::ProfileSetup;
use crate::icons;
use crate::tree_view::{Activity, TreeAction, TreeView};
use crate::Setup;

/// `state.db` setting: the docked window stays out.
const PINNED_SETTING: &str = "dock_pinned";

enum Dialog {
    Host(HostDialog),
    Folder(FolderDialog),
    Delete(ConfirmDelete),
    Import(Box<ImportDialog>),
}

pub struct App {
    core: Option<Core>,
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
        let mut profile = ProfileSetup::new(install, shim);
        notices.extend(profile.fix_moved());
        if let Some(core) = &core {
            crate::dock::set_pinned(core.setting(PINNED_SETTING).as_deref() == Some("1"));
            apply_theme(ctx, core.setting(THEME_SETTING).as_deref());
            let ctx = ctx.clone();
            core.set_repaint(move || ctx.request_repaint());
            if let Err(e) = core.start_tab_menu() {
                notices.push(t!("notice-tab-menu-unavailable", error = e.to_string()));
            }
        }
        let tree = SessionTree::load(&options.ssh_dir);
        App {
            core,
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
        }
    }

    fn reload(&mut self) {
        self.tree = SessionTree::load(&self.ssh_dir);
        self.generation += 1;
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

    fn sessions_panel(&mut self, ui: &mut egui::Ui) {
        let Some(core) = self.core.clone() else { return };
        let sessions = core.sessions();
        ui.horizontal(|ui| {
            ui.heading(t!("sessions-heading", count = sessions.iter().filter(|s| s.state.is_open()).count()));
            if ui.button(t!("sessions-clear-finished")).clicked() {
                core.clear_finished();
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
                    for s in &waiting {
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
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for s in &sessions {
                // typed by hand, not in the tree: offer to save it
                let typed = self
                    .tree
                    .find(&s.alias)
                    .is_none()
                    .then(|| native_term_app::quick::from_destination(&s.alias))
                    .flatten();
                if let Some(target) = session_card(ui, &core, s, typed) {
                    save = Some(target);
                }
                ui.add_space(6.0);
            }
        });
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

/// `state.db` setting: `light`, `dark`, or absent for the system's.
pub const THEME_SETTING: &str = "theme";

pub fn apply_theme(ctx: &egui::Context, setting: Option<&str>) {
    let (preference, title_bar) = match setting {
        Some("light") => (egui::ThemePreference::Light, egui::SystemTheme::Light),
        Some("dark") => (egui::ThemePreference::Dark, egui::SystemTheme::Dark),
        _ => (egui::ThemePreference::System, egui::SystemTheme::SystemDefault),
    };
    ctx.set_theme(preference);
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

/// One open session as a card: name and state, then where its tab is and
/// what can be done. Returns the target when "Save…" was clicked.
fn session_card(
    ui: &mut egui::Ui,
    core: &Core,
    s: &SessionView,
    typed: Option<native_term_app::quick::QuickTarget>,
) -> Option<native_term_app::quick::QuickTarget> {
    let mut save = None;
    let color = state_color(ui, &s.state);
    egui::Frame::group(ui.style()).corner_radius(6.0).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.5, color);
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
            if ui.add_enabled(open, egui::Button::new(t!("button-close")).small()).clicked() {
                core.close(&s.id);
            }
            if let Some(target) = typed {
                let button = egui::Button::new(icons::with(icons::SAVE, t!("quick-save"))).small();
                if ui.add(button).on_hover_text(t!("quick-save-hint")).clicked() {
                    save = Some(target);
                }
            }
        });
    });
    save
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
                });
            }
            self.profile.banner(ui, &mut self.notices);
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
        egui::CentralPanel::default().show_inside(ui, |ui| self.sessions_panel(ui));
        self.show_dialog(ctx);
    }
}
