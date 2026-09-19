//! "Sessions on the server": NativeTerm's tmux / screen sessions on one
//! host, or on every host of a folder kept on the server (see
//! `native_term_config::persistent`). One can be reopened in a new tab (or
//! its open tab shown), or ended; a folder's detached ones can be reopened
//! all at once (after a restart, say). A `tmux-log` session's log (an
//! ended session's too) can be read, copied here or deleted. The servers
//! are asked with `ssh -o BatchMode=yes` in the background, a few at a
//! time: keys or the agent only.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use native_term_app::{t, Core, HostRequest, SessionView};
use native_term_config::persistent::{self, RemoteSession};
use native_term_platform::Target;

use crate::dialogs::Outcome;

/// The sessions, and the sessions that have a log.
type Answer = Result<(Vec<RemoteSession>, Vec<String>), String>;

/// A log's host, name and text (or why it couldn't be read).
type LogText = (String, String, Result<String, String>);

/// A host (alias) and one of its sessions (name).
type Named = (String, String);

/// How much of a log the viewer shows (the end of it).
const TAIL: u64 = 256 * 1024;

/// Servers asked at the same time (an ssh connection each).
const AT_ONCE: usize = 6;

/// A log being read: its host and name, and the text once it's there.
struct LogView {
    alias: String,
    name: String,
    text: Option<Result<String, String>>,
    confirm_delete: bool,
}

/// What to do on a host before listing it again.
enum Change {
    End(RemoteSession),
    DeleteLog(String),
}

const RED: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
const GREEN: egui::Color32 = egui::Color32::from_rgb(0x2e, 0xa0, 0x43);

pub struct ServerSessionsDialog {
    /// A host's or a folder's label.
    title: String,
    /// Alias, label and `on_login` of each host asked.
    hosts: Vec<HostRequest>,
    ssh: PathBuf,
    /// `-F` for a folder other than `~/.ssh`.
    config: Option<PathBuf>,
    /// Copies of logs go to `<data dir>\logs\server\<alias>`.
    data_dir: PathBuf,
    log: Option<LogView>,
    /// A log's text, or a copy's result, from the background.
    log_arriving: Arc<Mutex<Option<LogText>>>,
    copy_arriving: Arc<Mutex<Option<String>>>,
    /// Where the background asks put their answers.
    arriving: Arc<Mutex<Vec<(String, Answer)>>>,
    /// Each host's answer; a host missing here is being asked.
    answers: HashMap<String, Answer>,
    /// The session whose "End" was clicked once.
    confirm_end: Option<Named>,
    note: Option<String>,
}

impl ServerSessionsDialog {
    pub fn new(
        ctx: &egui::Context,
        title: &str,
        hosts: Vec<HostRequest>,
        ssh: PathBuf,
        config: Option<PathBuf>,
        data_dir: PathBuf,
    ) -> ServerSessionsDialog {
        let mut dialog = ServerSessionsDialog {
            title: title.to_string(),
            hosts,
            ssh,
            config,
            data_dir,
            log: None,
            log_arriving: Arc::default(),
            copy_arriving: Arc::default(),
            arriving: Arc::default(),
            answers: HashMap::new(),
            confirm_end: None,
            note: None,
        };
        let all = dialog.aliases();
        dialog.ask(ctx, all, None);
        dialog
    }

    fn aliases(&self) -> Vec<String> {
        self.hosts.iter().map(|h| h.alias.clone()).collect()
    }

    /// Read the end of a session's log in the background.
    fn read_log(&mut self, ctx: &egui::Context, (alias, name): Named) {
        self.log = Some(LogView { alias: alias.clone(), name: name.clone(), text: None, confirm_delete: false });
        let (ssh, config) = (self.ssh.clone(), self.config.clone());
        let arriving = Arc::clone(&self.log_arriving);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let command = persistent::log_tail_command(&name, TAIL);
            let text = persistent::run_remote_bytes(&ssh, config.as_deref(), &alias, &command)
                .map(|b| persistent::log_text(&b));
            *arriving.lock().unwrap_or_else(|e| e.into_inner()) = Some((alias, name, text));
            ctx.request_repaint();
        });
    }

    /// Copy a whole log to this computer in the background, then show it
    /// in Explorer.
    fn copy_log(&mut self, ctx: &egui::Context, (alias, name): Named) {
        let (ssh, config) = (self.ssh.clone(), self.config.clone());
        let folder = self.data_dir.join("logs").join("server").join(sanitize(&alias));
        let arriving = Arc::clone(&self.copy_arriving);
        let ctx = ctx.clone();
        self.note = Some(t!("server-log-copying", name = name.as_str()));
        std::thread::spawn(move || {
            let copied =
                persistent::run_remote_bytes(&ssh, config.as_deref(), &alias, &persistent::log_cat_command(&name))
                    .and_then(|bytes| {
                        let file = folder.join(format!("{name}.log"));
                        std::fs::create_dir_all(&folder)
                            .and_then(|()| std::fs::write(&file, bytes))
                            .map_err(|e| e.to_string())?;
                        Ok(file)
                    });
            let note = match copied {
                Ok(file) => {
                    let _ =
                        std::process::Command::new("explorer.exe").arg(format!("/select,{}", file.display())).spawn();
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
    fn log_window(&mut self, ctx: &egui::Context, delete: &mut Option<Named>, copy: &mut Option<Named>) {
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
                        *copy = Some((view.alias.clone(), view.name.clone()));
                    }
                    if view.confirm_delete {
                        let button = egui::Button::new(egui::RichText::new(t!("server-log-delete-now")).color(RED));
                        if ui.add(button).clicked() {
                            *delete = Some((view.alias.clone(), view.name.clone()));
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

    /// List the sessions of `aliases` in the background, `AT_ONCE` hosts at
    /// a time; `change` is made first on its host.
    fn ask(&mut self, ctx: &egui::Context, aliases: Vec<String>, change: Option<(String, Change)>) {
        self.confirm_end = None;
        for alias in &aliases {
            self.answers.remove(alias);
        }
        let workers = aliases.len().min(AT_ONCE);
        let queue = Arc::new(Mutex::new(aliases));
        let change = Arc::new(Mutex::new(change));
        for _ in 0..workers {
            let (ssh, config) = (self.ssh.clone(), self.config.clone());
            let (queue, change, arriving) = (Arc::clone(&queue), Arc::clone(&change), Arc::clone(&self.arriving));
            let ctx = ctx.clone();
            std::thread::spawn(move || loop {
                let Some(alias) = queue.lock().unwrap_or_else(|e| e.into_inner()).pop() else { return };
                let run = |command: &str| persistent::run_remote(&ssh, config.as_deref(), &alias, command);
                let mine = {
                    let mut change = change.lock().unwrap_or_else(|e| e.into_inner());
                    if change.as_ref().is_some_and(|(a, _)| *a == alias) {
                        change.take().map(|(_, c)| c)
                    } else {
                        None
                    }
                };
                let changed = match mine {
                    Some(Change::End(session)) => run(&persistent::kill_command(&session)).map(|_| ()),
                    Some(Change::DeleteLog(name)) => run(&persistent::log_delete_command(&name)).map(|_| ()),
                    None => Ok(()),
                };
                let answer = changed
                    .and_then(|()| run(persistent::LIST_COMMAND))
                    .map(|out| (persistent::parse_sessions(&out), persistent::parse_logs(&out)));
                arriving.lock().unwrap_or_else(|e| e.into_inner()).push((alias, answer));
                ctx.request_repaint();
            });
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, core: &Core) -> Outcome<()> {
        for (alias, answer) in self.arriving.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            self.answers.insert(alias, answer);
        }
        if let Some((alias, name, text)) = self.log_arriving.lock().unwrap_or_else(|e| e.into_inner()).take() {
            if let Some(view) = self.log.as_mut().filter(|v| v.alias == alias && v.name == name) {
                view.text = Some(text);
            }
        }
        if let Some(note) = self.copy_arriving.lock().unwrap_or_else(|e| e.into_inner()).take() {
            self.note = Some(note);
        }
        let mut outcome = Outcome::Open;
        let mut open = true;
        let mut change = None;
        let mut refresh = false;
        let mut read = None;
        let mut reopen = Vec::new();
        let (mut delete, mut copy) = (None, None);
        let busy = self.hosts.iter().any(|h| !self.answers.contains_key(&h.alias));
        let tabs = core.sessions();
        let several = self.hosts.len() > 1;
        let detached = if several { self.detached(&tabs) } else { Vec::new() };
        egui::Window::new(t!("server-sessions-title", label = self.title.as_str()))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.weak(t!("server-sessions-intro"));
                ui.add_space(6.0);
                if self.hosts.is_empty() {
                    ui.label(t!("server-sessions-no-hosts"));
                }
                egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                    for host in self.hosts.clone() {
                        if several {
                            ui.add_space(4.0);
                            ui.strong(host.label.as_str()).on_hover_text(host.alias.as_str());
                        }
                        // two aliases of one server both list its sessions:
                        // each shows under its own host only
                        let answer = self.answers.get(&host.alias).cloned().map(|a| {
                            a.map(|(mut sessions, logs)| {
                                sessions.retain(|s| !another_hosts(&self.hosts, &host.alias, &s.name));
                                (sessions, logs)
                            })
                        });
                        match answer {
                            None => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(t!("server-sessions-asking"));
                                });
                            }
                            Some(Err(error)) => {
                                ui.colored_label(RED, error);
                                ui.weak(t!("server-sessions-batch"));
                            }
                            Some(Ok((sessions, logs))) if sessions.is_empty() && logs.is_empty() => {
                                ui.weak(t!("server-sessions-none"));
                            }
                            Some(Ok((sessions, logs))) => {
                                let mut picked = Picked::default();
                                self.rows(ui, core, &tabs, &host.alias, &sessions, &logs, &mut picked);
                                change = change.take().or(picked.change);
                                read = read.take().or(picked.read);
                                reopen.extend(picked.reopen);
                            }
                        }
                    }
                });
                if let Some(note) = &self.note {
                    ui.add_space(4.0);
                    ui.weak(note);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new(t!("server-sessions-refresh"))).clicked() {
                        refresh = true;
                    }
                    if !detached.is_empty() {
                        let button = egui::Button::new(t!("server-sessions-open-all", count = detached.len()));
                        if ui.add(button).on_hover_text(t!("server-sessions-open-all-hint")).clicked() {
                            reopen.extend(detached.iter().cloned());
                        }
                    }
                    if ui.button(t!("button-close")).clicked() {
                        outcome = Outcome::Cancel;
                    }
                });
            });
        if let Some(named) = read {
            self.read_log(ctx, named);
        }
        self.log_window(ctx, &mut delete, &mut copy);
        if let Some(named) = copy {
            self.copy_log(ctx, named);
        }
        if !reopen.is_empty() {
            self.reopen(core, &reopen);
        }
        if let Some((alias, name)) = delete {
            change = Some((alias, Change::DeleteLog(name)));
        }
        if let Some((alias, what)) = change {
            self.note = None;
            self.ask(ctx, vec![alias.clone()], Some((alias, what)));
        } else if refresh {
            self.note = None;
            let all = self.aliases();
            self.ask(ctx, all, None);
        }
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }

    /// One host's sessions, and the logs of its ended sessions.
    #[allow(clippy::too_many_arguments)]
    fn rows(
        &mut self,
        ui: &mut egui::Ui,
        core: &Core,
        tabs: &[SessionView],
        alias: &str,
        sessions: &[RemoteSession],
        logs: &[String],
        picked: &mut Picked,
    ) {
        let named = |name: &str| (alias.to_string(), name.to_string());
        egui::Grid::new(("server-sessions", alias)).num_columns(4).spacing([14.0, 6.0]).show(ui, |ui| {
            for session in sessions {
                ui.label(&session.name);
                let when = session.created.map(native_term_win::local_date_time).unwrap_or_default();
                ui.weak(format!("{} {when}", session.program.name()));
                // a tab of this NativeTerm has it: show that tab
                let tab = in_tab(tabs, alias, &session.name);
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
                    } else if persistent::belongs_to(alias, &session.name)
                        && ui.small_button(t!("server-sessions-open")).clicked()
                    {
                        picked.reopen.push(named(&session.name));
                    }
                    if self.confirm_end.as_ref() == Some(&named(&session.name)) {
                        let button = egui::Button::new(egui::RichText::new(t!("server-sessions-end-now")).color(RED));
                        if ui.add(button.small()).clicked() {
                            picked.change = Some((alias.to_string(), Change::End(session.clone())));
                        }
                        if ui.small_button(t!("server-sessions-keep")).clicked() {
                            self.confirm_end = None;
                        }
                    } else if ui
                        .small_button(t!("server-sessions-end"))
                        .on_hover_text(t!("server-sessions-end-hint"))
                        .clicked()
                    {
                        self.confirm_end = Some(named(&session.name));
                    }
                    if logs.contains(&session.name) && ui.small_button(t!("server-log-view")).clicked() {
                        picked.read = Some(named(&session.name));
                    }
                });
                ui.end_row();
            }
            // the logs of ended sessions (this host's)
            for name in logs {
                if sessions.iter().any(|s| &s.name == name) || !persistent::belongs_to(alias, name) {
                    continue;
                }
                ui.label(name);
                ui.weak("tmux");
                ui.weak(t!("server-log-ended"));
                if ui.small_button(t!("server-log-view")).clicked() {
                    picked.read = Some(named(name));
                }
                ui.end_row();
            }
        });
    }

    /// The sessions of this NativeTerm's hosts that no one is attached to
    /// and no tab has: the ones "Open All" opens.
    fn detached(&self, tabs: &[SessionView]) -> Vec<Named> {
        let answers = self.hosts.iter().filter_map(|h| match self.answers.get(&h.alias) {
            Some(Ok((sessions, _))) => Some((h.alias.as_str(), sessions)),
            _ => None,
        });
        reopenable(answers, |alias, name| in_tab(tabs, alias, name).is_some())
    }

    /// New tabs attached to these sessions: each one's session id is made
    /// so the shim names the server-side session the same again.
    fn reopen(&mut self, core: &Core, sessions: &[Named]) {
        let requests: Vec<HostRequest> = sessions
            .iter()
            .filter_map(|(alias, name)| {
                let host = self.hosts.iter().find(|h| h.alias == *alias)?;
                let id = persistent::session_id_for(alias, name, &native_term_config::new_id())?;
                Some(HostRequest { session: Some(id), ..host.clone() })
            })
            .collect();
        if requests.is_empty() {
            return;
        }
        core.open(&requests, Target::Recent);
        self.note = Some(match sessions {
            [(_, name)] => t!("server-sessions-opened", name = name.as_str()),
            _ => t!("server-sessions-opened-several", count = requests.len()),
        });
    }
}

/// What was clicked in a host's rows.
#[derive(Default)]
struct Picked {
    change: Option<(String, Change)>,
    read: Option<Named>,
    reopen: Vec<Named>,
}

/// The open tab of this NativeTerm that has the host's session `name`.
fn in_tab<'a>(tabs: &'a [SessionView], alias: &str, name: &str) -> Option<&'a SessionView> {
    tabs.iter().find(|s| s.state.is_open() && s.alias == alias && persistent::session_name(&s.alias, &s.id) == name)
}

/// The sessions of each host (alias) that are NativeTerm's for that host,
/// attached nowhere and not in a tab (`in_tab`).
fn reopenable<'a>(
    answers: impl Iterator<Item = (&'a str, &'a Vec<RemoteSession>)>,
    in_tab: impl Fn(&str, &str) -> bool,
) -> Vec<Named> {
    answers
        .flat_map(|(alias, sessions)| {
            sessions
                .iter()
                .filter(|s| !s.attached && persistent::belongs_to(alias, &s.name) && !in_tab(alias, &s.name))
                .map(move |s| (alias.to_string(), s.name.clone()))
        })
        .collect()
}

/// The session is NativeTerm's for another of `hosts` than `alias`.
fn another_hosts(hosts: &[HostRequest], alias: &str, name: &str) -> bool {
    hosts.iter().any(|h| h.alias != alias && persistent::belongs_to(&h.alias, name))
}

/// A folder name from an alias.
fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str, attached: bool) -> RemoteSession {
        RemoteSession { name: name.to_string(), program: persistent::Persistence::Tmux, attached, created: None }
    }

    #[test]
    fn a_session_shows_under_its_own_host() {
        let hosts = [HostRequest::new("web", "Web"), HostRequest::new("web-b", "B")];
        assert!(another_hosts(&hosts, "web", "nt-web-b-4e5f6a7b"));
        assert!(!another_hosts(&hosts, "web", "nt-web-0a1b2c3d"));
        assert!(!another_hosts(&hosts, "web-b", "nt-web-b-4e5f6a7b"));
        assert!(another_hosts(&hosts, "web-b", "nt-web-0a1b2c3d"));
        // not NativeTerm's for any of them: shown under each
        assert!(!another_hosts(&hosts, "web", "work"));
        assert!(!another_hosts(&hosts[..1], "web", "nt-web-b-4e5f6a7b"));
    }

    #[test]
    fn the_sessions_open_all_opens() {
        let web = vec![
            session("nt-web-0f3a9c21", false),
            session("nt-web-11111111", true),
            session("nt-web-22222222", false),
            session("nt-db-33333333", false),
            session("mine", false),
        ];
        let db = vec![session("nt-db-44444444", false)];
        let answers = [("web", &web), ("db", &db)].into_iter();
        let open = reopenable(answers, |alias, name| alias == "web" && name == "nt-web-22222222");
        assert_eq!(
            open,
            vec![("web".to_string(), "nt-web-0f3a9c21".to_string()), ("db".to_string(), "nt-db-44444444".to_string())]
        );
    }
}
