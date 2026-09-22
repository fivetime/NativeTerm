//! The "NativeTerm SSH" profile, where there is no terminal to install
//! one into: the same names as `terminal_profile.rs`, saying so. The
//! settings that name a terminal install and the favorites keep their
//! keys, so a data folder moves between systems without losing them.

// a stand-in carries the whole API, used or not
#![allow(dead_code)]

use std::path::PathBuf;

use native_term_app::{t, Core};

/// `state.db` setting: the folder of the terminal install to drive.
pub const INSTALL_SETTING: &str = "terminal.install";
/// Setting: whether favorite hosts get a place in the terminal's own
/// menus (nothing here has such a place).
pub const FAVORITES_SETTING: &str = "terminal.favorites";

/// Whether the profile is there (the same states as on Windows, so the
/// dialogs read the same); here it is always `Missing`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Installed,
    InSettings,
    Outdated { shim: PathBuf },
    Disabled,
    Missing,
}

impl Status {
    pub fn usable(&self) -> bool {
        matches!(self, Status::Installed | Status::InSettings)
    }

    pub fn is_fragment(&self) -> bool {
        matches!(self, Status::Installed | Status::Outdated { .. } | Status::Disabled)
    }
}

pub struct ProfileSetup {
    shim: PathBuf,
    pub status: Status,
    /// The program folder moved since the profile was written: never.
    pub moved: bool,
}

impl ProfileSetup {
    pub fn new(shim: PathBuf, _backups: PathBuf) -> ProfileSetup {
        ProfileSetup { shim, status: Status::Missing, moved: false }
    }

    pub fn refresh(&mut self) {}

    pub fn favorites_on(&self, _core: Option<&Core>) -> bool {
        false
    }

    pub fn fix_moved(&mut self) -> Option<String> {
        None
    }

    pub fn install_now(&mut self) -> Result<(), String> {
        Err(t!("profile-no-terminal"))
    }

    pub fn terminal_text(&self) -> String {
        t!("profile-no-terminal")
    }

    pub fn terminal_problem(&self) -> Option<String> {
        Some(t!("profile-no-terminal"))
    }

    pub fn remove_fragment(&mut self) -> Result<(), String> {
        Ok(())
    }

    pub fn describe(&self) -> String {
        t!("profile-no-terminal")
    }

    pub fn banner(&mut self, _ui: &mut egui::Ui, _notices: &mut [String]) {}

    pub fn shim_path(&self) -> &PathBuf {
        &self.shim
    }

    /// The terminal's settings file: there is none.
    pub fn settings_json(&self) -> PathBuf {
        PathBuf::new()
    }

    pub fn settings_ui(&mut self, ui: &mut egui::Ui, core: Option<&Core>, _notices: &mut [String]) {
        match core {
            Some(core) if !core.has_profile() && core.terminal_name() != "no terminal" => {
                ui.weak(t!("profile-driven-by", terminal = core.terminal_name()));
            }
            _ => {
                ui.weak(t!("profile-no-terminal"));
            }
        }
    }
}
