//! The "NativeTerm SSH" profile in the chosen Windows Terminal: status,
//! install and removal (a fragment, see `profile.rs` in the platform crate).
//! Installing is the user's action; only a fragment NativeTerm installed
//! itself is rewritten on its own, when the program folder moved.

use std::path::PathBuf;

use native_term_platform::windows_terminal::install::Install;
use native_term_platform::windows_terminal::profile::{self, Status};

pub struct ProfileSetup {
    install: Install,
    shim: PathBuf,
    root: Option<PathBuf>,
    pub status: Status,
}

impl ProfileSetup {
    pub fn new(install: Install, shim: PathBuf) -> ProfileSetup {
        let root = profile::fragments_root();
        let status = profile::status(&install, root.as_deref(), &shim);
        ProfileSetup { install, shim, root, status }
    }

    pub fn refresh(&mut self) {
        self.status = profile::status(&self.install, self.root.as_deref(), &self.shim);
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
            Ok(_) => format!("Updated the \"NativeTerm SSH\" profile: {} moved to {}", old.display(), self.shim.display()),
            Err(e) => format!("Could not update the \"NativeTerm SSH\" profile: {e}"),
        })
    }

    fn install_fragment(&mut self) -> Result<(), String> {
        let root = self.root.clone().ok_or("LOCALAPPDATA is not set")?;
        profile::install(&root, &self.shim, &self.settings_files()).map_err(|e| e.to_string())?;
        self.refresh();
        Ok(())
    }

    fn remove_fragment(&mut self) -> Result<(), String> {
        let root = self.root.clone().ok_or("LOCALAPPDATA is not set")?;
        profile::uninstall(&root, &self.settings_files()).map_err(|e| e.to_string())?;
        self.refresh();
        Ok(())
    }

    pub fn describe(&self) -> String {
        match &self.status {
            Status::Installed => "installed (fragment)".into(),
            Status::InSettings => "defined in this Terminal's settings.json".into(),
            Status::Outdated { shim } => format!("points at {}", shim.display()),
            Status::Disabled => "turned off on Terminal's Extensions page".into(),
            Status::Missing => "not installed".into(),
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
                ui.colored_label(
                    red,
                    "The \"NativeTerm SSH\" profile is turned off in Windows Terminal (Settings → Extensions → NativeTerm).",
                );
                if ui.button("Check again").clicked() {
                    self.refresh();
                }
            }
            _ => {
                ui.colored_label(red, "Windows Terminal doesn't have the \"NativeTerm SSH\" profile yet; tabs can't open.");
                if ui
                    .button("Install profile")
                    .on_hover_text(format!(
                        "Writes {} (read by every Windows Terminal of this user)",
                        self.root.as_ref().map(|r| profile::fragment_path(r).display().to_string()).unwrap_or_default()
                    ))
                    .clicked()
                {
                    if let Err(e) = self.install_fragment() {
                        notices.push(format!("Installing the profile failed: {e}"));
                    }
                }
            }
        });
    }

    pub fn settings_ui(&mut self, ui: &mut egui::Ui, notices: &mut Vec<String>) {
        ui.label(format!("Windows Terminal: {} ({:?})", self.install.dir.display(), self.install.kind));
        ui.label(format!("\"NativeTerm SSH\" profile: {}", self.describe()));
        ui.horizontal(|ui| {
            let installed = matches!(self.status, Status::Installed | Status::Outdated { .. } | Status::Disabled);
            if ui.add_enabled(!matches!(self.status, Status::Installed), egui::Button::new("Install / update")).clicked() {
                if let Err(e) = self.install_fragment() {
                    notices.push(format!("Installing the profile failed: {e}"));
                }
            }
            if ui.add_enabled(installed, egui::Button::new("Remove")).clicked() {
                if let Err(e) = self.remove_fragment() {
                    notices.push(format!("Removing the profile failed: {e}"));
                }
            }
            if ui.button("Check again").clicked() {
                self.refresh();
            }
        });
    }
}
