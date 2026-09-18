//! "Sessions on the server": NativeTerm's tmux / screen sessions on one
//! host (see `native_term_config::persistent`). One can be reopened in a
//! new tab (or its open tab shown), or ended. The server is asked with
//! `ssh -o BatchMode=yes` in the background: keys or the agent only.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use native_term_app::{t, Core, HostRequest};
use native_term_platform::Target;
use native_term_config::persistent::{self, RemoteSession};

use crate::dialogs::Outcome;

type Answer = Result<Vec<RemoteSession>, String>;

const RED: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
const GREEN: egui::Color32 = egui::Color32::from_rgb(0x2e, 0xa0, 0x43);

pub struct ServerSessionsDialog {
    alias: String,
    label: String,
    on_login: Option<String>,
    ssh: PathBuf,
    /// `-F` for a folder other than `~/.ssh`.
    config: Option<PathBuf>,
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
    ) -> ServerSessionsDialog {
        let mut dialog = ServerSessionsDialog {
            alias: alias.to_string(),
            label: label.to_string(),
            on_login,
            ssh,
            config,
            arriving: Arc::default(),
            answer: None,
            busy: false,
            confirm_end: None,
            note: None,
        };
        dialog.ask(ctx, None);
        dialog
    }

    /// List the sessions (after ending `end`, if given) in the background.
    fn ask(&mut self, ctx: &egui::Context, end: Option<RemoteSession>) {
        self.busy = true;
        self.confirm_end = None;
        let (ssh, config, alias) = (self.ssh.clone(), self.config.clone(), self.alias.clone());
        let arriving = Arc::clone(&self.arriving);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let run = |command: &str| persistent::run_remote(&ssh, config.as_deref(), &alias, command);
            let ended = match &end {
                Some(session) => run(&persistent::kill_command(session)).map(|_| ()),
                None => Ok(()),
            };
            let answer = ended.and_then(|()| run(persistent::LIST_COMMAND)).map(|out| persistent::parse_sessions(&out));
            *arriving.lock().unwrap_or_else(|e| e.into_inner()) = Some(answer);
            ctx.request_repaint();
        });
    }

    pub fn show(&mut self, ctx: &egui::Context, core: &Core) -> Outcome<()> {
        if let Some(answer) = self.arriving.lock().unwrap_or_else(|e| e.into_inner()).take() {
            self.answer = Some(answer);
            self.busy = false;
        }
        let mut outcome = Outcome::Open;
        let mut open = true;
        let mut end = None;
        let mut refresh = false;
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
                    (Some(Ok(sessions)), false) if sessions.is_empty() => {
                        ui.label(t!("server-sessions-none"));
                    }
                    (Some(Ok(sessions)), false) => {
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
                                });
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
        if end.is_some() || refresh {
            self.note = None;
            self.ask(ctx, end);
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
