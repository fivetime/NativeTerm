//! "Install my key": which public key, on which hosts; each host gets a
//! tab where the shim runs ssh once (asking for the password there) and
//! adds the key unless it is there. Without a key, one can be created.

use std::path::{Path, PathBuf};

use native_term_app::{t, Core};

use crate::dialogs::Outcome;

/// Public keys in `ssh_dir`, the usual ones first.
pub fn public_keys(ssh_dir: &Path) -> Vec<PathBuf> {
    let mut keys: Vec<PathBuf> = ["id_ed25519.pub", "id_ecdsa.pub", "id_rsa.pub"]
        .iter()
        .map(|n| ssh_dir.join(n))
        .filter(|p| p.is_file())
        .collect();
    if let Ok(entries) = std::fs::read_dir(ssh_dir) {
        let mut others: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "pub") && !keys.contains(p))
            .collect();
        others.sort();
        keys.extend(others);
    }
    keys
}

pub struct KeyDialog {
    /// (alias, label)
    hosts: Vec<(String, String)>,
    keys: Vec<PathBuf>,
    chosen: usize,
    ssh_dir: PathBuf,
    pub error: Option<String>,
}

impl KeyDialog {
    pub fn new(hosts: Vec<(String, String)>, ssh_dir: &Path) -> KeyDialog {
        KeyDialog { hosts, keys: public_keys(ssh_dir), chosen: 0, ssh_dir: ssh_dir.to_path_buf(), error: None }
    }

    pub fn show(&mut self, ctx: &egui::Context, core: &Core) -> Outcome<()> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        egui::Window::new(t!("key-title"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if self.keys.is_empty() {
                    ui.label(t!("key-none", dir = self.ssh_dir.display().to_string()));
                    ui.horizontal(|ui| {
                        if ui.button(t!("key-create")).clicked() {
                            let path = self.ssh_dir.join("id_ed25519").display().to_string();
                            match core.terminal().open_tool(&t!("key-create-tab"), &["--create-key".into(), path]) {
                                Ok(()) => outcome = Outcome::Cancel,
                                Err(e) => self.error = Some(e.to_string()),
                            }
                        }
                        if ui.button(t!("key-refresh")).clicked() {
                            self.keys = public_keys(&self.ssh_dir);
                        }
                        if ui.button(t!("button-cancel")).clicked() {
                            outcome = Outcome::Cancel;
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.label(t!("key-which"));
                        let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        egui::ComboBox::from_id_salt("public-key").selected_text(name(&self.keys[self.chosen])).show_ui(ui, |ui| {
                            for (i, k) in self.keys.iter().enumerate() {
                                ui.selectable_value(&mut self.chosen, i, name(k));
                            }
                        });
                    });
                    let names: Vec<&str> = self.hosts.iter().map(|(_, l)| l.as_str()).take(8).collect();
                    let more = self.hosts.len().saturating_sub(names.len());
                    ui.label(t!("key-hosts", count = self.hosts.len(), names = names.join(", "), more = more));
                    ui.weak(t!("key-note"));
                    ui.horizontal(|ui| {
                        if ui.button(t!("key-install", count = self.hosts.len())).clicked() {
                            let key = self.keys[self.chosen].display().to_string();
                            let terminal_hosts = self.hosts.clone();
                            let core = core.clone();
                            // one tab per host, a little apart
                            std::thread::spawn(move || {
                                for (i, (alias, label)) in terminal_hosts.iter().enumerate() {
                                    if i > 0 {
                                        std::thread::sleep(std::time::Duration::from_millis(300));
                                    }
                                    let title = t!("key-tab", label = label.as_str());
                                    let _ = core.terminal().open_tool(&title, &["--install-key".into(), key.clone(), alias.clone()]);
                                }
                            });
                            outcome = Outcome::Cancel;
                        }
                        if ui.button(t!("button-cancel")).clicked() {
                            outcome = Outcome::Cancel;
                        }
                    });
                }
                if let Some(e) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x3a, 0x3a), e);
                }
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usual_keys_first() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["work.pub", "id_rsa.pub", "id_ed25519.pub", "id_ed25519", "config"] {
            std::fs::write(dir.path().join(name), "x").unwrap();
        }
        let names: Vec<String> =
            public_keys(dir.path()).iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["id_ed25519.pub", "id_rsa.pub", "work.pub"]);
    }
}
