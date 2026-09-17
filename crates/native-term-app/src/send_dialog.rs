//! "Send command": type or pick a command, choose the sessions, send.
//! Only logged-in sessions can be chosen; sending to more than one asks
//! first; every send lands in the audit log (see `Core::send_text`).

use std::path::PathBuf;

use native_term_app::commands::{Command, Library};
use native_term_app::{t, Core, State};

use crate::dialogs::Outcome;
use crate::icons;

struct Target {
    id: String,
    label: String,
    state: State,
    chosen: bool,
}

pub struct SendDialog {
    targets: Vec<Target>,
    text: String,
    enter: bool,
    library: Result<Library, String>,
    library_path: PathBuf,
    picked: Option<String>,
    save_name: String,
    confirm: bool,
    result: Vec<(bool, String)>,
    focus_text: bool,
}

const RED: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x9a, 0x1a);
const GREEN: egui::Color32 = egui::Color32::from_rgb(0x2e, 0xa0, 0x43);

impl SendDialog {
    /// `chosen`: the sessions ticked at first (all logged-in ones if empty).
    pub fn new(core: &Core, chosen: &[String], data_dir: &std::path::Path) -> SendDialog {
        let targets = core
            .sessions()
            .into_iter()
            .filter(|s| s.state.is_open())
            .map(|s| {
                let ready = s.state == State::Connected && s.linked;
                let chosen = ready && (chosen.is_empty() || chosen.contains(&s.id));
                Target { id: s.id, label: s.label, state: s.state, chosen }
            })
            .collect();
        let library_path = Library::path_in(data_dir);
        let library = Library::load(&library_path).map_err(|e| e.to_string());
        SendDialog {
            targets,
            text: String::new(),
            enter: true,
            library,
            library_path,
            picked: None,
            save_name: String::new(),
            confirm: false,
            result: Vec::new(),
            focus_text: true,
        }
    }

    /// Refresh states (a session may log in or drop while the dialog is
    /// open); a session that can't receive any more is unticked.
    fn refresh(&mut self, core: &Core) {
        let sessions = core.sessions();
        for t in &mut self.targets {
            if let Some(s) = sessions.iter().find(|s| s.id == t.id) {
                t.state = s.state.clone();
                if !(s.state == State::Connected && s.linked) {
                    t.chosen = false;
                }
            }
        }
    }

    fn library_ui(&mut self, ui: &mut egui::Ui) {
        let lib = match &mut self.library {
            Ok(lib) => lib,
            Err(e) => {
                ui.colored_label(RED, t!("send-library-error", path = self.library_path.display().to_string(), error = e.as_str()));
                return;
            }
        };
        ui.horizontal(|ui| {
            ui.label(t!("send-saved"));
            let shown = self.picked.clone().unwrap_or_else(|| t!("send-saved-pick"));
            egui::ComboBox::from_id_salt("saved-commands").selected_text(shown).width(220.0).show_ui(ui, |ui| {
                if lib.commands.is_empty() {
                    ui.weak(t!("send-saved-none"));
                }
                let mut groups: Vec<Option<String>> = lib.commands.iter().map(|c| c.group.clone()).collect();
                groups.dedup();
                let mut seen = Vec::new();
                for group in groups {
                    if seen.contains(&group) {
                        continue;
                    }
                    seen.push(group.clone());
                    if let Some(g) = &group {
                        ui.weak(g);
                    }
                    for c in lib.commands.iter().filter(|c| c.group == group) {
                        if ui.selectable_label(self.picked.as_deref() == Some(&c.name), &c.name).on_hover_text(&c.text).clicked() {
                            self.picked = Some(c.name.clone());
                            self.text = c.text.clone();
                            self.enter = c.enter;
                            self.save_name = c.name.clone();
                        }
                    }
                }
            });
            if let Some(name) = self.picked.clone() {
                if ui.small_button(icons::CLEAR.to_string()).on_hover_text(t!("send-delete")).clicked() {
                    lib.remove(&name);
                    if let Err(e) = lib.save() {
                        self.result = vec![(false, e.to_string())];
                    }
                    self.picked = None;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(t!("send-save-as"));
            ui.add(egui::TextEdit::singleline(&mut self.save_name).desired_width(200.0));
            let ok = !self.save_name.trim().is_empty() && !self.text.trim().is_empty();
            if ui.add_enabled(ok, egui::Button::new(icons::with(icons::SAVE, t!("send-save")))).clicked() {
                let name = self.save_name.trim().to_string();
                let group = lib.commands.iter().find(|c| c.name == name).and_then(|c| c.group.clone());
                lib.put(Command { name: name.clone(), text: self.text.clone(), enter: self.enter, group });
                match lib.save() {
                    Ok(()) => self.picked = Some(name),
                    Err(e) => self.result = vec![(false, e.to_string())],
                }
            }
        });
    }

    pub fn show(&mut self, ctx: &egui::Context, core: &Core) -> Outcome<()> {
        self.refresh(core);
        let mut outcome = Outcome::Open;
        let mut open = true;
        let chosen: Vec<String> = self.targets.iter().filter(|t| t.chosen).map(|t| t.id.clone()).collect();
        egui::Window::new(t!("send-title"))
            .id(egui::Id::new("send-command"))
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                self.library_ui(ui);
                ui.separator();
                ui.label(t!("send-command-label"));
                let edit = ui.add(
                    egui::TextEdit::multiline(&mut self.text)
                        .code_editor()
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .hint_text("uptime"),
                );
                if std::mem::take(&mut self.focus_text) {
                    edit.request_focus();
                }
                if edit.changed() {
                    self.confirm = false;
                }
                ui.checkbox(&mut self.enter, t!("send-enter"));
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(t!("send-targets"));
                    if ui.small_button(t!("send-all")).clicked() {
                        for t in &mut self.targets {
                            t.chosen = t.state == State::Connected;
                        }
                    }
                    if ui.small_button(t!("send-none")).clicked() {
                        for t in &mut self.targets {
                            t.chosen = false;
                        }
                    }
                });
                egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    if self.targets.is_empty() {
                        ui.weak(t!("send-no-sessions"));
                    }
                    for target in &mut self.targets {
                        let ready = target.state == State::Connected;
                        ui.horizontal(|ui| {
                            ui.add_enabled(ready, egui::Checkbox::new(&mut target.chosen, &target.label));
                            if !ready {
                                ui.colored_label(AMBER, t!("send-not-logged-in", state = target.state.describe()));
                            }
                        });
                    }
                });
                ui.separator();
                let n = chosen.len();
                let can_send = n > 0 && (!self.text.is_empty() || self.enter);
                if self.confirm {
                    ui.colored_label(AMBER, t!("send-confirm", count = n));
                    ui.horizontal(|ui| {
                        if ui.button(t!("send-button", count = n)).clicked() {
                            self.send(core, &chosen);
                        }
                        if ui.button(t!("send-back")).clicked() {
                            self.confirm = false;
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        if ui.add_enabled(can_send, egui::Button::new(t!("send-button", count = n))).clicked() {
                            if n > 1 {
                                self.confirm = true;
                            } else {
                                self.send(core, &chosen);
                            }
                        }
                        if ui.button(t!("button-close")).clicked() {
                            outcome = Outcome::Cancel;
                        }
                    });
                }
                for (ok, line) in &self.result {
                    ui.colored_label(if *ok { GREEN } else { RED }, line);
                }
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }

    fn send(&mut self, core: &Core, ids: &[String]) {
        let report = core.send_text(ids, &self.text, self.enter);
        self.confirm = false;
        self.result.clear();
        if !report.sent.is_empty() {
            self.result.push((true, t!("send-result-sent", count = report.sent.len(), labels = report.sent.join(", "))));
        }
        if !report.skipped.is_empty() {
            self.result.push((false, t!("send-result-skipped", labels = report.skipped.join(", "))));
        }
        if !report.failed.is_empty() {
            self.result.push((false, t!("send-result-failed", labels = report.failed.join(", "))));
        }
        self.focus_text = true;
    }
}
