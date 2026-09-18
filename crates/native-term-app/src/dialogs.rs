//! Modal-ish windows for editing sessions. Each returns what the user
//! decided; the app performs it and shows errors back in the dialog.

use std::path::PathBuf;

use native_term_app::t;
use native_term_config::ops::HostDraft;
use native_term_config::password::{Target, REFUSED};
use native_term_win::credentials::{self, Saved};

pub enum Outcome<T> {
    Open,
    Cancel,
    Submit(T),
}

/// New or edited host.
pub struct HostDialog {
    pub title: String,
    /// `Some(alias)` when editing.
    pub alias: Option<String>,
    /// Target file for a new host.
    pub file: Option<PathBuf>,
    label: String,
    hostname: String,
    user: String,
    port: String,
    proxy_jump: String,
    identity_files: String,
    note: String,
    on_login: String,
    /// The host's own `NativeTermPersistent`; `None` follows the folder.
    persistent: Option<String>,
    /// The folder's default, shown with "as the folder".
    folder_persistent: Option<String>,
    /// The account's saved password (editing a saved host only).
    password: Option<PasswordField>,
    pub error: Option<String>,
}

/// The optional saved password of the host's account: written to and
/// removed from Credential Manager right away, never kept here.
struct PasswordField {
    target: Target,
    state: PasswordState,
    typed: String,
    message: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PasswordState {
    None,
    Saved,
    Refused,
}

impl PasswordField {
    fn new(target: Target) -> PasswordField {
        let state = match credentials::read(&target.name) {
            Ok(Some(saved)) if saved.comment == REFUSED => PasswordState::Refused,
            Ok(Some(_)) => PasswordState::Saved,
            _ => PasswordState::None,
        };
        PasswordField { target, state, typed: String::new(), message: None }
    }

    fn show(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.label(egui::RichText::new(t!("password-title")).strong());
        let (text, color) = match self.state {
            PasswordState::None => (t!("password-none"), None),
            PasswordState::Saved => (t!("password-saved", target = self.target.name.as_str()), None),
            PasswordState::Refused => (t!("password-refused"), Some(egui::Color32::from_rgb(0xd0, 0x3a, 0x3a))),
        };
        match color {
            Some(c) => ui.colored_label(c, text),
            None => ui.weak(text),
        };
        ui.horizontal(|ui| {
            let account = format!("{}@{}", self.target.user, self.target.host);
            let hint = t!("password-hint", account = account.as_str());
            let field = ui.add(egui::TextEdit::singleline(&mut self.typed).password(true).hint_text(hint).desired_width(200.0));
            no_ime(&field);
            if ui.add_enabled(!self.typed.is_empty(), egui::Button::new(t!("password-save"))).clicked() {
                let saved = Saved { user: self.target.user.clone(), secret: std::mem::take(&mut self.typed), comment: String::new() };
                let result = credentials::write(&self.target.name, &saved);
                drop(saved);
                self.message = Some(match result {
                    Ok(()) => {
                        self.state = PasswordState::Saved;
                        t!("password-stored")
                    }
                    Err(e) => e.to_string(),
                });
            }
            if self.state != PasswordState::None && ui.button(t!("password-remove")).clicked() {
                self.message = Some(match credentials::delete(&self.target.name) {
                    Ok(_) => {
                        self.state = PasswordState::None;
                        t!("password-removed")
                    }
                    Err(e) => e.to_string(),
                });
            }
        });
        if let Some(message) = &self.message {
            ui.weak(message);
        }
        ui.weak(t!("password-warning"));
    }
}

fn opt(text: &str) -> Option<String> {
    let t = text.trim();
    (!t.is_empty()).then(|| t.to_string())
}

impl HostDialog {
    pub fn new_host(file: PathBuf, folder: &str) -> HostDialog {
        HostDialog::from_draft(t!("host-new-title", folder = folder), None, Some(file), &HostDraft::default())
    }

    /// A new host, filled in (saving a quick connect).
    pub fn new_host_from(file: PathBuf, folder: &str, draft: &HostDraft) -> HostDialog {
        HostDialog::from_draft(t!("host-new-title", folder = folder), None, Some(file), draft)
    }

    pub fn edit(alias: &str, draft: &HostDraft) -> HostDialog {
        HostDialog::from_draft(t!("host-edit-title", alias = alias), Some(alias.to_string()), None, draft)
    }

    fn from_draft(title: String, alias: Option<String>, file: Option<PathBuf>, d: &HostDraft) -> HostDialog {
        HostDialog {
            title,
            alias,
            file,
            label: d.label.clone(),
            hostname: d.hostname.clone(),
            user: d.user.clone().unwrap_or_default(),
            port: d.port.map(|p| p.to_string()).unwrap_or_default(),
            proxy_jump: d.proxy_jump.clone().unwrap_or_default(),
            identity_files: d.identity_files.join("\n"),
            note: d.note.clone().unwrap_or_default(),
            on_login: d.on_login.clone().unwrap_or_default(),
            persistent: d.persistent.clone(),
            folder_persistent: None,
            password: None,
            error: None,
        }
    }

    /// Offer a saved password for this account (`ssh -G`'s user, host,
    /// port); a new host has none yet.
    pub fn with_password(mut self, target: Option<Target>) -> HostDialog {
        self.password = target.map(PasswordField::new);
        self
    }

    /// The folder's `NativeTermPersistent`, for "as the folder (…)".
    pub fn with_folder_default(mut self, value: Option<String>) -> HostDialog {
        self.folder_persistent = value;
        self
    }

    fn draft(&self) -> Result<HostDraft, String> {
        let port = match self.port.trim() {
            "" => None,
            p => Some(p.parse::<u16>().map_err(|_| t!("host-bad-port", port = p))?),
        };
        let hostname = self.hostname.trim().to_string();
        let label = if self.label.trim().is_empty() { hostname.clone() } else { self.label.trim().to_string() };
        Ok(HostDraft {
            label,
            hostname,
            user: opt(&self.user),
            port,
            proxy_jump: opt(&self.proxy_jump),
            identity_files: self.identity_files.lines().filter_map(opt).collect(),
            note: opt(&self.note),
            on_login: opt(&self.on_login),
            persistent: self.persistent.clone(),
        })
    }

    /// The choices for "keep on the server": (stored value, text).
    fn persistent_choices(&self) -> Vec<(Option<String>, String)> {
        let folder = match self.folder_persistent.as_deref() {
            Some(p @ ("tmux" | "screen")) => p.to_string(),
            _ => t!("persistent-off"),
        };
        vec![
            (None, t!("persistent-folder", value = folder.as_str())),
            (Some("tmux".into()), "tmux".into()),
            (Some("screen".into()), "screen".into()),
            (Some("off".into()), t!("persistent-off")),
        ]
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<HostDraft> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        egui::Window::new(self.title.clone())
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("host-fields").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    let field = |ui: &mut egui::Ui, name: String, value: &mut String, hint: String| {
                        ui.label(name);
                        ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(280.0));
                        ui.end_row();
                    };
                    field(ui, t!("field-name"), &mut self.label, t!("field-name-hint"));
                    field(ui, t!("field-host"), &mut self.hostname, t!("field-host-hint"));
                    field(ui, t!("field-user"), &mut self.user, t!("field-user-hint"));
                    field(ui, t!("field-port"), &mut self.port, "22".into());
                    field(ui, t!("field-jump"), &mut self.proxy_jump, t!("field-jump-hint"));
                    ui.label(t!("field-keys"));
                    ui.add(
                        egui::TextEdit::multiline(&mut self.identity_files)
                            .hint_text(t!("field-keys-hint"))
                            .desired_rows(2)
                            .desired_width(280.0),
                    );
                    ui.end_row();
                    field(ui, t!("field-note"), &mut self.note, t!("field-note-hint"));
                    field(ui, t!("field-on-login"), &mut self.on_login, t!("field-on-login-hint"));
                    ui.label(t!("field-persistent")).on_hover_text(t!("field-persistent-hint"));
                    let choices = self.persistent_choices();
                    let current = choices.iter().find(|(v, _)| *v == self.persistent).map(|(_, t)| t.clone()).unwrap_or_default();
                    egui::ComboBox::from_id_salt("host-persistent").selected_text(current).width(280.0).show_ui(ui, |ui| {
                        for (value, text) in choices {
                            ui.selectable_value(&mut self.persistent, value, text);
                        }
                    });
                    ui.end_row();
                });
                if let Some(alias) = &self.alias {
                    ui.weak(t!("host-alias-kept", alias = alias.as_str()));
                }
                match &mut self.password {
                    Some(field) => field.show(ui),
                    None if self.alias.is_none() => {
                        ui.separator();
                        ui.weak(t!("password-after-save"));
                    }
                    None => {}
                }
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x3a, 0x3a), error);
                }
                ui.horizontal(|ui| {
                    let ready = !self.hostname.trim().is_empty();
                    if ui.add_enabled(ready, egui::Button::new(t!("button-save"))).clicked() {
                        match self.draft() {
                            Ok(d) => outcome = Outcome::Submit(d),
                            Err(e) => self.error = Some(e),
                        }
                    }
                    if ui.button(t!("button-cancel")).clicked() {
                        outcome = Outcome::Cancel;
                    }
                });
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }
}

/// A password field keeps the input method off while it has the focus, as
/// Windows' own password boxes do: an IME in Chinese mode would otherwise
/// turn the typed letters into candidates (seen with Sogou pinyin).
/// eframe allows the IME exactly when the frame's output asks for it.
pub fn no_ime(field: &egui::Response) {
    if field.has_focus() {
        field.ctx.output_mut(|o| o.ime = None);
    }
}

/// A folder name (new folder, or rename).
pub struct FolderDialog {
    pub title: String,
    /// `Some(file)` when renaming.
    pub file: Option<PathBuf>,
    name: String,
    pub error: Option<String>,
}

impl FolderDialog {
    pub fn new_folder() -> FolderDialog {
        FolderDialog { title: t!("folder-new-title"), file: None, name: String::new(), error: None }
    }

    pub fn rename(file: PathBuf, current: &str) -> FolderDialog {
        FolderDialog { title: t!("folder-rename-title", name = current), file: Some(file), name: current.to_string(), error: None }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<String> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        egui::Window::new(self.title.clone())
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let edit = ui.add(egui::TextEdit::singleline(&mut self.name).hint_text(t!("folder-name-hint")).desired_width(260.0));
                if self.name.is_empty() {
                    edit.request_focus();
                }
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x3a, 0x3a), error);
                }
                ui.horizontal(|ui| {
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.add_enabled(!self.name.trim().is_empty(), egui::Button::new(t!("button-save"))).clicked()
                        || (enter && !self.name.trim().is_empty())
                    {
                        outcome = Outcome::Submit(self.name.trim().to_string());
                    }
                    if ui.button(t!("button-cancel")).clicked() {
                        outcome = Outcome::Cancel;
                    }
                });
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }
}

/// "Forget the host keys of …?"
pub struct ConfirmForget {
    pub alias: String,
    names: Vec<String>,
    pub error: Option<String>,
}

impl ConfirmForget {
    pub fn new(alias: &str, names: Vec<String>) -> ConfirmForget {
        ConfirmForget { alias: alias.to_string(), names, error: None }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        egui::Window::new(t!("forget-title"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(t!("forget-question", alias = self.alias.as_str()));
                for name in &self.names {
                    ui.monospace(format!("  {name}"));
                }
                ui.weak(t!("forget-note"));
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x3a, 0x3a), error);
                }
                ui.horizontal(|ui| {
                    if ui.button(t!("forget-button")).clicked() {
                        outcome = Outcome::Submit(());
                    }
                    if ui.button(t!("button-cancel")).clicked() {
                        outcome = Outcome::Cancel;
                    }
                });
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }
}

/// "Really delete?"
/// A batch close that would close tabs holding other panes as well.
pub struct ConfirmCloseMixed {
    pub ids: Vec<String>,
    /// (label, its tab holds other panes)
    sessions: Vec<(String, bool)>,
}

impl ConfirmCloseMixed {
    pub fn new(ids: Vec<String>, sessions: Vec<(String, bool)>) -> ConfirmCloseMixed {
        ConfirmCloseMixed { ids, sessions }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        let mixed: Vec<&str> = self.sessions.iter().filter(|(_, m)| *m).map(|(l, _)| l.as_str()).collect();
        egui::Window::new(t!("close-mixed-title"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(t!("close-mixed-what", count = self.sessions.len(), mixed = mixed.len()));
                ui.weak(t!("close-mixed-hint"));
                egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    for (label, mixed) in &self.sessions {
                        if *mixed {
                            ui.label(format!("{label}  ·  {}", t!("session-split")));
                        } else {
                            ui.weak(label);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(t!("close-mixed-close", count = self.sessions.len())).clicked() {
                        outcome = Outcome::Submit(());
                    }
                    if ui.button(t!("button-cancel")).clicked() {
                        outcome = Outcome::Cancel;
                    }
                });
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }
}

pub struct ConfirmDelete {
    pub alias: String,
    label: String,
    pub error: Option<String>,
}

impl ConfirmDelete {
    pub fn new(alias: &str, label: &str) -> ConfirmDelete {
        ConfirmDelete { alias: alias.to_string(), label: label.to_string(), error: None }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        egui::Window::new(t!("delete-title"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(t!("delete-question", label = self.label.as_str(), alias = self.alias.as_str()));
                ui.weak(t!("delete-backup-note"));
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x3a, 0x3a), error);
                }
                ui.horizontal(|ui| {
                    if ui.button(t!("button-delete")).clicked() {
                        outcome = Outcome::Submit(());
                    }
                    if ui.button(t!("button-cancel")).clicked() {
                        outcome = Outcome::Cancel;
                    }
                });
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }
}
