//! The main window: the session tree, the open sessions, and the dialogs
//! that edit sessions.

use std::path::{Path, PathBuf};

use native_term_app::{Core, SessionView, State};
use native_term_config::ops::{Editor, HostDraft};
use native_term_config::write::Writer;
use native_term_config::SessionTree;

use crate::dialogs::{ConfirmDelete, FolderDialog, HostDialog, Outcome};
use crate::import_dialog::ImportDialog;
use crate::terminal_profile::ProfileSetup;
use crate::tree_view::{TreeAction, TreeView};
use crate::Setup;

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
            let ctx = ctx.clone();
            core.set_repaint(move || ctx.request_repaint());
            if let Err(e) = core.start_tab_menu() {
                notices.push(format!("NativeTerm's tab menu isn't available: {e}"));
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
            .map(|f| if f.name.is_empty() { "~/.ssh/config".to_string() } else { f.label().to_string() })
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
                    None => Err(format!("{alias} is gone")),
                };
                if let Err(e) = result {
                    self.notices.push(format!("Moving {alias} failed: {e}"));
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
                            None => Err(format!("{alias} is gone (changed outside NativeTerm?)")),
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
            ui.heading(format!("Open sessions ({})", sessions.iter().filter(|s| s.state.is_open()).count()));
            if ui.button("Clear finished").clicked() {
                core.clear_finished();
            }
        });
        ui.separator();
        if sessions.is_empty() {
            ui.label("Double-click a host, or right-click it or a folder for more.");
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("sessions").striped(true).num_columns(4).show(ui, |ui| {
                ui.strong("Session");
                ui.strong("State");
                ui.strong("Tab");
                ui.strong("");
                ui.end_row();
                for s in &sessions {
                    session_row(ui, &core, s);
                    ui.end_row();
                }
            });
        });
    }
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

fn session_row(ui: &mut egui::Ui, core: &Core, s: &SessionView) {
    let label = ui.label(&s.label);
    if s.label != s.alias {
        label.on_hover_text(&s.alias);
    }
    let mut state = s.state.describe();
    if s.attempt > 1 && s.state.is_open() {
        state.push_str(&format!(" · attempt {}", s.attempt));
    }
    ui.colored_label(state_color(ui, &s.state), state);
    match &s.location {
        Some(l) => {
            let mut text = format!("window {} · tab {}", l.window_number, l.tab_index + 1);
            if l.selected {
                text.push_str(" · selected");
            }
            if l.mixed {
                text.push_str(" · split");
            }
            let response = ui.label(text);
            if l.title != s.label {
                response.on_hover_text(format!("current title: {}", l.title));
            }
        }
        None if s.state.is_open() => {
            ui.weak("not located");
        }
        None => {
            ui.label("");
        }
    }
    ui.horizontal(|ui| {
        let open = s.state.is_open();
        if ui.add_enabled(s.location.is_some(), egui::Button::new("Focus")).clicked() {
            core.focus(&s.id);
        }
        let label = if s.state == State::Waiting { "Connect" } else { "Reconnect" };
        if ui.add_enabled(open && s.linked && s.state.can_connect(), egui::Button::new(label)).clicked() {
            core.connect(&s.id);
        }
        let live = matches!(s.state, State::Connecting | State::Connected);
        if ui.add_enabled(open && s.linked && live, egui::Button::new("Disconnect")).clicked() {
            core.disconnect(&s.id);
        }
        if ui.add_enabled(open, egui::Button::new("Close")).clicked() {
            core.close(&s.id);
        }
    });
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
                ui.toggle_value(&mut self.show_settings, "⚙ Settings");
                if ui.button("Import from SecureCRT…").clicked() && self.dialog.is_none() {
                    self.dialog = Some(Dialog::Import(Box::new(ImportDialog::new(self.ssh_dir.clone(), self.data_dir.clone()))));
                }
            });
            if self.show_settings {
                ui.group(|ui| self.profile.settings_ui(ui, &mut self.notices));
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
        let mut actions = Vec::new();
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(320.0)
            .show_inside(ui, |ui| actions = self.view.show(ui, &self.tree, self.generation, &recent));
        for action in actions {
            self.handle(action);
        }
        egui::CentralPanel::default().show_inside(ui, |ui| self.sessions_panel(ui));
        self.show_dialog(ctx);
    }
}
