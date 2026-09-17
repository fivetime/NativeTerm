//! "Import from SecureCRT / PuTTY": pick the folder (SecureCRT), preview
//! what would happen, then import in the background.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use native_term_app::import::{self, Line};
use native_term_app::t;
use native_term_config::ops::ImportOutcome;
use native_term_config::putty;
use native_term_config::securecrt::{self, Origin, Plan};
use native_term_config::SessionTree;

use crate::dialogs::Outcome;

enum Step {
    Choose,
    Preview { plan: Box<Plan>, lines: Vec<Line> },
    Running { rx: Receiver<Result<ImportOutcome, String>>, done: Arc<AtomicUsize>, total: usize },
    Finished { text: Vec<String>, wrote: bool },
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
    pub fn putty(ssh_dir: PathBuf, data_dir: PathBuf, tree: &SessionTree) -> ImportDialog {
        let path = format!(r"HKEY_CURRENT_USER\{}", putty::sessions_key());
        let mut dialog = ImportDialog { origin: Origin::Putty, path, step: Step::Choose, error: None, ssh_dir, data_dir };
        dialog.preview(tree);
        dialog
    }

    fn preview(&mut self, tree: &SessionTree) {
        let scanned = match self.origin {
            Origin::SecureCrt => securecrt::scan(&PathBuf::from(self.path.trim())),
            Origin::Putty => putty::scan(),
        };
        match scanned {
            Ok(scan) => {
                let plan = securecrt::plan(&scan, tree);
                let lines = import::summary(&scan, &plan);
                self.error = None;
                self.step = Step::Preview { plan: Box::new(plan), lines };
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn start(&mut self, plan: Plan, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let done = Arc::new(AtomicUsize::new(0));
        let total = plan.host_count();
        let editor = crate::app::editor_for(&self.ssh_dir, &self.data_dir);
        let progress = Arc::clone(&done);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let repaint = ctx.clone();
            let result = editor
                .import(&plan, &move |n, _| {
                    progress.store(n, Ordering::Relaxed);
                    repaint.request_repaint();
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.step = Step::Running { rx, done, total };
    }

    /// `Submit(())` once something was written (the tree needs a reload).
    pub fn show(&mut self, ctx: &egui::Context, tree: &SessionTree) -> Outcome<()> {
        if let Step::Running { rx, .. } = &self.step {
            if let Ok(result) = rx.try_recv() {
                self.step = match result {
                    Ok(o) => {
                        let mut text = vec![t!("import-done", hosts = o.hosts(), folders = o.written.len())];
                        if o.keys_added > 0 {
                            text.push(t!("import-keys-added", count = o.keys_added));
                        }
                        if let Some(e) = &o.keys_failed {
                            text.push(t!("import-keys-failed", error = e.as_str()));
                        }
                        text.extend(
                            o.failed.iter().map(|(label, why)| t!("import-folder-failed", folder = label.as_str(), error = why.as_str())),
                        );
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
                    Step::Preview { lines, .. } => {
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
                        ui.add(egui::ProgressBar::new(n as f32 / (*total).max(1) as f32).text(format!("{n} / {total}")));
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
                        if ui.add_enabled(!self.path.trim().is_empty(), egui::Button::new(t!("import-preview"))).clicked() {
                            self.preview(tree);
                        }
                        if let Step::Preview { plan, .. } = &self.step {
                            let n = plan.host_count();
                            let keys = !plan.host_keys.is_empty();
                            if ui.add_enabled(n > 0 || keys, egui::Button::new(t!("import-run", count = n))).clicked() {
                                start = Some(Plan::clone(plan));
                            }
                        }
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
        if let Some(plan) = start {
            self.start(plan, ctx);
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
