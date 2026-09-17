//! "Import from SecureCRT": pick the folder, preview what would happen,
//! then import in the background.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use native_term_app::import::{self, Line};
use native_term_config::ops::ImportOutcome;
use native_term_config::securecrt::{self, Plan};
use native_term_config::SessionTree;

use crate::dialogs::Outcome;

enum Step {
    Choose,
    Preview { plan: Box<Plan>, lines: Vec<Line> },
    Running { rx: Receiver<Result<ImportOutcome, String>>, done: Arc<AtomicUsize>, total: usize },
    Finished { text: Vec<String>, wrote: bool },
}

pub struct ImportDialog {
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
        ImportDialog { path, step: Step::Choose, error: None, ssh_dir, data_dir }
    }

    fn preview(&mut self, tree: &SessionTree) {
        let config = PathBuf::from(self.path.trim());
        match securecrt::scan(&config) {
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
                        let mut text = vec![format!("Imported {} hosts into {} folders.", o.hosts(), o.written.len())];
                        text.extend(o.failed.iter().map(|(label, why)| format!("Folder {label} was not written: {why}")));
                        Step::Finished { text, wrote: o.hosts() > 0 }
                    }
                    Err(e) => Step::Finished { text: vec![format!("Import failed: {e}")], wrote: false },
                };
            }
        }
        let mut outcome = Outcome::Open;
        let mut open = true;
        let running = matches!(self.step, Step::Running { .. });
        let mut start = None;
        egui::Window::new("Import from SecureCRT")
            .collapsible(false)
            .resizable(true)
            .default_width(640.0)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("SecureCRT config folder");
                    ui.add_enabled(
                        matches!(self.step, Step::Choose | Step::Preview { .. }),
                        egui::TextEdit::singleline(&mut self.path).hint_text("…\\VanDyke\\Config").desired_width(360.0),
                    );
                });
                ui.weak(format!("Into: {}  (every changed file is backed up first)", self.ssh_dir.display()));
                if let Some(e) = &self.error {
                    ui.colored_label(RED, e);
                }
                ui.separator();
                match &mut self.step {
                    Step::Choose => {
                        if self.path.is_empty() {
                            ui.label("SecureCRT's config folder wasn't found; enter it above.");
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
                        ui.weak("Each folder is checked with ssh -G after writing.");
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
                        if ui.add_enabled(!self.path.trim().is_empty(), egui::Button::new("Preview")).clicked() {
                            self.preview(tree);
                        }
                        if let Step::Preview { plan, .. } = &self.step {
                            let n = plan.host_count();
                            if ui.add_enabled(n > 0, egui::Button::new(format!("Import {n} hosts"))).clicked() {
                                start = Some(Plan::clone(plan));
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            outcome = Outcome::Cancel;
                        }
                    }
                    Step::Running { .. } => {
                        ui.add(egui::Spinner::new());
                    }
                    Step::Finished { wrote, .. } => {
                        if ui.button("Close").clicked() {
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
