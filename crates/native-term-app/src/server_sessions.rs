//! "Sessions on the server": NativeTerm's tmux / screen sessions on one
//! host (see `native_term_config::persistent`). One can be reopened in a
//! new tab (or its open tab shown), or ended. A `tmux-log` session's log
//! (an ended session's too) can be read, copied here or deleted. The
//! server is asked with `ssh -o BatchMode=yes` in the background: keys or
//! the agent only.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use native_term_app::{t, Core, HostRequest};
use native_term_platform::Target;
use native_term_config::persistent::{self, RemoteSession};

use crate::dialogs::Outcome;

/// The sessions, and the sessions that have a log.
type Answer = Result<(Vec<RemoteSession>, Vec<String>), String>;

/// A log's name and its text (or why it couldn't be read).
type LogText = (String, Result<String, String>);

/// How much of a log the viewer shows (the end of it).
const TAIL: u64 = 256 * 1024;

/// A log being read: its name, and the text once it's there.
struct LogView {
    name: String,
    text: Option<Result<String, String>>,
    confirm_delete: bool,
}

const RED: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
const GREEN: egui::Color32 = egui::Color32::from_rgb(0x2e, 0xa0, 0x43);

pub struct ServerSessionsDialog {
    alias: String,
    label: String,
    on_login: Option<String>,
    ssh: PathBuf,
    /// `-F` for a folder other than `~/.ssh`.
    config: Option<PathBuf>,
    /// Copies of logs go to `<data dir>\logs\server\<alias>`.
    data_dir: PathBuf,
    log: Option<LogView>,
    /// A log's text, or a copy's result, from the background.
    log_arriving: Arc<Mutex<Option<LogText>>>,
    copy_arriving: Arc<Mutex<Option<String>>>,
    /// Where the background ask puts its answer.
    arriving: Arc<Mutex<Option<Answer>>>,
    answer: Option<Answer>,
    busy: bool,
    /// The session whose "End" was clicked once.
    confirm_end: Option<String>,
    note: Option<String>,
}

impl ServerSessionsDialog {
    pub fn new(
        ctx: &egui::Context,
        alias: &str,
        label: &str,
        on_login: Option<String>,
        ssh: PathBuf,
        config: Option<PathBuf>,
        data_dir: PathBuf,
    ) -> ServerSessionsDialog {
        let mut dialog = ServerSessionsDialog {
            alias: alias.to_string(),
            label: label.to_string(),
            on_login,
            ssh,
            config,
            data_dir,
            log: None,
            log_arriving: Arc::default(),
            copy_arriving: Arc::default(),
            arriving: Arc::default(),
            answer: None,
            busy: false,
            confirm_end: None,
            note: None,
        };
        dialog.ask(ctx, None, None);
        dialog
    }

    /// Read the end of a session's log in the background.
    fn read_log(&mut self, ctx: &egui::Context, name: &str) {
        self.log = Some(LogView { name: name.to_string(), text: None, confirm_delete: false });
        let (ssh, config, alias, name) = (self.ssh.clone(), self.config.clone(), self.alias.clone(), name.to_string());
        let arriving = Arc::clone(&self.log_arriving);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let command = persistent::log_tail_command(&name, TAIL);
            let text = persistent::run_remote_bytes(&ssh, config.as_deref(), &alias, &command).map(|b| persistent::log_text(&b));
            *arriving.lock().unwrap_or_else(|e| e.into_inner()) = Some((name, text));
            ctx.request_repaint();
        });
    }

    /// Copy a whole log to this computer in the background, then show it
    /// in Explorer.
    fn copy_log(&mut self, ctx: &egui::Context, name: &str) {
        let (ssh, config, alias, name) = (self.ssh.clone(), self.config.clone(), self.alias.clone(), name.to_string());
        let folder = self.data_dir.join("logs").join("server").join(sanitize(&alias));
        let arriving = Arc::clone(&self.copy_arriving);
        let ctx = ctx.clone();
        self.note = Some(t!("server-log-copying", name = name.as_str()));
        std::thread::spawn(move || {
            let copied = persistent::run_remote_bytes(&ssh, config.as_deref(), &alias, &persistent::log_cat_command(&name))
                .and_then(|bytes| {
                    let file = folder.join(format!("{name}.log"));
                    std::fs::create_dir_all(&folder).and_then(|()| std::fs::write(&file, bytes)).map_err(|e| e.to_string())?;
                    Ok(file)
                });
            let note = match copied {
                Ok(file) => {
                    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", file.display())).spawn();
                    t!("server-log-copied", file = file.display().to_string())
                }
                Err(e) => t!("server-log-failed", error = e),
            };
            *arriving.lock().unwrap_or_else(|e| e.into_inner()) = Some(note);
            ctx.request_repaint();
        });
    }

    /// The log viewer; sets `delete` / `copy` to the log to delete on the
    /// server / copy here.
    fn log_window(&mut self, ctx: &egui::Context, delete: &mut Option<String>, copy: &mut Option<String>) {
        let Some(view) = &mut self.log else { return };
        let mut open = true;
        egui::Window::new(t!("server-log-title", name = view.name.as_str()))
            .id(egui::Id::new("server-log"))
            .open(&mut open)
            .default_size([760.0, 480.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let ready = matches!(view.text, Some(Ok(_)));
                    let button = egui::Button::new(t!("server-log-copy"));
                    if ui.add_enabled(ready, button).on_hover_text(t!("server-log-copy-hint")).clicked() {
                        *copy = Some(view.name.clone());
                    }
                    if view.confirm_delete {
                        let button = egui::Button::new(egui::RichText::new(t!("server-log-delete-now")).color(RED));
                        if ui.add(button).clicked() {
                            *delete = Some(view.name.clone());
                        }
                        if ui.button(t!("server-sessions-keep")).clicked() {
                            view.confirm_delete = false;
                        }
                    } else if ui.button(t!("server-log-delete")).clicked() {
                        view.confirm_delete = true;
                    }
                });
                ui.weak(t!("server-log-note", file = persistent::log_file(&view.name)));
                ui.separator();
                match &view.text {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(t!("server-sessions-asking"));
                        });
                    }
                    Some(Err(error)) => {
                        ui.colored_label(RED, error);
                    }
                    Some(Ok(text)) => {
                        egui::ScrollArea::both().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                            // selectable, not editable
                            let mut shown = text.as_str();
                            let edit = egui::TextEdit::multiline(&mut shown).font(egui::TextStyle::Monospace);
                            ui.add(edit.desired_width(f32::INFINITY));
                        });
                    }
                }
            });
        if !open || delete.is_some() {
            self.log = None;
        }
    }

    /// List the sessions (after ending `end` or deleting the log of
    /// `delete_log`, if given) in the background.
    fn ask(&mut self, ctx: &egui::Context, end: Option<RemoteSession>, delete_log: Option<String>) {
        self.busy = true;
        self.confirm_end = None;
        let (ssh, config, alias) = (self.ssh.clone(), self.config.clone(), self.alias.clone());
        let arriving = Arc::clone(&self.arriving);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let run = |command: &str| persistent::run_remote(&ssh, config.as_deref(), &alias, command);
            let ended = match (&end, &delete_log) {
                (Some(session), _) => run(&persistent::kill_command(session)).map(|_| ()),
                (None, Some(name)) => run(&persistent::log_delete_command(name)).map(|_| ()),
                (None, None) => Ok(()),
            };
            let answer = ended
                .and_then(|()| run(persistent::LIST_COMMAND))
                .map(|out| (persistent::parse_sessions(&out), persistent::parse_logs(&out)));
            *arriving.lock().unwrap_or_else(|e| e.into_inner()) = Some(answer);
            ctx.request_repaint();
        });
    }

    pub fn show(&mut self, ctx: &egui::Context, core: &Core) -> Outcome<()> {
        if let Some(answer) = self.arriving.lock().unwrap_or_else(|e| e.into_inner()).take() {
            self.answer = Some(answer);
            self.busy = false;
        }
        if let Some((name, text)) = self.log_arriving.lock().unwrap_or_else(|e| e.into_inner()).take() {
            if let Some(view) = self.log.as_mut().filter(|v| v.name == name) {
                view.text = Some(text);
            }
        }
        if let Some(note) = self.copy_arriving.lock().unwrap_or_else(|e| e.into_inner()).take() {
            self.note = Some(note);
        }
        let mut outcome = Outcome::Open;
        let mut open = true;
        let mut end = None;
        let mut refresh = false;
        let mut read = None;
        let (mut delete, mut copy) = (None, None);
        egui::Window::new(t!("server-sessions-title", label = self.label.as_str()))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.weak(t!("server-sessions-intro"));
                ui.add_space(6.0);
                let answer = self.answer.clone();
                match (&answer, self.busy) {
                    (_, true) => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(t!("server-sessions-asking"));
                        });
                    }
                    (Some(Err(error)), false) => {
                        ui.colored_label(RED, error);
                        ui.weak(t!("server-sessions-batch"));
                    }
                    (Some(Ok((sessions, logs))), false) if sessions.is_empty() && logs.is_empty() => {
                        ui.label(t!("server-sessions-none"));
                    }
                    (Some(Ok((sessions, logs))), false) => {
                        let tabs = core.sessions();
                        egui::Grid::new("server-sessions").num_columns(4).spacing([14.0, 6.0]).show(ui, |ui| {
                            for session in sessions {
                                ui.label(&session.name);
                                let when = session.created.map(native_term_win::local_date_time).unwrap_or_default();
                                ui.weak(format!("{} {when}", session.program.name()));
                                // a tab of this NativeTerm has it: show that tab
                                let tab = tabs.iter().find(|s| {
                                    s.state.is_open()
                                        && s.alias == self.alias
                                        && persistent::session_name(&s.alias, &s.id) == session.name
                                });
                                match (tab, session.attached) {
                                    (Some(tab), _) => ui.colored_label(GREEN, t!("server-sessions-in-tab", label = tab.label.as_str())),
                                    (None, true) => ui.label(t!("server-sessions-attached")),
                                    (None, false) => ui.weak(t!("server-sessions-detached")),
                                };
                                ui.horizontal(|ui| {
                                    if let Some(tab) = tab {
                                        if ui.small_button(t!("server-sessions-show")).clicked() {
                                            core.run(&tab.id, native_term_app::actions::SessionCommand::Focus);
                                        }
                                    } else if persistent::belongs_to(&self.alias, &session.name)
                                        && ui.small_button(t!("server-sessions-open")).clicked()
                                    {
                                        self.reopen(core, &session.name);
                                    }
                                    if self.confirm_end.as_deref() == Some(session.name.as_str()) {
                                        let button = egui::Button::new(egui::RichText::new(t!("server-sessions-end-now")).color(RED));
                                        if ui.add(button.small()).clicked() {
                                            end = Some(session.clone());
                                        }
                                        if ui.small_button(t!("server-sessions-keep")).clicked() {
                                            self.confirm_end = None;
                                        }
                                    } else if ui.small_button(t!("server-sessions-end")).on_hover_text(t!("server-sessions-end-hint")).clicked() {
                                        self.confirm_end = Some(session.name.clone());
                                    }
                                    if logs.contains(&session.name) && ui.small_button(t!("server-log-view")).clicked() {
                                        read = Some(session.name.clone());
                                    }
                                });
                                ui.end_row();
                            }
                            // the logs of ended sessions (this host's)
                            for name in logs {
                                if sessions.iter().any(|s| &s.name == name) || !persistent::belongs_to(&self.alias, name) {
                                    continue;
                                }
                                ui.label(name);
                                ui.weak("tmux");
                                ui.weak(t!("server-log-ended"));
                                if ui.small_button(t!("server-log-view")).clicked() {
                                    read = Some(name.clone());
                                }
                                ui.end_row();
                            }
                        });
                    }
                    (None, false) => {}
                }
                if let Some(note) = &self.note {
                    ui.add_space(4.0);
                    ui.weak(note);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!self.busy, egui::Button::new(t!("server-sessions-refresh"))).clicked() {
                        refresh = true;
                    }
                    if ui.button(t!("button-close")).clicked() {
                        outcome = Outcome::Cancel;
                    }
                });
            });
        if let Some(name) = read {
            self.read_log(ctx, &name);
        }
        self.log_window(ctx, &mut delete, &mut copy);
        if let Some(name) = copy {
            self.copy_log(ctx, &name);
        }
        if end.is_some() || refresh || delete.is_some() {
            self.note = None;
            self.ask(ctx, end, delete);
        }
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }

    /// A new tab attached to `name`: its session id is made so the shim
    /// names the server-side session `name` again.
    fn reopen(&mut self, core: &Core, name: &str) {
        let Some(id) = persistent::session_id_for(&self.alias, name, &native_term_config::new_id()) else { return };
        let request =
            HostRequest { session: Some(id), on_login: self.on_login.clone(), ..HostRequest::new(&self.alias, &self.label) };
        core.open(&[request], Target::Recent);
        self.note = Some(t!("server-sessions-opened", name = name));
    }
}

/// A folder name from an alias.
fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect()
}
