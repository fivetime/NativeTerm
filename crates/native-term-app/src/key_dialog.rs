//! "Install my key": which public key, on which hosts; each host gets a
//! tab where the shim runs ssh once (asking for the password there) and
//! adds the key unless it is there. Hosts that share a password can go in
//! one tab instead, the password asked once (the shim's askpass helper).
//! Without a key, one can be created.

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

/// Aliases in groups whose joined length stays under `limit` (a batch
/// tab's command line), at least one alias each.
pub fn batches(aliases: &[String], limit: usize) -> Vec<Vec<String>> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut length = 0;
    for alias in aliases {
        match groups.last_mut() {
            Some(group) if length + alias.len() < limit => group.push(alias.clone()),
            _ => {
                groups.push(vec![alias.clone()]);
                length = 0;
            }
        }
        length += alias.len() + 1;
    }
    groups
}

/// Room for aliases on a batch tab's command line (Windows allows 32767
/// characters; wt.exe, the shim's path and the key take some).
const BATCH_LIMIT: usize = 24_000;

pub struct KeyDialog {
    /// (alias, label)
    hosts: Vec<(String, String)>,
    keys: Vec<PathBuf>,
    chosen: usize,
    ssh_dir: PathBuf,
    /// One tab and one password for all the hosts.
    one_password: bool,
    pub error: Option<String>,
}

impl KeyDialog {
    pub fn new(hosts: Vec<(String, String)>, ssh_dir: &Path) -> KeyDialog {
        KeyDialog {
            hosts,
            keys: public_keys(ssh_dir),
            chosen: 0,
            ssh_dir: ssh_dir.to_path_buf(),
            one_password: true,
            error: None,
        }
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
                        let name =
                            |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        egui::ComboBox::from_id_salt("public-key")
                            .selected_text(name(&self.keys[self.chosen]))
                            .show_ui(ui, |ui| {
                                for (i, k) in self.keys.iter().enumerate() {
                                    ui.selectable_value(&mut self.chosen, i, name(k));
                                }
                            });
                    });
                    let names: Vec<&str> = self.hosts.iter().map(|(_, l)| l.as_str()).take(8).collect();
                    let more = self.hosts.len().saturating_sub(names.len());
                    ui.label(t!("key-hosts", count = self.hosts.len(), names = names.join(", "), more = more));
                    let batch = self.hosts.len() > 1 && self.one_password;
                    if self.hosts.len() > 1 {
                        ui.checkbox(&mut self.one_password, t!("key-one-password"));
                    }
                    ui.weak(if batch { t!("key-one-password-hint") } else { t!("key-note") });
                    ui.horizontal(|ui| {
                        if ui.button(t!("key-install", count = self.hosts.len())).clicked() {
                            let key = self.keys[self.chosen].display().to_string();
                            let terminal_hosts = self.hosts.clone();
                            let core = core.clone();
                            // one tab per host (or per batch), a little apart
                            std::thread::spawn(move || {
                                let tabs: Vec<(String, Vec<String>)> = if batch {
                                    let aliases: Vec<String> = terminal_hosts.iter().map(|(a, _)| a.clone()).collect();
                                    let groups = batches(&aliases, BATCH_LIMIT);
                                    let mut tabs = Vec::new();
                                    for group in groups {
                                        let title = t!("key-batch-tab", count = group.len());
                                        let args =
                                            ["--install-key-batch".to_string(), key.clone()].into_iter().chain(group);
                                        tabs.push((title, args.collect()));
                                    }
                                    tabs
                                } else {
                                    let tab = |(alias, label): &(String, String)| {
                                        (
                                            t!("key-tab", label = label.as_str()),
                                            vec!["--install-key".into(), key.clone(), alias.clone()],
                                        )
                                    };
                                    terminal_hosts.iter().map(tab).collect()
                                };
                                for (i, (title, args)) in tabs.iter().enumerate() {
                                    if i > 0 {
                                        std::thread::sleep(std::time::Duration::from_millis(300));
                                    }
                                    let _ = core.terminal().open_tool(title, args);
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

    #[test]
    fn batches_fit_the_command_line() {
        let aliases: Vec<String> = ["web01", "web02", "db-primary", "x"].iter().map(|s| s.to_string()).collect();
        assert_eq!(batches(&aliases, 1000), vec![aliases.clone()]);
        let groups = batches(&aliases, 13);
        assert_eq!(groups, [vec!["web01", "web02"], vec!["db-primary", "x"]]);
        // an alias longer than the limit still gets its own group
        assert_eq!(batches(&["a-very-long-alias".to_string()], 4), [vec!["a-very-long-alias"]]);
        assert!(batches(&[], 10).is_empty());
    }
}
