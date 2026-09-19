//! Shared credential sets (`NativeTermCredential <name>`): a password kept
//! once in Credential Manager (`NativeTerm/cred/<name>`) for every host
//! that names the set, or whose folder does. This dialog adds sets,
//! changes and removes their passwords, and says how many hosts use each;
//! hosts and folders pick a set in the host dialog and the folder menu.
//! A set holds a password only: the user is the ssh config's.

use std::collections::HashMap;

use native_term_app::t;
use native_term_config::password::{self, REFUSED};
use native_term_config::tree::SessionTree;
use native_term_win::credentials::{self, Saved};

use crate::dialogs::{no_ime, Outcome};

const RED: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);

/// The sets in Credential Manager, by name.
pub fn names() -> Vec<String> {
    let prefix = password::set_prefix();
    credentials::list(&prefix)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| entry.strip_prefix(&prefix).map(str::to_string))
        .collect()
}

/// How many hosts use each set (their own setting or their folder's).
pub fn usage(tree: &SessionTree) -> HashMap<String, usize> {
    let mut used = HashMap::new();
    for (folder, host) in tree.hosts().filter(|(_, h)| h.plink.is_none()) {
        if let Some(set) = password::set_for_host(folder, host) {
            *used.entry(set.to_string()).or_insert(0) += 1;
        }
    }
    used
}

/// Stores a set's password (a new set, or a new password: the refused mark
/// goes with the old one).
fn save(name: &str, secret: String) -> std::io::Result<()> {
    let saved = Saved { user: String::new(), secret, comment: String::new() };
    let result = credentials::write(&password::set_entry(name), &saved);
    drop(saved);
    result
}

struct Row {
    name: String,
    refused: bool,
    typed: String,
}

pub struct CredentialSetsDialog {
    rows: Vec<Row>,
    used: HashMap<String, usize>,
    /// Sets named in the ssh config but not in Credential Manager.
    missing: Vec<String>,
    new_name: String,
    new_password: String,
    /// The set whose removal waits for a second click.
    removing: Option<String>,
    message: Option<(String, bool)>,
}

impl CredentialSetsDialog {
    pub fn new(tree: &SessionTree) -> CredentialSetsDialog {
        let mut dialog = CredentialSetsDialog {
            rows: Vec::new(),
            used: usage(tree),
            missing: Vec::new(),
            new_name: String::new(),
            new_password: String::new(),
            removing: None,
            message: None,
        };
        dialog.refresh();
        dialog
    }

    fn refresh(&mut self) {
        self.rows = names()
            .into_iter()
            .map(|name| {
                let refused =
                    credentials::read(&password::set_entry(&name)).ok().flatten().is_some_and(|s| s.comment == REFUSED);
                Row { name, refused, typed: String::new() }
            })
            .collect();
        let mut missing: Vec<String> =
            self.used.keys().filter(|n| !self.rows.iter().any(|r| &r.name == *n)).cloned().collect();
        missing.sort();
        self.missing = missing;
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        egui::Window::new(t!("cred-sets-title"))
            .collapsible(false)
            .resizable(false)
            .min_width(660.0)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.weak(t!("cred-sets-intro"));
                ui.separator();
                if self.rows.is_empty() {
                    ui.weak(t!("cred-sets-none"));
                }
                let mut changed = false;
                egui::Grid::new("cred-sets").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
                    for row in &mut self.rows {
                        ui.strong(row.name.as_str());
                        let used = self.used.get(&row.name).copied().unwrap_or(0);
                        if row.refused {
                            ui.colored_label(RED, t!("cred-sets-refused"));
                        } else {
                            ui.weak(t!("cred-sets-used", count = used));
                        }
                        // a set size: in a grid cell the desired width isn't kept
                        let field = ui.add_sized(
                            [180.0, 22.0],
                            egui::TextEdit::singleline(&mut row.typed)
                                .password(true)
                                .hint_text(t!("cred-sets-new-password")),
                        );
                        no_ime(&field);
                        ui.horizontal(|ui| {
                            if ui.add_enabled(!row.typed.is_empty(), egui::Button::new(t!("password-save"))).clicked() {
                                self.message = Some(match save(&row.name, std::mem::take(&mut row.typed)) {
                                    Ok(()) => (t!("cred-sets-saved", name = row.name.as_str()), false),
                                    Err(e) => (e.to_string(), true),
                                });
                                changed = true;
                            }
                            let second = self.removing.as_deref() == Some(row.name.as_str());
                            let text = if second {
                                t!("cred-sets-remove-confirm", count = used)
                            } else {
                                t!("password-remove")
                            };
                            let button = egui::Button::new(if second {
                                egui::RichText::new(text).color(RED)
                            } else {
                                egui::RichText::new(text)
                            });
                            if ui.add(button).clicked() {
                                if second {
                                    self.removing = None;
                                    self.message = Some(match credentials::delete(&password::set_entry(&row.name)) {
                                        Ok(_) => (t!("cred-sets-removed", name = row.name.as_str()), false),
                                        Err(e) => (e.to_string(), true),
                                    });
                                    changed = true;
                                } else {
                                    self.removing = Some(row.name.clone());
                                }
                            }
                        });
                        ui.end_row();
                    }
                });
                if !self.missing.is_empty() {
                    ui.colored_label(RED, t!("cred-sets-missing", names = self.missing.join(", ")));
                }
                ui.separator();
                ui.label(egui::RichText::new(t!("cred-sets-add")).strong());
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_name)
                            .hint_text(t!("cred-sets-name-hint"))
                            .desired_width(140.0),
                    );
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.new_password)
                            .password(true)
                            .hint_text(t!("cred-sets-password-hint"))
                            .desired_width(180.0),
                    );
                    no_ime(&field);
                    let name = self.new_name.trim().to_string();
                    let ready = !name.is_empty() && !self.new_password.is_empty();
                    if ui.add_enabled(ready, egui::Button::new(t!("cred-sets-add-button"))).clicked() {
                        self.message = Some(if !password::valid_set_name(&name) {
                            (t!("cred-sets-bad-name"), true)
                        } else if self.rows.iter().any(|r| r.name == name) {
                            (t!("cred-sets-exists", name = name.as_str()), true)
                        } else {
                            match save(&name, std::mem::take(&mut self.new_password)) {
                                Ok(()) => {
                                    self.new_name.clear();
                                    changed = true;
                                    (t!("cred-sets-added", name = name.as_str()), false)
                                }
                                Err(e) => (e.to_string(), true),
                            }
                        });
                    }
                });
                if let Some((text, error)) = &self.message {
                    if *error {
                        ui.colored_label(RED, text);
                    } else {
                        ui.weak(text);
                    }
                }
                ui.weak(t!("password-warning"));
                if changed {
                    self.refresh();
                }
                if ui.button(t!("button-close")).clicked() {
                    outcome = Outcome::Cancel;
                }
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }
}
