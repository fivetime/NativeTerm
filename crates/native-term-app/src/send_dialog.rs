//! "Send command": type or pick a command, choose the sessions, send.
//! Logged-in sessions get it in their tab; a session of a host kept in
//! tmux on the server (`NativeTermPersistent tmux`) that isn't logged in
//! (disconnected, reconnecting, its tab closed) can get it through tmux
//! there (`native_term_app::tmux_send`, in the background). Sending to more
//! than one asks first; every send lands in the audit log.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use native_term_app::commands::{Command, Library};
use native_term_app::tmux_send::{self, Failure};
use native_term_app::{t, Core, State};

use crate::dialogs::Outcome;
use crate::icons;

struct Target {
    id: String,
    label: String,
    alias: String,
    state: State,
    chosen: bool,
    locked: bool,
    /// Logged in with its tab linked: it can be sent to in the tab.
    linked: bool,
    /// Its tmux session on the server, if its host keeps one.
    tmux: Option<String>,
}

impl Target {
    fn direct(&self) -> bool {
        self.state == State::Connected && self.linked
    }

    /// Not in its tab, but through tmux on the server.
    fn through_tmux(&self) -> bool {
        !self.direct() && self.tmux.is_some()
    }

    fn can_send(&self) -> bool {
        self.direct() || self.through_tmux()
    }
}

/// How to reach hosts kept in tmux: the ssh to run, its config, and the
/// hosts (aliases) whose sessions run in tmux on the server.
pub struct TmuxRoute {
    pub ssh: PathBuf,
    pub config: Option<PathBuf>,
    pub hosts: HashSet<String>,
}

/// A send through tmux that has ended: the session's label and how.
type TmuxResult = (String, Result<(), Failure>);

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
    route: Option<TmuxRoute>,
    /// Sends through tmux still running, and where they report.
    pending: usize,
    tx: Sender<TmuxResult>,
    rx: Receiver<TmuxResult>,
    /// The text of the last send (for the audit log of tmux sends).
    sent_text: String,
}

const RED: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x9a, 0x1a);
const GREEN: egui::Color32 = egui::Color32::from_rgb(0x2e, 0xa0, 0x43);

impl SendDialog {
    /// `chosen`: the sessions ticked at first (all logged-in ones if empty,
    /// except locked ones and those of folders marked "No group send",
    /// which can still be ticked by hand).
    pub fn new(
        core: &Core,
        chosen: &[String],
        data_dir: &std::path::Path,
        no_group_send: &std::collections::HashSet<String>,
    ) -> SendDialog {
        let targets = core
            .sessions()
            .into_iter()
            // a closed tab's session too: tmux may still have it (see `with_tmux`)
            .filter(|s| s.state.is_open() || s.state == State::Gone)
            .map(|s| {
                let ready = s.state == State::Connected && s.linked;
                // a locked session only when asked for by name
                let all = chosen.is_empty() && !s.locked && !no_group_send.contains(&s.alias);
                let chosen = ready && (chosen.contains(&s.id) || all);
                Target {
                    id: s.id,
                    label: s.label,
                    alias: s.alias,
                    state: s.state,
                    chosen,
                    locked: s.locked,
                    linked: s.linked,
                    tmux: None,
                }
            })
            .collect();
        let library_path = Library::path_in(data_dir);
        let library = Library::load(&library_path).map_err(|e| e.to_string());
        let (tx, rx) = mpsc::channel();
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
            route: None,
            pending: 0,
            tx,
            rx,
            sent_text: String::new(),
        }
    }

    /// Hosts kept in tmux: their sessions can be sent to through tmux on
    /// the server when not logged in (and one asked for by name is ticked).
    pub fn with_tmux(mut self, route: TmuxRoute, chosen: &[String]) -> SendDialog {
        for t in &mut self.targets {
            if route.hosts.contains(&t.alias) {
                t.tmux = Some(native_term_config::persistent::session_name(&t.alias, &t.id));
                if t.through_tmux() && chosen.contains(&t.id) {
                    t.chosen = true;
                }
            }
        }
        // a closed tab's session is only worth listing if tmux keeps it
        self.targets.retain(|t| t.state != State::Gone || t.tmux.is_some());
        self.route = Some(route);
        self
    }

    /// Refresh states (a session may log in or drop while the dialog is
    /// open); a session that can't receive any more is unticked.
    fn refresh(&mut self, core: &Core) {
        let sessions = core.sessions();
        for t in &mut self.targets {
            if let Some(s) = sessions.iter().find(|s| s.id == t.id) {
                t.state = s.state.clone();
                t.linked = s.linked;
                if !t.can_send() {
                    t.chosen = false;
                }
            }
        }
    }

    fn library_ui(&mut self, ui: &mut egui::Ui) {
        let lib = match &mut self.library {
            Ok(lib) => lib,
            Err(e) => {
                ui.colored_label(
                    RED,
                    t!("send-library-error", path = self.library_path.display().to_string(), error = e.as_str()),
                );
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
                        if ui
                            .selectable_label(self.picked.as_deref() == Some(&c.name), &c.name)
                            .on_hover_text(&c.text)
                            .clicked()
                        {
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
        self.collect(core);
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
                            t.chosen = t.can_send() && !t.locked;
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
                        let ready = target.can_send();
                        ui.horizontal(|ui| {
                            ui.add_enabled(ready, egui::Checkbox::new(&mut target.chosen, &target.label));
                            if target.through_tmux() {
                                let name = target.tmux.clone().unwrap_or_default();
                                ui.weak(t!("send-through-tmux", state = target.state.describe()))
                                    .on_hover_text(t!("send-through-tmux-hint", name = name.as_str()));
                            } else if !ready {
                                ui.colored_label(AMBER, t!("send-not-logged-in", state = target.state.describe()));
                            } else if target.locked {
                                ui.weak(t!("send-locked"));
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
                            self.send(core, &chosen, ui.ctx());
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
                                self.send(core, &chosen, ui.ctx());
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
                if self.pending > 0 {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak(t!("send-tmux-pending", count = self.pending));
                    });
                }
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }

    /// Sends through tmux that have ended: their results, and the audit
    /// log for those that arrived.
    fn collect(&mut self, core: &Core) {
        while let Ok((label, result)) = self.rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            match result {
                Ok(()) => {
                    core.record_send(&[format!("{label} (tmux)")], &self.sent_text);
                    self.result.push((true, t!("send-tmux-sent", label = label.as_str())));
                }
                Err(Failure::NoSession) => self.result.push((false, t!("send-tmux-none", label = label.as_str()))),
                Err(Failure::Ssh(e)) => {
                    self.result.push((false, t!("send-tmux-failed", label = label.as_str(), error = e.as_str())))
                }
            }
        }
    }

    /// The chosen sessions that aren't logged in, through tmux on the
    /// server, each on a thread of its own (one ssh connection each).
    fn send_through_tmux(&mut self, ids: &[String], ctx: &egui::Context) {
        let Some(route) = &self.route else { return };
        let lines = native_term_app::commands::lines(&self.text, self.enter);
        for t in self.targets.iter().filter(|t| ids.contains(&t.id) && t.through_tmux()) {
            let (ssh, config, alias) = (route.ssh.clone(), route.config.clone(), t.alias.clone());
            let (name, label, lines) = (t.tmux.clone().unwrap_or_default(), t.label.clone(), lines.clone());
            let (tx, ctx) = (self.tx.clone(), ctx.clone());
            self.pending += 1;
            std::thread::spawn(move || {
                let result = tmux_send::send(&ssh, config.as_deref(), &alias, &name, &lines);
                let _ = tx.send((label, result));
                ctx.request_repaint();
            });
        }
    }

    fn send(&mut self, core: &Core, ids: &[String], ctx: &egui::Context) {
        // in their tabs: the logged-in ones; through tmux: the others
        let (direct, tmux): (Vec<String>, Vec<String>) = ids
            .iter()
            .cloned()
            .partition(|id| self.targets.iter().find(|t| &t.id == id).is_none_or(|t| !t.through_tmux()));
        let report = if direct.is_empty() {
            native_term_app::SendReport::default()
        } else {
            core.send_text(&direct, &self.text, self.enter)
        };
        self.confirm = false;
        self.result.clear();
        self.sent_text = self.text.clone();
        self.send_through_tmux(&tmux, ctx);
        if !report.sent.is_empty() {
            self.result
                .push((true, t!("send-result-sent", count = report.sent.len(), labels = report.sent.join(", "))));
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
