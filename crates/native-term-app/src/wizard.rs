//! The first-run wizard: environment checks, import, keys, and where
//! things are kept. Every step can be skipped; each button only opens the
//! dialog or tab that does the work, so nothing happens behind the
//! user's back. Shown once (a `state.db` setting), and again from Settings.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use native_term_app::t;
use native_term_win::service::ServiceState;

use crate::agent::Status as AgentStatus;

/// `state.db` setting: the wizard was finished or skipped.
pub const DONE_SETTING: &str = "first_run_done";

const STEPS: usize = 4;

/// What the user asked for; carried out by the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WizardAction {
    InstallProfile,
    OpenSettings,
    ImportSecureCrt,
    ImportPutty,
    CreateKey,
    InstallKeys,
    OpenFolder(PathBuf),
    /// Copy NativeTerm's data to this folder, used from the next start.
    MoveData(PathBuf),
    /// Move the session folders here (a synced folder).
    MoveFolders(PathBuf),
    /// Use the session folders already here (synced from another computer).
    AdoptFolders(PathBuf),
    /// Finished or skipped: don't show it again.
    Done,
}

/// What the wizard shows, gathered by the app each frame.
pub struct Facts<'a> {
    pub terminal: String,
    pub profile: String,
    pub profile_usable: bool,
    pub agent: Option<AgentStatus>,
    pub securecrt: Option<PathBuf>,
    pub putty: bool,
    pub hosts: usize,
    pub public_keys: usize,
    pub ssh_dir: &'a Path,
    pub data_dir: &'a Path,
    /// The data folder can be changed from here (not set by `--data-dir`
    /// or the environment).
    pub data_movable: bool,
    /// Tabs can be opened (the core runs).
    pub can_open_tabs: bool,
    /// Where the session folders are now.
    pub folders_dir: &'a Path,
    /// Folders a sync client keeps in step (name, path).
    pub sync_roots: &'a [(String, PathBuf)],
}

/// Where the session folders go in a synced folder.
pub fn synced_folders(root: &Path) -> PathBuf {
    root.join("NativeTerm").join("ssh-folders")
}

pub struct Wizard {
    step: usize,
    /// Step 4: the new data folder being typed.
    new_data_dir: String,
    /// `ssh -V`, asked in the background.
    ssh: Arc<Mutex<Option<Result<String, String>>>>,
}

/// `ssh -V` (it prints to stderr).
fn ssh_version(ssh: &Path) -> Result<String, String> {
    let mut command = Command::new(ssh);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // no console window
    }
    let output = command.arg("-V").stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let text = if text.is_empty() { String::from_utf8_lossy(&output.stdout).trim().to_string() } else { text };
    if output.status.success() && !text.is_empty() {
        Ok(text)
    } else {
        Err(text)
    }
}

const GREEN: egui::Color32 = egui::Color32::from_rgb(0x2e, 0xa0, 0x43);
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x9a, 0x1a);

fn check_line(ui: &mut egui::Ui, ok: bool, text: String) {
    ui.horizontal_wrapped(|ui| {
        if ok {
            ui.colored_label(GREEN, "✔");
        } else {
            ui.colored_label(AMBER, "⚠");
        }
        ui.label(text);
    });
}

impl Wizard {
    pub fn new(ctx: &egui::Context) -> Wizard {
        let ssh = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&ssh);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let program = native_term_session::ssh_program();
            let result = ssh_version(&program).map(|v| format!("{v}  ({})", program.display()));
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            ctx.request_repaint();
        });
        Wizard { step: 0, new_data_dir: String::new(), ssh }
    }

    fn step_title(&self) -> String {
        match self.step {
            0 => t!("wizard-check"),
            1 => t!("wizard-import"),
            2 => t!("wizard-keys"),
            _ => t!("wizard-data"),
        }
    }

    fn check(&self, ui: &mut egui::Ui, facts: &Facts, actions: &mut Vec<WizardAction>) {
        ui.label(t!("wizard-check-intro"));
        match self.ssh.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            None => {
                ui.weak(t!("wizard-ssh-checking"));
            }
            Some(Ok(version)) => check_line(ui, true, t!("wizard-ssh-ok", version = version)),
            Some(Err(e)) => check_line(ui, false, t!("wizard-ssh-missing", error = e)),
        }
        check_line(ui, true, t!("wizard-terminal", terminal = facts.terminal.as_str()));
        ui.horizontal_wrapped(|ui| {
            check_line(ui, facts.profile_usable, t!("wizard-profile", status = facts.profile.as_str()));
            if !facts.profile_usable && ui.button(t!("profile-install")).clicked() {
                actions.push(WizardAction::InstallProfile);
            }
        });
        match &facts.agent {
            None => {
                ui.weak(t!("agent-checking"));
            }
            Some(status) => {
                let running = status.service == ServiceState::Running || status.other_agent;
                let text = if running {
                    t!("wizard-agent-running")
                } else if status.needs_agent() {
                    t!("wizard-agent-needed")
                } else {
                    t!("wizard-agent-off")
                };
                ui.horizontal_wrapped(|ui| {
                    check_line(ui, running || !status.needs_agent(), text);
                    if status.needs_agent() && ui.small_button(t!("agent-hint-show")).clicked() {
                        actions.push(WizardAction::OpenSettings);
                    }
                });
            }
        }
    }

    fn import(&self, ui: &mut egui::Ui, facts: &Facts, actions: &mut Vec<WizardAction>) {
        ui.label(t!("wizard-import-intro", hosts = facts.hosts));
        ui.horizontal_wrapped(|ui| {
            if ui.button(t!("import-securecrt-button")).clicked() {
                actions.push(WizardAction::ImportSecureCrt);
            }
            match &facts.securecrt {
                Some(path) => ui.weak(t!("wizard-securecrt-found", path = path.display().to_string())),
                None => ui.weak(t!("wizard-securecrt-not-found")),
            };
        });
        ui.horizontal_wrapped(|ui| {
            if facts.putty {
                if ui.button(t!("import-putty-button")).clicked() {
                    actions.push(WizardAction::ImportPutty);
                }
            } else {
                ui.weak(t!("wizard-putty-none"));
            }
        });
        ui.weak(t!("wizard-import-later"));
    }

    fn keys(&self, ui: &mut egui::Ui, facts: &Facts, actions: &mut Vec<WizardAction>) {
        ui.label(t!("wizard-keys-intro"));
        if facts.public_keys == 0 {
            ui.label(t!("key-none", dir = facts.ssh_dir.display().to_string()));
            if ui.add_enabled(facts.can_open_tabs, egui::Button::new(t!("key-create"))).clicked() {
                actions.push(WizardAction::CreateKey);
            }
        } else {
            check_line(ui, true, t!("wizard-keys-found", count = facts.public_keys));
            let enabled = facts.can_open_tabs && facts.hosts > 0;
            if ui.add_enabled(enabled, egui::Button::new(t!("wizard-install-keys", count = facts.hosts))).clicked() {
                actions.push(WizardAction::InstallKeys);
            }
            ui.weak(t!("key-note"));
        }
    }

    fn data(&mut self, ui: &mut egui::Ui, facts: &Facts, actions: &mut Vec<WizardAction>) {
        ui.label(t!("wizard-data-intro"));
        for (what, path) in [(t!("wizard-sessions-dir"), facts.ssh_dir), (t!("wizard-data-dir"), facts.data_dir)] {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{what}: {}", path.display()));
                if ui.small_button(t!("wizard-open-folder")).clicked() {
                    actions.push(WizardAction::OpenFolder(path.to_path_buf()));
                }
            });
        }
        if facts.data_movable {
            ui.horizontal(|ui| {
                ui.label(t!("wizard-data-move"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_data_dir)
                        .hint_text(r"D:\Sync\NativeTerm")
                        .desired_width(260.0),
                );
                let ready = !self.new_data_dir.trim().is_empty();
                if ui.add_enabled(ready, egui::Button::new(t!("data-dir-move"))).clicked() {
                    actions.push(WizardAction::MoveData(PathBuf::from(self.new_data_dir.trim())));
                }
            });
        } else {
            ui.weak(t!("data-dir-fixed"));
        }
        ui.add_space(8.0);
        ui.strong(t!("wizard-sync-title"));
        let inside = facts.sync_roots.iter().find(|(_, root)| facts.folders_dir.starts_with(root));
        match inside {
            Some((name, _)) => {
                let path = facts.folders_dir.display().to_string();
                check_line(ui, true, t!("wizard-sync-inside", name = name.as_str(), path = path));
            }
            None if facts.sync_roots.is_empty() => {
                ui.label(t!("wizard-sync-none"));
            }
            None => {
                ui.label(t!("wizard-sync-intro"));
                for (name, root) in facts.sync_roots {
                    let target = synced_folders(root);
                    ui.horizontal_wrapped(|ui| {
                        if native_term_config::ops::holds_folders(&target) {
                            if ui.button(t!("wizard-sync-adopt", name = name.as_str())).clicked() {
                                actions.push(WizardAction::AdoptFolders(target.clone()));
                            }
                        } else if ui.button(t!("wizard-sync-move", name = name.as_str())).clicked() {
                            actions.push(WizardAction::MoveFolders(target.clone()));
                        }
                        ui.weak(target.display().to_string());
                    });
                }
            }
        }
        ui.weak(t!("wizard-sync-note"));
    }

    /// Draws the wizard; returns what the user chose.
    pub fn show(&mut self, ctx: &egui::Context, facts: &Facts) -> Vec<WizardAction> {
        let mut actions = Vec::new();
        let mut open = true;
        let title = t!("wizard-title");
        egui::Window::new(title)
            .id(egui::Id::new("first-run"))
            .collapsible(false)
            .resizable(false)
            .default_width(560.0)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(520.0);
                let n = self.step + 1;
                ui.strong(t!("wizard-step", n = n, total = STEPS, title = self.step_title()));
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_min_height(170.0);
                    match self.step {
                        0 => self.check(ui, facts, &mut actions),
                        1 => self.import(ui, facts, &mut actions),
                        2 => self.keys(ui, facts, &mut actions),
                        _ => self.data(ui, facts, &mut actions),
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.step > 0, egui::Button::new(t!("wizard-back"))).clicked() {
                        self.step -= 1;
                    }
                    if self.step + 1 < STEPS {
                        if ui.button(t!("wizard-next")).clicked() {
                            self.step += 1;
                        }
                        if ui.button(t!("wizard-skip")).clicked() {
                            actions.push(WizardAction::Done);
                        }
                    } else if ui.button(t!("wizard-finish")).clicked() {
                        actions.push(WizardAction::Done);
                    }
                });
            });
        if !open {
            actions.push(WizardAction::Done);
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_ssh_for_its_version() {
        let ssh = Path::new(r"C:\Windows\System32\OpenSSH\ssh.exe");
        if ssh.exists() {
            assert!(ssh_version(ssh).unwrap().starts_with("OpenSSH"));
        }
        assert!(ssh_version(Path::new(r"C:\no\such\ssh.exe")).is_err());
    }
}
