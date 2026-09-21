//! Modal-ish windows for editing sessions. Each returns what the user
//! decided; the app performs it and shows errors back in the dialog.

use std::path::PathBuf;

use native_term_app::t;
use native_term_config::ops::HostDraft;
use native_term_config::password::{self, Target, REFUSED};
use native_term_config::persistent;
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
    /// The host's own tab color / color scheme (`None`: as the folder).
    tab_color: Option<String>,
    color_scheme: Option<String>,
    /// The folder's, shown with "as the folder".
    folder_look: (Option<String>, Option<String>),
    /// The account's saved password (editing a saved host only): its own
    /// entry, or its credential set's.
    password: Option<PasswordField>,
    /// The account's own entry (`password` switches between it and a set).
    account: Option<Target>,
    /// The host's own `NativeTermCredential` (`None`: as the folder;
    /// `none`: no set).
    credential: Option<String>,
    /// A new set's name being typed ("New…").
    new_set: Option<String>,
    /// The folder's set, and the sets there are, for the choice.
    folder_credential: Option<String>,
    sets: Vec<String>,
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
        let set = self.target.set.clone();
        let title = match &set {
            Some(set) => t!("password-title-set", set = set.as_str()),
            None => t!("password-title"),
        };
        ui.label(egui::RichText::new(title).strong());
        let (text, color) = match (self.state, &set) {
            (PasswordState::None, Some(_)) => (t!("password-none-set"), None),
            (PasswordState::None, None) => (t!("password-none"), None),
            (PasswordState::Saved, Some(_)) => (t!("password-saved-set"), None),
            (PasswordState::Saved, None) => (t!("password-saved", target = self.target.name.as_str()), None),
            (PasswordState::Refused, Some(_)) => {
                (t!("password-refused-set"), Some(egui::Color32::from_rgb(0xd0, 0x3a, 0x3a)))
            }
            (PasswordState::Refused, None) => (t!("password-refused"), Some(egui::Color32::from_rgb(0xd0, 0x3a, 0x3a))),
        };
        match color {
            Some(c) => ui.colored_label(c, text),
            None => ui.weak(text),
        };
        ui.horizontal(|ui| {
            let account = format!("{}@{}", self.target.user, self.target.host);
            let hint = match &set {
                Some(_) => t!("password-hint-set"),
                None => t!("password-hint", account = account.as_str()),
            };
            let field =
                ui.add(egui::TextEdit::singleline(&mut self.typed).password(true).hint_text(hint).desired_width(200.0));
            no_ime(&field);
            if ui.add_enabled(!self.typed.is_empty(), egui::Button::new(t!("password-save"))).clicked() {
                // a set's password isn't one account's
                let user = if set.is_some() { String::new() } else { self.target.user.clone() };
                let saved = Saved { user, secret: std::mem::take(&mut self.typed), comment: String::new() };
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
            // a set is removed where all its hosts are seen (Credential Sets)
            if set.is_none() && self.state != PasswordState::None && ui.button(t!("password-remove")).clicked() {
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

/// A folder's `NativeTermPersistent` in words.
fn persistent_text(value: Option<&str>) -> String {
    match value.map(str::to_ascii_lowercase).as_deref() {
        Some(v) if persistent::logged(v) => t!("persistent-tmux-log"),
        Some(p @ ("tmux" | "screen")) => p.to_string(),
        _ => t!("persistent-off"),
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
            tab_color: d.tab_color.clone(),
            color_scheme: d.color_scheme.clone(),
            folder_look: (None, None),
            password: None,
            account: None,
            credential: d.credential.clone(),
            new_set: None,
            folder_credential: None,
            sets: Vec::new(),
            error: None,
        }
    }

    /// The folder's credential set and the sets there are, for the choice.
    pub fn with_credentials(mut self, folder: Option<String>, sets: Vec<String>) -> HostDialog {
        self.folder_credential = folder.filter(|f| password::valid_set_name(f));
        self.sets = sets;
        self.follow_set();
        self
    }

    /// The set the host would use as the dialog stands.
    fn chosen_set(&self) -> Option<String> {
        match self.new_set.as_deref().map(str::trim) {
            Some(name) => Some(name.to_string()).filter(|n| password::valid_set_name(n)),
            None => match self.credential.as_deref() {
                Some(own) if own.eq_ignore_ascii_case("none") => None,
                Some(own) => Some(own.to_string()).filter(|n| password::valid_set_name(n)),
                None => self.folder_credential.clone(),
            },
        }
    }

    /// The password field shows the chosen set's password, or the
    /// account's own.
    fn follow_set(&mut self) {
        let Some(account) = &self.account else { return };
        let set = self.chosen_set();
        if self.password.as_ref().is_some_and(|p| p.target.set == set) {
            return;
        }
        let target = match &set {
            Some(set) => Target { name: password::set_entry(set), set: Some(set.clone()), ..account.clone() },
            None => account.clone(),
        };
        self.password = Some(PasswordField::new(target));
    }

    /// The credential-set choice: as the folder, none, a set, or a new one.
    fn credential_choice(&mut self, ui: &mut egui::Ui) {
        let folder_text = self.folder_credential.clone().unwrap_or_else(|| t!("credential-folder-none"));
        let current = match (&self.new_set, self.credential.as_deref()) {
            (Some(_), _) => t!("credential-new"),
            (None, None) => t!("credential-folder", value = folder_text.as_str()),
            (None, Some(v)) if v.eq_ignore_ascii_case("none") => t!("credential-none"),
            (None, Some(v)) => v.to_string(),
        };
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("host-credential").selected_text(current).width(180.0).show_ui(ui, |ui| {
                let mut pick = |ui: &mut egui::Ui, value: Option<String>, text: String| {
                    let on = self.new_set.is_none() && self.credential == value;
                    if ui.selectable_label(on, text).clicked() {
                        self.credential = value;
                        self.new_set = None;
                    }
                };
                pick(ui, None, t!("credential-folder", value = folder_text.as_str()));
                pick(ui, Some("none".into()), t!("credential-none"));
                for set in self.sets.clone() {
                    pick(ui, Some(set.clone()), set);
                }
                if ui.selectable_label(self.new_set.is_some(), t!("credential-new")).clicked() {
                    self.new_set = Some(String::new());
                }
            });
            if let Some(name) = &mut self.new_set {
                ui.add(egui::TextEdit::singleline(name).hint_text(t!("cred-sets-name-hint")).desired_width(110.0));
            }
        });
    }

    /// The folder's tab color and color scheme, for "as the folder (…)".
    pub fn with_folder_look(mut self, tab_color: Option<String>, color_scheme: Option<String>) -> HostDialog {
        self.folder_look = (tab_color, color_scheme);
        self
    }

    /// Offer a saved password for this account (`ssh -G`'s user, host,
    /// port), or for its credential set; a new host has none yet.
    pub fn with_password(mut self, target: Option<Target>) -> HostDialog {
        self.account = target;
        self.follow_set();
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
            tab_color: self.tab_color.clone().filter(|c| c != "#"),
            color_scheme: self.color_scheme.clone().filter(|s| !s.trim().is_empty()),
            credential: match self.new_set.as_deref().map(str::trim) {
                Some(name) if password::valid_set_name(name) => Some(name.to_string()),
                Some(_) => return Err(t!("cred-sets-bad-name")),
                None => self.credential.clone(),
            },
        })
    }

    /// The choices for "keep on the server": (stored value, text).
    fn persistent_choices(&self) -> Vec<(Option<String>, String)> {
        let folder = persistent_text(self.folder_persistent.as_deref());
        vec![
            (None, t!("persistent-folder", value = folder.as_str())),
            (Some("tmux".into()), "tmux".into()),
            (Some(persistent::TMUX_LOG.into()), t!("persistent-tmux-log")),
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
                    ui.label(t!("field-tab-color")).on_hover_text(t!("field-tab-color-hint"));
                    tab_color_choice(ui, &mut self.tab_color, self.folder_look.0.as_deref());
                    ui.end_row();
                    ui.label(t!("field-color-scheme"));
                    color_scheme_choice(ui, &mut self.color_scheme, self.folder_look.1.as_deref());
                    ui.end_row();
                    ui.label(t!("field-persistent")).on_hover_text(t!("field-persistent-hint"));
                    let choices = self.persistent_choices();
                    let current =
                        choices.iter().find(|(v, _)| *v == self.persistent).map(|(_, t)| t.clone()).unwrap_or_default();
                    egui::ComboBox::from_id_salt("host-persistent").selected_text(current).width(280.0).show_ui(
                        ui,
                        |ui| {
                            for (value, text) in choices {
                                ui.selectable_value(&mut self.persistent, value, text);
                            }
                        },
                    );
                    ui.end_row();
                    ui.label(t!("field-credential")).on_hover_text(t!("field-credential-hint"));
                    self.credential_choice(ui);
                    ui.end_row();
                });
                if let Some(alias) = &self.alias {
                    ui.weak(t!("host-alias-kept", alias = alias.as_str()));
                }
                self.follow_set();
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

/// A preset's or a hex color's swatch and name.
pub fn color_text(value: &str) -> egui::RichText {
    use native_term_config::appearance::{tab_color, PRESETS};
    let name = PRESETS.iter().find(|(n, _)| n.eq_ignore_ascii_case(value)).map(|(n, _)| match *n {
        "red" => t!("color-red"),
        "orange" => t!("color-orange"),
        "yellow" => t!("color-yellow"),
        "green" => t!("color-green"),
        "blue" => t!("color-blue"),
        _ => t!("color-purple"),
    });
    let swatch = tab_color(value).and_then(|hex| egui::Color32::from_hex(&hex).ok()).unwrap_or(egui::Color32::GRAY);
    egui::RichText::new(format!("■ {}", name.unwrap_or_else(|| value.to_string()))).color(swatch)
}

/// Tab color: as the folder, none, a preset, or a hex value typed in.
fn tab_color_choice(ui: &mut egui::Ui, value: &mut Option<String>, folder: Option<&str>) {
    use native_term_config::appearance::PRESETS;
    let folder_text = folder.map(|f| color_text(f).text().to_string()).unwrap_or_else(|| t!("look-none"));
    let custom = value.as_deref().is_some_and(|v| v.starts_with('#'));
    let current: egui::WidgetText = match value.as_deref() {
        None => t!("look-folder", value = folder_text.as_str()).into(),
        Some("none") => t!("look-none").into(),
        Some(_) if custom => t!("look-custom").into(),
        Some(v) => color_text(v).into(),
    };
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("host-tab-color").selected_text(current).width(180.0).show_ui(ui, |ui| {
            ui.selectable_value(value, None, t!("look-folder", value = folder_text.as_str()));
            ui.selectable_value(value, Some("none".into()), t!("look-none"));
            for (name, _) in PRESETS {
                ui.selectable_value(value, Some(name.to_string()), color_text(name));
            }
            if ui.selectable_label(custom, t!("look-custom")).clicked() && !custom {
                *value = Some("#".into());
            }
        });
        if let Some(v) = value.as_mut().filter(|v| v.starts_with('#')) {
            ui.add(egui::TextEdit::singleline(v).hint_text("#C0392B").desired_width(90.0));
            if let Some(color) =
                native_term_config::appearance::tab_color(v).and_then(|h| egui::Color32::from_hex(&h).ok())
            {
                ui.colored_label(color, "■");
            }
        }
    });
}

/// Color scheme: as the folder, the Terminal's default, or one of the
/// Terminal's built-in schemes (the shim applies it in the tab).
fn color_scheme_choice(ui: &mut egui::Ui, value: &mut Option<String>, folder: Option<&str>) {
    use native_term_config::appearance::SCHEMES;
    let folder_text = folder.map(str::to_string).unwrap_or_else(|| t!("look-terminal-default"));
    let current = match value.as_deref() {
        None => t!("look-folder", value = folder_text.as_str()),
        Some("none") => t!("look-terminal-default"),
        Some(v) => v.to_string(),
    };
    egui::ComboBox::from_id_salt("host-color-scheme").selected_text(current).width(180.0).show_ui(ui, |ui| {
        ui.selectable_value(value, None, t!("look-folder", value = folder_text.as_str()));
        ui.selectable_value(value, Some("none".into()), t!("look-terminal-default"));
        for s in &SCHEMES {
            ui.selectable_value(value, Some(s.name.to_string()), s.name);
        }
    });
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
        FolderDialog {
            title: t!("folder-rename-title", name = current),
            file: Some(file),
            name: current.to_string(),
            error: None,
        }
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
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.name).hint_text(t!("folder-name-hint")).desired_width(260.0),
                );
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

/// What to do with files dropped into a tab: Terminal pasted their names
/// and the client held the text back.
pub struct DropDialog {
    pub alias: String,
    pub session: String,
    pub label: String,
    pub paths: Vec<PathBuf>,
    /// The text Terminal pasted, sent to the session when that is chosen.
    pub text: String,
    /// The answer is remembered and this dialog skipped from then on.
    pub remember: bool,
    /// The session's files are on a server we can reach over SFTP.
    pub can_upload: bool,
}

/// What the person chose for dropped files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropChoice {
    Upload,
    Text,
}

impl DropDialog {
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<(DropChoice, bool)> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        let total: u64 = self.paths.iter().filter_map(|p| p.metadata().ok()).map(|m| m.len()).sum();
        egui::Window::new(t!("drop-title"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(t!("drop-what", count = self.paths.len(), label = self.label.as_str()));
                egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                    for path in &self.paths {
                        let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().to_string();
                        if path.is_dir() {
                            ui.label(format!("{name}  ·  {}", t!("drop-folder")));
                        } else {
                            ui.label(name);
                        }
                    }
                });
                if total > 0 {
                    ui.weak(t!("drop-size", size = crate::files_window::size_text(total)));
                }
                ui.weak(t!("drop-where"));
                ui.checkbox(&mut self.remember, t!("drop-remember"));
                ui.horizontal(|ui| {
                    let upload = ui.add_enabled(self.can_upload, egui::Button::new(t!("drop-upload")));
                    if upload.clicked() {
                        outcome = Outcome::Submit((DropChoice::Upload, self.remember));
                    }
                    if ui.button(t!("drop-text")).clicked() {
                        outcome = Outcome::Submit((DropChoice::Text, self.remember));
                    }
                    if ui.button(t!("button-cancel")).clicked() {
                        outcome = Outcome::Cancel;
                    }
                });
                if !self.can_upload {
                    ui.weak(t!("drop-no-sftp"));
                }
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }
}
