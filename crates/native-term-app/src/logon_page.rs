//! The logon actions' page, laid out as SecureCRT's Session Options →
//! Connection → Logon Actions: "Automate logon", "Send initial carriage
//! return", the Expect/Send table (moved up and down, added, edited and
//! deleted), and the editor of a row as its "Expect/Send Properties"
//! dialog, with the escapes it lists. Shared by the session options (an
//! SSH host, or a folder's default) and the non-SSH session dialog. A
//! session either uses its folder's set or has a whole set of its own
//! (see `native_term_config::logon`).
//!
//! A hidden Send is kept in the system's password store, not in the
//! configuration: what is typed for one goes there when the dialog is
//! saved ([`LogonPage::secrets`]), under an id of its own that the
//! configuration names. Its text is never read back into this page:
//! editing such a row leaves it as it is unless something new is typed.
//! "Logon script" and "Remote command" are not on the page: NativeTerm
//! has no script engine, and ssh's `RemoteCommand` is under Connection.

use native_term_app::t;
use native_term_config::logon::{self, LogonActions, Step};

use crate::dialogs::no_ime;

/// A row as the page holds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Row {
    step: Step,
    /// A hidden row's text typed here, to be stored on saving (empty: the
    /// stored one stays).
    typed: String,
    /// A hidden row copied from the folder's set: the entry its text is
    /// copied from on saving.
    copy_of: Option<String>,
}

impl Row {
    fn from_step(step: &Step) -> Row {
        Row { step: step.clone(), typed: String::new(), copy_of: None }
    }

    /// What the Send column shows.
    fn shown_send(&self) -> String {
        if self.step.hide {
            "••••••".to_string()
        } else {
            self.step.send.clone()
        }
    }
}

/// The row being edited below the table.
struct Editor {
    /// The row it replaces (`None`: a new one).
    at: Option<usize>,
    row: Row,
    /// A hidden row kept its stored text (nothing typed yet).
    kept: bool,
}

/// What saving writes to and removes from the password store.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Secrets {
    /// Entry id and text.
    pub write: Vec<(String, String)>,
    /// Entries copied (from, to): a folder's hidden Sends a session took
    /// over with its set.
    pub copy: Vec<(String, String)>,
    /// Entry ids no row names any more.
    pub remove: Vec<String>,
}

pub struct LogonPage {
    /// A session's page (a folder's has no "use the folder's").
    session: bool,
    /// The session uses its folder's set.
    inherit: bool,
    automate: bool,
    initial_cr: bool,
    rows: Vec<Row>,
    folder: LogonActions,
    /// The hidden Sends' ids of the set of its own the page opened with
    /// (a folder's, or a session's own; not a folder's seen by a session).
    stored: Vec<String>,
    selected: Option<usize>,
    editor: Option<Editor>,
    /// The credential sets there are (for `\s`, `\w`).
    sets: Vec<String>,
    error: Option<String>,
}

impl LogonPage {
    /// A session's page: its own set if it has one, else its folder's.
    pub fn for_session(own: Option<LogonActions>, folder: Option<LogonActions>, sets: Vec<String>) -> LogonPage {
        let folder = folder.unwrap_or_default();
        let inherit = own.is_none();
        let mut page = LogonPage::with(own.as_ref().unwrap_or(&folder), sets);
        page.session = true;
        page.inherit = inherit;
        page.stored = own.as_ref().map(ids).unwrap_or_default();
        page.folder = folder;
        page
    }

    /// A folder's default set.
    pub fn for_folder(folder: Option<LogonActions>, sets: Vec<String>) -> LogonPage {
        LogonPage::with(&folder.unwrap_or_default(), sets)
    }

    fn with(actions: &LogonActions, sets: Vec<String>) -> LogonPage {
        LogonPage {
            session: false,
            inherit: false,
            automate: actions.automate,
            initial_cr: actions.initial_cr,
            rows: actions.steps.iter().map(Row::from_step).collect(),
            folder: LogonActions::default(),
            stored: ids(actions),
            selected: None,
            editor: None,
            sets,
            error: None,
        }
    }

    /// The set as it stands (hidden rows naming their entries).
    fn actions(&self) -> LogonActions {
        LogonActions {
            automate: self.automate,
            initial_cr: self.initial_cr,
            steps: self.rows.iter().map(|r| r.step.clone()).collect(),
        }
    }

    /// What to write: a session's own set, or `None` for its folder's; a
    /// folder's set, or `None` when nothing in it differs from nothing set.
    pub fn result(&self) -> Option<LogonActions> {
        match (self.session, self.inherit) {
            (true, true) => None,
            (true, false) => Some(self.actions()),
            (false, _) => (self.actions() != LogonActions::default()).then(|| self.actions()),
        }
    }

    /// The password store's changes that go with [`result`]: the hidden
    /// Sends typed here, and the entries no row names any more (a folder's
    /// set seen by a session that inherits it is not the session's).
    pub fn secrets(&self) -> Secrets {
        let written = self.result();
        let kept = written.as_ref().map(ids).unwrap_or_default();
        let mut secrets = Secrets::default();
        if written.is_some() {
            for row in self.rows.iter().filter(|r| r.step.hide) {
                let Some(id) = logon::secret_id(&row.step.send) else { continue };
                if !row.typed.is_empty() {
                    secrets.write.push((id.to_string(), row.typed.clone()));
                } else if let Some(from) = &row.copy_of {
                    secrets.copy.push((from.clone(), id.to_string()));
                }
            }
        }
        secrets.remove = self.stored.iter().filter(|id| !kept.contains(id)).cloned().collect();
        secrets
    }

    /// A problem that keeps the page from being saved, if any.
    pub fn error(&self) -> Option<String> {
        if self.editor.is_some() {
            return Some(t!("logon-editing"));
        }
        None
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if self.session {
            if ui.checkbox(&mut self.inherit, t!("logon-use-folder")).changed() && !self.inherit {
                // starting from what the folder has (its hidden Sends
                // copied: typed again, they would be the session's own)
                self.automate = self.folder.automate;
                self.initial_cr = self.folder.initial_cr;
                self.rows = self.folder.steps.iter().map(Row::from_step).collect();
                for row in self.rows.iter_mut().filter(|r| r.step.hide) {
                    row.copy_of = logon::secret_id(&row.step.send).map(str::to_string);
                    row.step.send = format!("secret:{}", logon::new_secret_id());
                }
            }
            ui.add_space(4.0);
        }
        let editable = !self.inherit;
        // (the folder's set, shown as it is, while the session uses it)
        let (mut automate, mut initial_cr) =
            if editable { (self.automate, self.initial_cr) } else { (self.folder.automate, self.folder.initial_cr) };
        ui.add_enabled_ui(editable, |ui| {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.checkbox(&mut automate, t!("logon-automate"));
                    ui.add_space(24.0);
                    ui.checkbox(&mut initial_cr, t!("logon-initial-cr"));
                });
                ui.add_enabled_ui(automate, |ui| self.table(ui));
            });
        });
        if editable {
            self.automate = automate;
            self.initial_cr = initial_cr;
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::from_rgb(0xd0, 0x3a, 0x3a), error);
        }
        ui.weak(t!("logon-note"));
    }

    fn table(&mut self, ui: &mut egui::Ui) {
        let shown: Vec<Row> =
            if self.inherit { self.folder.steps.iter().map(Row::from_step).collect() } else { self.rows.clone() };
        ui.horizontal_top(|ui| {
            egui::Frame::NONE.stroke(ui.visuals().widgets.noninteractive.bg_stroke).inner_margin(4.0).show(ui, |ui| {
                ui.set_width(ui.available_width() - 40.0);
                ui.set_min_height(110.0);
                egui::ScrollArea::vertical().id_salt("logon-rows").max_height(160.0).show(ui, |ui| {
                    egui::Grid::new("logon-table").num_columns(5).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
                        for head in [
                            t!("logon-expect"),
                            t!("logon-send"),
                            t!("logon-hide"),
                            t!("logon-enter"),
                            t!("logon-credential"),
                        ] {
                            ui.strong(head);
                        }
                        ui.end_row();
                        for (i, row) in shown.iter().enumerate() {
                            let chosen = self.selected == Some(i);
                            let expect =
                                if row.step.expect.is_empty() { t!("logon-at-once") } else { row.step.expect.clone() };
                            let clicked =
                                ui.selectable_label(chosen, egui::RichText::new(expect).monospace()).clicked()
                                    | ui.selectable_label(chosen, egui::RichText::new(row.shown_send()).monospace())
                                        .clicked();
                            ui.label(if row.step.hide { "✓" } else { "" });
                            ui.label(if row.step.enter { "✓" } else { "" });
                            ui.label(row.step.credential.clone().unwrap_or_default());
                            ui.end_row();
                            if clicked {
                                self.selected = Some(i);
                            }
                        }
                    });
                });
            });
            ui.vertical(|ui| {
                let can_up = self.selected.is_some_and(|i| i > 0);
                let can_down = self.selected.is_some_and(|i| i + 1 < self.rows.len());
                if ui.add_enabled(can_up && self.editor.is_none(), egui::Button::new("▲")).clicked() {
                    if let Some(i) = self.selected {
                        self.rows.swap(i, i - 1);
                        self.selected = Some(i - 1);
                    }
                }
                if ui.add_enabled(can_down && self.editor.is_none(), egui::Button::new("▼")).clicked() {
                    if let Some(i) = self.selected {
                        self.rows.swap(i, i + 1);
                        self.selected = Some(i + 1);
                    }
                }
            });
        });
        let idle = self.editor.is_none();
        ui.horizontal(|ui| {
            if ui.add_enabled(idle, egui::Button::new(t!("logon-add"))).clicked() {
                let row = Row { step: Step { enter: true, ..Step::default() }, ..Row::default() };
                self.editor = Some(Editor { at: None, row, kept: false });
            }
            let one = self.selected.filter(|i| *i < self.rows.len());
            if ui.add_enabled(idle && one.is_some(), egui::Button::new(t!("logon-edit"))).clicked() {
                if let Some(i) = one {
                    let row = self.rows[i].clone();
                    let kept = row.step.hide && row.typed.is_empty();
                    self.editor = Some(Editor { at: Some(i), row, kept });
                }
            }
            if ui.add_enabled(idle && one.is_some(), egui::Button::new(t!("logon-delete"))).clicked() {
                if let Some(i) = one {
                    self.rows.remove(i);
                    self.selected = None;
                }
            }
        });
        if self.editor.is_some() {
            self.editor_ui(ui);
        }
    }

    /// SecureCRT's "Expect/Send Properties", below the table.
    fn editor_ui(&mut self, ui: &mut egui::Ui) {
        let sets = self.sets.clone();
        let Some(editor) = self.editor.as_mut() else { return };
        let mut done: Option<bool> = None;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong(if editor.at.is_some() { t!("logon-edit-title") } else { t!("logon-add-title") });
            egui::Grid::new("logon-editor").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label(t!("logon-expect-label"));
                ui.add(egui::TextEdit::singleline(&mut editor.row.step.expect).desired_width(360.0));
                ui.end_row();
                ui.label(t!("logon-send-label"));
                if editor.row.step.hide {
                    let hint = if editor.kept { t!("logon-send-kept") } else { String::new() };
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut editor.row.typed)
                            .password(true)
                            .hint_text(hint)
                            .desired_width(360.0),
                    );
                    no_ime(&field);
                } else {
                    ui.add(egui::TextEdit::singleline(&mut editor.row.step.send).desired_width(360.0));
                }
                ui.end_row();
                ui.label("");
                ui.horizontal(|ui| {
                    let was = editor.row.step.hide;
                    ui.checkbox(&mut editor.row.step.hide, t!("logon-hide-label"));
                    if editor.row.step.hide != was {
                        // hidden: typed again into the password field (it
                        // goes to the store); shown: typed again in the open
                        if editor.row.step.hide {
                            editor.row.typed = std::mem::take(&mut editor.row.step.send);
                            editor.row.step.send = format!("secret:{}", logon::new_secret_id());
                        } else {
                            editor.row.step.send = std::mem::take(&mut editor.row.typed);
                        }
                        editor.row.copy_of = None;
                        editor.kept = false;
                    }
                    ui.add_space(24.0);
                    ui.checkbox(&mut editor.row.step.enter, t!("logon-enter-label"));
                });
                ui.end_row();
                ui.label(t!("logon-credential-label"));
                let none = t!("logon-credential-none");
                let chosen = editor.row.step.credential.clone().unwrap_or_else(|| none.clone());
                egui::ComboBox::from_id_salt("logon-credential").selected_text(chosen).show_ui(ui, |ui| {
                    ui.selectable_value(&mut editor.row.step.credential, None, none.as_str());
                    for set in &sets {
                        ui.selectable_value(&mut editor.row.step.credential, Some(set.clone()), set.as_str());
                    }
                });
                ui.end_row();
            });
            ui.add_space(4.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.strong(t!("logon-commands"));
                for line in [t!("logon-commands-keys"), t!("logon-commands-text"), t!("logon-commands-credentials")] {
                    ui.monospace(line);
                }
                ui.weak(t!("logon-clipboard-note"));
            });
            ui.horizontal(|ui| {
                if ui.button(t!("button-ok")).clicked() {
                    done = Some(true);
                }
                if ui.button(t!("button-cancel")).clicked() {
                    done = Some(false);
                }
            });
        });
        match done {
            Some(true) => {
                let Some(editor) = self.editor.take() else { return };
                let empty_hidden = editor.row.step.hide && editor.row.typed.is_empty() && !editor.kept;
                let empty =
                    editor.row.step.expect.is_empty() && editor.row.step.send.is_empty() && !editor.row.step.hide;
                if empty || empty_hidden {
                    self.error = Some(t!("logon-row-empty"));
                    self.editor = Some(editor);
                    return;
                }
                self.error = None;
                match editor.at {
                    Some(i) if i < self.rows.len() => self.rows[i] = editor.row,
                    _ => {
                        self.rows.push(editor.row);
                        self.selected = Some(self.rows.len() - 1);
                    }
                }
            }
            Some(false) => {
                self.editor = None;
                self.error = None;
            }
            None => {}
        }
    }
}

/// The ids of the hidden Sends a set names.
fn ids(actions: &LogonActions) -> Vec<String> {
    actions.steps.iter().filter(|s| s.hide).filter_map(|s| logon::secret_id(&s.send)).map(str::to_string).collect()
}

/// The password store's changes, done: new hidden Sends first (before the
/// configuration names them), entries no row names after it no longer
/// does. The first error stops it.
pub fn store_secrets(secrets: &Secrets) -> std::io::Result<()> {
    use native_term_os::credentials::{self, Saved};
    let copied = secrets.copy.iter().filter_map(|(from, to)| {
        let saved = credentials::read(&logon::secret_entry(from)).ok().flatten()?;
        Some((to.clone(), saved.secret))
    });
    for (id, text) in secrets.write.iter().cloned().chain(copied) {
        let saved = Saved { user: String::new(), secret: text, comment: String::new() };
        credentials::write(&logon::secret_entry(&id), &saved)?;
    }
    Ok(())
}

/// The entries no row names any more, taken out (after the configuration
/// was written; one that can't go is left, harmless).
pub fn remove_secrets(secrets: &Secrets) {
    for id in &secrets.remove {
        let _ = native_term_os::credentials::delete(&logon::secret_entry(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(expect: &str, send: &str) -> Step {
        Step { expect: expect.into(), send: send.into(), enter: true, ..Step::default() }
    }

    fn hidden(expect: &str, id: &str) -> Step {
        Step { hide: true, ..step(expect, &format!("secret:{id}")) }
    }

    #[test]
    fn a_session_writes_its_own_set_or_none() {
        let folder = LogonActions { automate: true, steps: vec![step("$", "sudo -i")], ..LogonActions::default() };
        let page = LogonPage::for_session(None, Some(folder.clone()), Vec::new());
        assert_eq!(page.result(), None, "inherits: nothing of its own is written");
        assert_eq!(page.secrets(), Secrets::default());
        let mut page = LogonPage::for_session(None, Some(folder.clone()), Vec::new());
        page.inherit = false;
        page.initial_cr = true;
        assert_eq!(page.result(), Some(LogonActions { initial_cr: true, ..folder }));
        assert_eq!(LogonPage::for_folder(None, Vec::new()).result(), None, "a folder with nothing set writes nothing");
    }

    #[test]
    fn hidden_sends_go_to_the_store_and_come_out_of_it() {
        let own = LogonActions {
            automate: true,
            steps: vec![hidden("Password:", "old-1"), hidden("enable", "old-2"), step("#", "terminal length 0")],
            ..LogonActions::default()
        };
        let mut page = LogonPage::for_session(Some(own), None, Vec::new());
        assert_eq!(page.secrets(), Secrets::default(), "nothing typed, nothing removed");
        // one typed again, one row deleted
        page.rows[0].typed = "n3w".into();
        page.rows.remove(1);
        let secrets = page.secrets();
        assert_eq!(secrets.write, [("old-1".to_string(), "n3w".to_string())]);
        assert_eq!(secrets.remove, ["old-2".to_string()]);
        // the session goes back to its folder's set: all of its own go
        page.inherit = true;
        assert_eq!(page.secrets().remove, ["old-1".to_string(), "old-2".to_string()]);
        assert!(page.secrets().write.is_empty());
        // a folder's own set, emptied: its entries go
        let mut folder = LogonPage::for_folder(
            Some(LogonActions { automate: true, steps: vec![hidden("x", "f-9")], ..LogonActions::default() }),
            Vec::new(),
        );
        folder.rows.clear();
        folder.automate = false;
        assert_eq!(folder.secrets().remove, ["f-9".to_string()]);
    }

    #[test]
    fn a_folders_hidden_sends_are_not_the_sessions() {
        let folder =
            LogonActions { automate: true, steps: vec![hidden("Password:", "f-1")], ..LogonActions::default() };
        let page = LogonPage::for_session(None, Some(folder), Vec::new());
        assert!(page.secrets().remove.is_empty(), "the folder keeps its own");
    }

    #[test]
    fn nothing_is_saved_while_a_row_is_edited() {
        let mut page = LogonPage::for_folder(None, Vec::new());
        assert_eq!(page.error(), None);
        page.editor = Some(Editor { at: None, row: Row::default(), kept: false });
        assert!(page.error().is_some());
    }

    #[test]
    fn a_folders_hidden_sends_are_copied_when_a_session_takes_its_set() {
        let folder =
            LogonActions { automate: true, steps: vec![hidden("Password:", "f-1")], ..LogonActions::default() };
        let mut page = LogonPage::for_session(None, Some(folder), Vec::new());
        page.inherit = false;
        // (as the checkbox does it)
        page.rows = page.folder.steps.iter().map(Row::from_step).collect();
        page.rows[0].copy_of = Some("f-1".into());
        page.rows[0].step.send = "secret:s-1".into();
        let secrets = page.secrets();
        assert_eq!(secrets.copy, [("f-1".to_string(), "s-1".to_string())]);
        assert!(secrets.remove.is_empty() && secrets.write.is_empty());
    }
}
