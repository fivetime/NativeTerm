//! "Clean up": what to do before deleting NativeTerm, and what it leaves
//! behind.
//!
//! Two kinds of thing. What NativeTerm should take back itself, because
//! nothing else will: the Windows Terminal profile it installed, which
//! would otherwise be a profile pointing at a program that isn't there.
//! And what stays: the lines in `~/.ssh` (see `traces.rs`), the data
//! directory, saved passwords in Credential Manager, and the registry
//! value that says where the data is. Those are listed with their paths
//! and removed only when the person says so — the ssh lines are what
//! makes their sessions work, and the passwords are theirs.

use std::path::PathBuf;

use crate::terminal_profile::Status;
use native_term_app::t;
use native_term_config::password;
use native_term_config::traces::Traces;
use native_term_os::credentials;

use crate::dialogs::Outcome;

/// What NativeTerm put outside its own folder. The Windows Terminal
/// profile is not in here: the app knows its state at every moment and
/// hands it to `show`.
pub struct Cleanup {
    pub traces: Traces,
    pub data_dir: PathBuf,
    /// Saved passwords and credential sets (`NativeTerm…` in Credential
    /// Manager).
    pub credentials: Vec<String>,
    /// `HKCU\Software\NativeTerm\DataDir`, when it points somewhere.
    pub registry: Option<String>,
    /// What was done, for the person to read.
    pub done: Vec<String>,
    /// A removal that waits for a second click.
    confirming: Option<Step>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Credentials,
    Registry,
}

/// The saved passwords and sets NativeTerm has stored.
fn saved_entries() -> Vec<String> {
    let mut entries = credentials::list(&password::prefix()).unwrap_or_default();
    entries.sort();
    entries.dedup();
    entries
}

/// The data-folder pointer in the registry, where there is one.
fn registry_value() -> Option<String> {
    #[cfg(windows)]
    {
        native_term_os::desktop::user_registry_string(
            native_term_app::data_dir::REGISTRY_KEY,
            native_term_app::data_dir::REGISTRY_VALUE,
        )
        .ok()
        .flatten()
        .filter(|value| !value.is_empty())
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Remove that pointer.
fn delete_registry_value() -> std::io::Result<()> {
    #[cfg(windows)]
    {
        native_term_os::registry::delete_user_value(
            native_term_app::data_dir::REGISTRY_KEY,
            native_term_app::data_dir::REGISTRY_VALUE,
        )
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

impl Cleanup {
    pub fn new(
        ssh_dir: &std::path::Path,
        folders: &std::path::Path,
        shim: &std::path::Path,
        data_dir: PathBuf,
    ) -> Cleanup {
        Cleanup {
            traces: native_term_config::traces::find(ssh_dir, folders, shim),
            data_dir,
            credentials: saved_entries(),
            registry: registry_value(),
            done: Vec::new(),
            confirming: None,
        }
    }

    /// Everything as text, to keep or paste somewhere.
    pub fn as_text(&self) -> String {
        let mut lines = vec![t!("cleanup-title")];
        lines.push(t!("cleanup-data", path = self.data_dir.display().to_string()));
        if let Some(value) = &self.registry {
            lines.push(t!("cleanup-registry", value = value.as_str()));
        }
        if !self.credentials.is_empty() {
            lines.push(t!("cleanup-credentials", count = self.credentials.len()));
            lines.extend(self.credentials.iter().map(|e| format!("  {e}")));
        }
        if !self.traces.is_empty() {
            lines.push(t!("cleanup-ssh", count = self.traces.lines().len()));
            lines.extend(self.traces.lines().iter().map(|line| format!("  {line}")));
        }
        lines.join("\n")
    }
}

/// What the dialog asks the app to do (it owns the profile).
pub enum CleanupAction {
    RemoveFragment,
}

pub struct CleanupDialog {
    pub cleanup: Cleanup,
    /// Set by the app after it removed the profile.
    pub message: Option<String>,
}

impl CleanupDialog {
    pub fn new(cleanup: Cleanup) -> CleanupDialog {
        CleanupDialog { cleanup, message: None }
    }

    pub fn show(&mut self, ctx: &egui::Context, status: &Status, actions: &mut Vec<CleanupAction>) -> Outcome<()> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        egui::Window::new(t!("cleanup-title"))
            .collapsible(false)
            .resizable(true)
            .min_width(680.0)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.weak(t!("cleanup-intro"));
                ui.separator();

                // 1. the Windows Terminal profile: ours to take back
                ui.strong(t!("cleanup-fragment-title"));
                let fragment = status.is_fragment();
                ui.horizontal_wrapped(|ui| {
                    ui.label(match fragment {
                        true => t!("cleanup-fragment-there"),
                        false => t!("cleanup-fragment-gone"),
                    });
                    if ui.add_enabled(fragment, egui::Button::new(t!("cleanup-remove-fragment"))).clicked() {
                        actions.push(CleanupAction::RemoveFragment);
                    }
                });
                if matches!(status, Status::InSettings) {
                    ui.label(t!("cleanup-in-settings"));
                }
                ui.add_space(6.0);

                // 2. what stays behind
                ui.strong(t!("cleanup-left-title"));
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(t!("cleanup-data", path = self.cleanup.data_dir.display().to_string()));
                        if ui.small_button(t!("wizard-open-folder")).clicked() {
                            let _ = native_term_os::shell::open_file(&self.cleanup.data_dir);
                        }
                    });
                    if let Some(value) = self.cleanup.registry.clone() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(t!("cleanup-registry", value = value.as_str()));
                            self.remover(ui, Step::Registry, t!("cleanup-remove-registry"));
                        });
                    }
                    if !self.cleanup.credentials.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(t!("cleanup-credentials", count = self.cleanup.credentials.len()));
                            self.remover(ui, Step::Credentials, t!("cleanup-remove-credentials"));
                        });
                    }
                    if self.cleanup.traces.is_empty() {
                        ui.label(t!("cleanup-ssh-none"));
                    } else {
                        ui.label(t!("cleanup-ssh", count = self.cleanup.traces.lines().len()));
                        ui.weak(t!("cleanup-ssh-hint"));
                        for line in self.cleanup.traces.lines() {
                            ui.monospace(line);
                        }
                        ui.weak(t!("cleanup-files", count = self.cleanup.traces.files.len()));
                    }
                });

                if let Some(message) = &self.message {
                    ui.separator();
                    ui.label(message.as_str());
                }
                for line in &self.cleanup.done {
                    ui.label(line.as_str());
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(t!("cleanup-copy")).clicked() {
                        ui.ctx().copy_text(self.cleanup.as_text());
                    }
                    if ui.button(t!("button-close")).clicked() {
                        outcome = Outcome::Cancel;
                    }
                });
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }

    /// A removal that takes two clicks: the second one says "really".
    fn remover(&mut self, ui: &mut egui::Ui, step: Step, label: String) {
        let asking = self.cleanup.confirming == Some(step);
        let text = if asking { t!("cleanup-really") } else { label };
        if ui.small_button(text).clicked() {
            if !asking {
                self.cleanup.confirming = Some(step);
                return;
            }
            self.cleanup.confirming = None;
            match step {
                Step::Credentials => {
                    let mut gone = 0;
                    let mut failed = Vec::new();
                    for entry in std::mem::take(&mut self.cleanup.credentials) {
                        match credentials::delete(&entry) {
                            Ok(true) => gone += 1,
                            Ok(false) => {}
                            Err(e) => failed.push(format!("{entry}: {e}")),
                        }
                    }
                    self.cleanup.done.push(t!("cleanup-credentials-gone", count = gone));
                    self.cleanup.done.extend(failed);
                    self.cleanup.credentials = saved_entries();
                }
                Step::Registry => match delete_registry_value() {
                    Ok(()) => {
                        self.cleanup.registry = None;
                        self.cleanup.done.push(t!("cleanup-registry-gone"));
                    }
                    Err(e) => self.cleanup.done.push(e.to_string()),
                },
            }
        }
    }
}
