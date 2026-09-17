//! The "NativeTerm SSH" profile in the chosen Windows Terminal: status,
//! install and removal (a fragment, see `profile.rs` in the platform crate).
//! Installing is the user's action; only a fragment NativeTerm installed
//! itself is rewritten on its own, when the program folder moved.

use std::path::PathBuf;

use native_term_app::t;
use native_term_platform::windows_terminal::install::{Install, Kind};
use native_term_platform::windows_terminal::profile::{self, Status};
use native_term_platform::windows_terminal::sources;

pub struct ProfileSetup {
    install: Install,
    shim: PathBuf,
    root: Option<PathBuf>,
    pub status: Status,
    /// Where `settings.json` is copied before NativeTerm changes it.
    backups: PathBuf,
    /// Terminal's own SSH profiles are turned off (`None`: unreadable).
    ssh_hidden: Option<bool>,
}

impl ProfileSetup {
    pub fn new(install: Install, shim: PathBuf, backups: PathBuf) -> ProfileSetup {
        let root = profile::fragments_root();
        let status = profile::status(&install, root.as_deref(), &shim);
        let mut setup = ProfileSetup { install, shim, root, status, backups, ssh_hidden: None };
        setup.refresh();
        setup
    }

    pub fn refresh(&mut self) {
        self.status = profile::status(&self.install, self.root.as_deref(), &self.shim);
        self.ssh_hidden = std::fs::read_to_string(self.install.settings_json())
            .ok()
            .map(|text| sources::is_disabled(&text, sources::SSH_SOURCE));
    }

    /// Hide or show Terminal's own SSH profiles (a backed-up edit of its
    /// `settings.json`).
    fn set_ssh_hidden(&mut self, hide: bool) -> Result<Option<PathBuf>, String> {
        let result = sources::set_disabled(&self.install.settings_json(), sources::SSH_SOURCE, hide, &self.backups);
        self.refresh();
        result.map_err(|e| e.to_string())
    }

    /// Every Terminal that reads the fragment reloads when its
    /// `settings.json` changes, so touch all known ones. A test fragment
    /// folder (`NATIVETERM_FRAGMENTS_DIR`) only concerns the chosen one.
    fn settings_files(&self) -> Vec<PathBuf> {
        let mut files = vec![self.install.settings_json()];
        if std::env::var_os(profile::FRAGMENTS_ENV).is_some() {
            return files;
        }
        for other in Install::discover(&[]) {
            let file = other.settings_json();
            if !files.contains(&file) && file.exists() {
                files.push(file);
            }
        }
        files
    }

    /// Rewrite our own fragment after the program folder moved.
    pub fn fix_moved(&mut self) -> Option<String> {
        let Status::Outdated { shim: old } = &self.status else { return None };
        let old = old.clone();
        let root = self.root.clone()?;
        let result = profile::install(&root, &self.shim, &self.settings_files());
        self.refresh();
        Some(match result {
            Ok(_) => t!("profile-updated", old = old.display().to_string(), new = self.shim.display().to_string()),
            Err(e) => t!("profile-update-failed", error = e.to_string()),
        })
    }

    fn install_fragment(&mut self) -> Result<(), String> {
        let root = self.root.clone().ok_or_else(|| t!("profile-no-localappdata"))?;
        profile::install(&root, &self.shim, &self.settings_files()).map_err(|e| e.to_string())?;
        self.refresh();
        Ok(())
    }

    fn remove_fragment(&mut self) -> Result<(), String> {
        let root = self.root.clone().ok_or_else(|| t!("profile-no-localappdata"))?;
        profile::uninstall(&root, &self.settings_files()).map_err(|e| e.to_string())?;
        self.refresh();
        Ok(())
    }

    pub fn describe(&self) -> String {
        match &self.status {
            Status::Installed => t!("profile-installed"),
            Status::InSettings => t!("profile-in-settings"),
            Status::Outdated { shim } => t!("profile-outdated", path = shim.display().to_string()),
            Status::Disabled => t!("profile-disabled"),
            Status::Missing => t!("profile-missing"),
        }
    }

    /// A warning line when tabs can't open; empty otherwise.
    pub fn banner(&mut self, ui: &mut egui::Ui, notices: &mut Vec<String>) {
        if self.status.usable() {
            return;
        }
        let red = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
        ui.horizontal_wrapped(|ui| match &self.status {
            Status::Disabled => {
                ui.colored_label(red, t!("profile-banner-disabled"));
                if ui.button(t!("button-check-again")).clicked() {
                    self.refresh();
                }
            }
            _ => {
                ui.colored_label(red, t!("profile-banner-missing"));
                let path = self.root.as_ref().map(|r| profile::fragment_path(r).display().to_string()).unwrap_or_default();
                if ui
                    .button(t!("profile-install"))
                    .on_hover_text(t!("profile-install-hint", path = path))
                    .clicked()
                {
                    if let Err(e) = self.install_fragment() {
                        notices.push(t!("profile-install-failed", error = e));
                    }
                }
            }
        });
    }

    pub fn settings_ui(&mut self, ui: &mut egui::Ui, notices: &mut Vec<String>) {
        ui.label(t!("settings-terminal", dir = self.install.dir.display().to_string(), kind = kind_name(&self.install.kind)));
        ui.label(t!("settings-profile", status = self.describe()));
        ui.horizontal(|ui| {
            let installed = matches!(self.status, Status::Installed | Status::Outdated { .. } | Status::Disabled);
            if ui.add_enabled(!matches!(self.status, Status::Installed), egui::Button::new(t!("profile-install-update"))).clicked() {
                if let Err(e) = self.install_fragment() {
                    notices.push(t!("profile-install-failed", error = e));
                }
            }
            if ui.add_enabled(installed, egui::Button::new(t!("profile-remove"))).clicked() {
                if let Err(e) = self.remove_fragment() {
                    notices.push(t!("profile-remove-failed", error = e));
                }
            }
            if ui.button(t!("button-check-again")).clicked() {
                self.refresh();
            }
        });
        if let Some(hidden) = self.ssh_hidden {
            let mut hide = hidden;
            let response = ui.checkbox(&mut hide, t!("settings-hide-terminal-ssh"));
            if response.on_hover_text(t!("settings-hide-terminal-ssh-hint")).changed() {
                match self.set_ssh_hidden(hide) {
                    Ok(Some(backup)) => notices.push(t!("settings-terminal-changed", backup = backup.display().to_string())),
                    Ok(None) => {}
                    Err(e) => notices.push(t!("settings-terminal-change-failed", error = e)),
                }
            }
        }
    }
}

fn kind_name(kind: &Kind) -> String {
    match kind {
        Kind::Packaged => t!("terminal-kind-packaged"),
        Kind::Portable => t!("terminal-kind-portable"),
        Kind::Unpackaged => t!("terminal-kind-unpackaged"),
    }
}
