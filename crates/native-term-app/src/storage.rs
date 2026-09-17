//! Checks on where sessions and data live: files a cloud client may keep
//! only online (OneDrive "Files On-Demand"), and copies left by sync
//! conflicts. A conflict copy in `config.d` is serious: `web-PC.conf`
//! still matches `*.conf`, so ssh reads it and its hosts appear twice.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use native_term_app::t;
use native_term_win::cloud::{cloud_state, CloudState};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Health {
    /// Only in the cloud now.
    pub cloud_only: Vec<PathBuf>,
    /// Synced, but may be freed up to the cloud later.
    pub not_kept: Vec<PathBuf>,
    /// Look like sync conflict copies.
    pub conflicts: Vec<PathBuf>,
}

impl Health {
    pub fn is_fine(&self) -> bool {
        self.cloud_only.is_empty() && self.not_kept.is_empty() && self.conflicts.is_empty()
    }
}

/// Whether a file name looks like a copy made by a sync conflict:
/// OneDrive (`name-PC.ext`, `name-PC-2.ext`), Dropbox ("conflicted
/// copy"), Syncthing (`.sync-conflict-`), rclone bisync (`.conflict1`),
/// and numbered copies (`name (1).ext`).
pub fn is_conflict_copy(name: &str, machine: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.contains("conflicted copy") || lower.contains("冲突") || lower.contains(".sync-conflict-") {
        return true;
    }
    if let Some(at) = lower.find(".conflict") {
        if lower[at + ".conflict".len()..].starts_with(|c: char| c.is_ascii_digit()) {
            return true;
        }
    }
    let stem = match lower.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => lower.as_str(),
    };
    // "name (1)"
    if let Some(open) = stem.rfind(" (") {
        let inner = &stem[open + 2..];
        if inner.strip_suffix(')').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
            return true;
        }
    }
    // "name-PC" or "name-PC-2"
    let machine = machine.to_lowercase();
    if machine.is_empty() {
        return false;
    }
    let without_number = match stem.rsplit_once('-') {
        Some((head, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => head,
        _ => stem,
    };
    without_number.strip_suffix(machine.as_str()).is_some_and(|head| head.len() > 1 && head.ends_with('-'))
}

fn files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> =
        entries.filter_map(Result::ok).filter(|e| e.file_type().is_ok_and(|t| t.is_file())).map(|e| e.path()).collect();
    files.sort();
    files
}

/// Look at the ssh folder, its `config.d`, and the data folder.
pub fn check(ssh_dir: &Path, data_dir: &Path) -> Health {
    let machine = std::env::var("COMPUTERNAME").unwrap_or_default();
    let mut health = Health::default();
    let mut files: Vec<PathBuf> = files_in(ssh_dir)
        .into_iter()
        .filter(|f| {
            let name = f.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            name.starts_with("config") || name.starts_with("known_hosts")
        })
        .collect();
    files.extend(files_in(&ssh_dir.join("config.d")));
    files.extend(files_in(data_dir));
    for file in files {
        let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if is_conflict_copy(&name, &machine) {
            health.conflicts.push(file.clone());
        }
        match cloud_state(&file) {
            Some(CloudState::CloudOnly) => health.cloud_only.push(file),
            Some(CloudState::Available) => health.not_kept.push(file),
            _ => {}
        }
    }
    health
}

/// The check, run in the background; shown as a warning line.
#[derive(Default)]
pub struct StorageCheck {
    result: Arc<Mutex<Option<Health>>>,
    dismissed: Option<Health>,
}

fn names(paths: &[PathBuf]) -> String {
    let shown: Vec<String> = paths.iter().take(4).map(|p| p.display().to_string()).collect();
    let more = paths.len().saturating_sub(shown.len());
    if more > 0 {
        format!("{} …(+{more})", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

impl StorageCheck {
    pub fn refresh(&self, ssh_dir: &Path, data_dir: &Path, ctx: &egui::Context) {
        let slot = Arc::clone(&self.result);
        let (ssh_dir, data_dir) = (ssh_dir.to_path_buf(), data_dir.to_path_buf());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let health = check(&ssh_dir, &data_dir);
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(health);
            ctx.request_repaint();
        });
    }

    pub fn banner(&mut self, ui: &mut egui::Ui) {
        let Some(health) = self.result.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
        if health.is_fine() || self.dismissed.as_ref() == Some(&health) {
            return;
        }
        let amber = egui::Color32::from_rgb(0xd0, 0x9a, 0x1a);
        let red = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
        let mut dismiss = false;
        ui.vertical(|ui| {
            if !health.conflicts.is_empty() {
                ui.colored_label(red, t!("storage-conflicts", count = health.conflicts.len(), files = names(&health.conflicts)));
            }
            if !health.cloud_only.is_empty() {
                ui.colored_label(red, t!("storage-cloud-only", count = health.cloud_only.len(), files = names(&health.cloud_only)));
            }
            if !health.not_kept.is_empty() {
                ui.colored_label(amber, t!("storage-not-kept", count = health.not_kept.len(), files = names(&health.not_kept)));
            }
            ui.horizontal(|ui| {
                let folder = health.conflicts.iter().chain(&health.cloud_only).chain(&health.not_kept).next().and_then(|f| f.parent());
                if let Some(folder) = folder {
                    if ui.small_button(t!("wizard-open-folder")).clicked() {
                        let _ = std::process::Command::new("explorer.exe").arg(folder).spawn();
                    }
                }
                dismiss = ui.small_button(t!("agent-hint-dismiss")).clicked();
            });
        });
        if dismiss {
            // until something changes
            self.dismissed = Some(health);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_names() {
        let pc = "DESKTOP-AB12";
        for name in [
            "web-DESKTOP-AB12.conf",
            "web-desktop-ab12-2.conf",
            "Global-DESKTOP-AB12-3.ini",
            "commands (1).toml",
            "web (Simon's conflicted copy 2026-09-18).conf",
            "web.sync-conflict-20260918-120000-ABCDEFG.conf",
            "web.conf.conflict1",
            "web.conflict2.conf",
            "生产 (冲突副本).conf",
        ] {
            assert!(is_conflict_copy(name, pc), "{name}");
        }
        for name in ["web.conf", "desktop-ab12.conf", "web-2.conf", "node (prod).conf", "conflicts.toml", "state.db", "config"] {
            assert!(!is_conflict_copy(name, pc), "{name}");
        }
        assert!(!is_conflict_copy("web-.conf", ""));
    }

    #[test]
    fn checks_the_folders() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path().join(".ssh");
        std::fs::create_dir_all(ssh.join("config.d")).unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let machine = std::env::var("COMPUTERNAME").unwrap_or_default();
        for f in [ssh.join("config"), ssh.join("id_ed25519 (1)"), ssh.join("config.d").join("web.conf"), data.join("state.db")] {
            std::fs::write(f, "x").unwrap();
        }
        std::fs::write(ssh.join("config.d").join(format!("web-{machine}.conf")), "x").unwrap();
        std::fs::write(data.join("commands (1).toml"), "x").unwrap();
        let health = check(&ssh, &data);
        let names: Vec<String> =
            health.conflicts.iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        if machine.is_empty() {
            assert_eq!(names, ["commands (1).toml"]);
        } else {
            assert_eq!(names, [format!("web-{machine}.conf"), "commands (1).toml".to_string()]);
        }
        assert!(health.cloud_only.is_empty() && health.not_kept.is_empty(), "temp files are local");
    }
}
