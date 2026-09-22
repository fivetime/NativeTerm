//! "Import from SecureCRT / PuTTY": pick the folder (SecureCRT), preview
//! what would happen, then import in the background.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use native_term_app::import::{self, Line};
use native_term_app::t;
use native_term_config::ops::ImportOutcome;
#[cfg(windows)]
use native_term_config::putty;
use native_term_config::securecrt::{self, Origin, Plan, Scan};
use native_term_config::SessionTree;

use crate::dialogs::Outcome;

/// A scan's result: the sessions, and the button bars' commands with their
/// preview lines.
type Scanned = Result<(Scan, Vec<Line>, Vec<crate::commands_import::Imported>), String>;

enum Step {
    Choose,
    /// Reading the configuration in the background: a folder on OneDrive
    /// may have to be downloaded first, which took long enough to freeze
    /// the window when it was read on the UI thread.
    Scanning {
        rx: Receiver<Scanned>,
    },
    /// `commands`: SecureCRT's send-string buttons, for the command library.
    Preview {
        plan: Box<Plan>,
        lines: Vec<Line>,
        commands: Vec<crate::commands_import::Imported>,
    },
    Running {
        rx: Receiver<(Result<ImportOutcome, String>, Option<String>)>,
        done: Arc<AtomicUsize>,
        total: usize,
    },
    Finished {
        text: Vec<String>,
        wrote: bool,
    },
}

pub struct ImportDialog {
    origin: Origin,
    path: String,
    step: Step,
    error: Option<String>,
    ssh_dir: PathBuf,
    data_dir: PathBuf,
}

const RED: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x9a, 0x1a);

impl ImportDialog {
    pub fn new(ssh_dir: PathBuf, data_dir: PathBuf) -> ImportDialog {
        let path = import::securecrt_config_path().map(|p| p.display().to_string()).unwrap_or_default();
        ImportDialog { origin: Origin::SecureCrt, path, step: Step::Choose, error: None, ssh_dir, data_dir }
    }

    /// PuTTY's saved sessions, previewed right away.
    pub fn putty(ssh_dir: PathBuf, data_dir: PathBuf, ctx: &egui::Context) -> ImportDialog {
        #[cfg(windows)]
        let path = format!(r"HKEY_CURRENT_USER\{}", putty::sessions_key());
        #[cfg(not(windows))]
        let path = String::new();
        let mut dialog =
            ImportDialog { origin: Origin::Putty, path, step: Step::Choose, error: None, ssh_dir, data_dir };
        dialog.preview(ctx);
        dialog
    }

    /// Reads the configuration in the background; `scanned` makes the
    /// preview once it's there.
    fn preview(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let (origin, path, ctx) = (self.origin, PathBuf::from(self.path.trim()), ctx.clone());
        std::thread::spawn(move || {
            let scanned = match origin {
                Origin::SecureCrt => securecrt::scan(&path),
                #[cfg(windows)]
                Origin::Putty => putty::scan(),
                #[cfg(not(windows))]
                Origin::Putty => Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "no PuTTY here")),
            };
            let result = scanned.map_err(|e| e.to_string()).map(|scan| {
                let mut lines = Vec::new();
                let commands = match origin {
                    Origin::SecureCrt => crate::commands_import::scan(&path, &mut lines),
                    Origin::Putty => Vec::new(),
                };
                (scan, lines, commands)
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.error = None;
        self.step = Step::Scanning { rx };
    }

    /// The preview of a finished scan (planning against the tree is quick).
    fn scanned(&mut self, scanned: Scanned, tree: &SessionTree) {
        match scanned {
            Ok((scan, button_lines, commands)) => {
                let plan = securecrt::plan(&scan, tree);
                let mut lines = import::summary(&scan, &plan);
                lines.extend(button_lines);
                self.step = Step::Preview { plan: Box::new(plan), lines, commands };
            }
            Err(e) => {
                self.error = Some(e);
                self.step = Step::Choose;
            }
        }
    }

    fn start(&mut self, plan: Plan, commands: Vec<crate::commands_import::Imported>, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let done = Arc::new(AtomicUsize::new(0));
        let total = plan.host_count();
        let editor = crate::app::editor_for(&self.ssh_dir, &self.data_dir);
        let progress = Arc::clone(&done);
        let ctx = ctx.clone();
        let library = native_term_app::commands::Library::path_in(&self.data_dir);
        std::thread::spawn(move || {
            let commands = (!commands.is_empty()).then(|| crate::commands_import::add(&library, commands));
            let repaint = ctx.clone();
            let result = editor
                .import(&plan, &move |n, _| {
                    progress.store(n, Ordering::Relaxed);
                    repaint.request_repaint();
                })
                .map_err(|e| e.to_string());
            let _ = tx.send((result, commands));
            ctx.request_repaint();
        });
        self.step = Step::Running { rx, done, total };
    }

    /// `Submit(())` once something was written (the tree needs a reload).
    pub fn show(&mut self, ctx: &egui::Context, tree: &SessionTree) -> Outcome<()> {
        if let Step::Scanning { rx } = &self.step {
            match rx.try_recv() {
                Ok(scanned) => self.scanned(scanned, tree),
                Err(mpsc::TryRecvError::Disconnected) => self.step = Step::Choose,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Step::Running { rx, .. } = &self.step {
            if let Ok((result, commands)) = rx.try_recv() {
                self.step = match result {
                    Ok(o) => {
                        let mut text = vec![t!("import-done", hosts = o.hosts(), folders = o.written.len())];
                        if o.keys_added > 0 {
                            text.push(t!("import-keys-added", count = o.keys_added));
                        }
                        if let Some(e) = &o.keys_failed {
                            text.push(t!("import-keys-failed", error = e.as_str()));
                        }
                        text.extend(o.failed.iter().map(|(label, why)| {
                            t!("import-folder-failed", folder = label.as_str(), error = why.as_str())
                        }));
                        text.extend(commands);
                        Step::Finished { text, wrote: o.hosts() > 0 || o.keys_added > 0 }
                    }
                    Err(e) => Step::Finished { text: vec![t!("import-failed", error = e)], wrote: false },
                };
            }
        }
        let mut outcome = Outcome::Open;
        let mut open = true;
        let running = matches!(self.step, Step::Running { .. });
        let mut start = None;
        let (title, folder_label) = match self.origin {
            Origin::SecureCrt => (t!("import-title"), t!("import-folder-label")),
            Origin::Putty => (t!("import-title-putty"), t!("import-putty-from")),
        };
        let editable = self.origin == Origin::SecureCrt && matches!(self.step, Step::Choose | Step::Preview { .. });
        egui::Window::new(title)
            .id(egui::Id::new("import-sessions"))
            .collapsible(false)
            .resizable(true)
            .default_width(640.0)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(folder_label);
                    ui.add_enabled(
                        editable,
                        egui::TextEdit::singleline(&mut self.path).hint_text("…\\VanDyke\\Config").desired_width(360.0),
                    );
                });
                ui.weak(t!("import-into", path = self.ssh_dir.display().to_string()));
                if let Some(e) = &self.error {
                    ui.colored_label(RED, e);
                }
                ui.separator();
                match &mut self.step {
                    Step::Choose => {
                        if self.path.is_empty() {
                            ui.label(t!("import-not-found"));
                        }
                    }
                    Step::Scanning { .. } => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(t!("import-scanning"));
                        });
                    }
                    Step::Preview { lines, .. } => {
                        // lines: the sessions' summary, then the buttons'

                        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            for (i, l) in lines.iter().enumerate() {
                                let text = if l.warning {
                                    egui::RichText::new(&l.text).color(AMBER)
                                } else {
                                    egui::RichText::new(&l.text)
                                };
                                if l.details.is_empty() {
                                    ui.label(text);
                                } else {
                                    egui::CollapsingHeader::new(text).id_salt(("import-line", i)).show(ui, |ui| {
                                        for d in &l.details {
                                            ui.label(d);
                                        }
                                    });
                                }
                            }
                        });
                    }
                    Step::Running { done, total, .. } => {
                        let n = done.load(Ordering::Relaxed);
                        ui.add(
                            egui::ProgressBar::new(n as f32 / (*total).max(1) as f32).text(format!("{n} / {total}")),
                        );
                        ui.weak(t!("import-checking"));
                    }
                    Step::Finished { text, .. } => {
                        for t in text {
                            ui.label(t.as_str());
                        }
                    }
                }
                ui.separator();
                ui.horizontal(|ui| match &self.step {
                    Step::Choose | Step::Preview { .. } => {
                        if ui
                            .add_enabled(!self.path.trim().is_empty(), egui::Button::new(t!("import-preview")))
                            .clicked()
                        {
                            self.preview(ctx);
                        }
                        if let Step::Preview { plan, commands, .. } = &self.step {
                            let n = plan.host_count();
                            let keys = !plan.host_keys.is_empty();
                            let label = if commands.is_empty() {
                                t!("import-run", count = n)
                            } else {
                                t!("import-run-commands", count = n, commands = commands.len())
                            };
                            if ui.add_enabled(n > 0 || keys || !commands.is_empty(), egui::Button::new(label)).clicked()
                            {
                                start = Some((Plan::clone(plan), commands.clone()));
                            }
                        }
                        if ui.button(t!("button-cancel")).clicked() {
                            outcome = Outcome::Cancel;
                        }
                    }
                    Step::Scanning { .. } => {
                        if ui.button(t!("button-cancel")).clicked() {
                            outcome = Outcome::Cancel;
                        }
                    }
                    Step::Running { .. } => {
                        ui.add(egui::Spinner::new());
                    }
                    Step::Finished { wrote, .. } => {
                        if ui.button(t!("button-close")).clicked() {
                            outcome = if *wrote { Outcome::Submit(()) } else { Outcome::Cancel };
                        }
                    }
                });
            });
        if let Some((plan, commands)) = start {
            self.start(plan, commands, ctx);
        }
        if !open && !running {
            outcome = match self.step {
                Step::Finished { wrote: true, .. } => Outcome::Submit(()),
                _ => Outcome::Cancel,
            };
        }
        outcome
    }
}
