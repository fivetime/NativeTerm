//! The "NativeTerm SSH" profile in the chosen Windows Terminal: status,
//! install and removal (a fragment, see `profile.rs` in the platform crate).
//! Installing is the user's action; only a fragment NativeTerm installed
//! itself is rewritten on its own, when the program folder moved.

use std::path::PathBuf;

use native_term_app::{t, Core};
use native_term_platform::windows_terminal::install::{self, Install, Kind};
use native_term_platform::windows_terminal::profile::{self, Status};
use native_term_platform::windows_terminal::sources;

/// `state.db` setting: the folder of the Terminal install to drive, when
/// more than one is installed. Read at start (see `choose_install`).
pub const INSTALL_SETTING: &str = "terminal.install";

pub struct ProfileSetup {
    install: Install,
    shim: PathBuf,
    root: Option<PathBuf>,
    pub status: Status,
    /// Where `settings.json` is copied before NativeTerm changes it.
    backups: PathBuf,
    /// Terminal's own SSH profiles are turned off (`None`: unreadable).
    ssh_hidden: Option<bool>,
    /// The fragment named another helper at start and was put right: the
    /// program folder moved (or a second copy had it).
    pub moved: bool,
}

impl ProfileSetup {
    pub fn new(install: Install, shim: PathBuf, backups: PathBuf) -> ProfileSetup {
        let root = profile::fragments_root();
        let status = profile::status(&install, root.as_deref(), &shim);
        let mut setup = ProfileSetup { install, shim, root, status, backups, ssh_hidden: None, moved: false };
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

    /// Rewrite our own fragment when it names another helper: the program
    /// folder moved, or a second copy of NativeTerm had it.
    ///
    /// A helper that is no longer there means this program moved, which
    /// needs no telling; one that is still there means there are two
    /// copies, and then the change is worth a word.
    pub fn fix_moved(&mut self) -> Option<String> {
        let Status::Outdated { shim: old } = &self.status else { return None };
        let old = old.clone();
        let root = self.root.clone()?;
        self.moved = true;
        let result = profile::install(&root, &self.shim, &self.settings_files());
        self.refresh();
        match result {
            Ok(_) if !old.exists() => None,
            Ok(_) => {
                Some(t!("profile-updated", old = old.display().to_string(), new = self.shim.display().to_string()))
            }
            Err(e) => Some(t!("profile-update-failed", error = e.to_string())),
        }
    }

    /// Install the fragment (the user asked, e.g. in the wizard).
    pub fn install_now(&mut self) -> Result<(), String> {
        self.install_fragment()
    }

    /// The chosen Terminal, in words.
    pub fn terminal_text(&self) -> String {
        let version = self.install.version.map(|v| format!(", {v}")).unwrap_or_default();
        format!("{} ({}{version})", self.install.dir.display(), kind_name(&self.install.kind))
    }

    /// What stands between NativeTerm and the chosen Terminal, if anything
    /// (the same checks as at start, for the wizard). An app execution
    /// alias that is off is not one: the package's own copy is used.
    pub fn terminal_problem(&self) -> Option<String> {
        if let Some(version) = self.install.version.filter(|v| v.old()) {
            let (major, minor) = install::OLDEST;
            return Some(t!("notice-terminal-old", version = version.to_string(), oldest = format!("{major}.{minor}")));
        }
        if self.install.launcher_now().is_none() {
            return Some(t!("notice-wt-missing", dir = self.install.dir.display().to_string()));
        }
        None
    }

    fn install_fragment(&mut self) -> Result<(), String> {
        let root = self.root.clone().ok_or_else(|| t!("profile-no-localappdata"))?;
        profile::install(&root, &self.shim, &self.settings_files()).map_err(|e| e.to_string())?;
        self.refresh();
        Ok(())
    }

    pub fn remove_fragment(&mut self) -> Result<(), String> {
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
                let path =
                    self.root.as_ref().map(|r| profile::fragment_path(r).display().to_string()).unwrap_or_default();
                if ui.button(t!("profile-install")).on_hover_text(t!("profile-install-hint", path = path)).clicked() {
                    if let Err(e) = self.install_fragment() {
                        notices.push(t!("profile-install-failed", error = e));
                    }
                }
            }
        });
    }

    /// The helper every tab runs (what NativeTerm wrote into the profile
    /// and into the `ProxyCommand` lines).
    pub fn shim_path(&self) -> &PathBuf {
        &self.shim
    }

    /// The Terminal's own `settings.json` (its key bindings, among others).
    pub fn settings_json(&self) -> PathBuf {
        self.install.settings_json()
    }

    /// Which Terminal NativeTerm drives. Each install is its own
    /// single-instance app, so only one can be driven at a time; the
    /// choice is read at the next start.
    fn install_choice(&self, ui: &mut egui::Ui, core: Option<&Core>, notices: &mut Vec<String>) {
        let Some(core) = core else { return };
        // the installed packages, and the one in use if it is neither of
        // them (a portable copy, or --terminal-dir)
        let mut others = Install::discover(&[]);
        if !others.iter().any(|i| i.dir == self.install.dir) {
            others.insert(0, self.install.clone());
        }
        if others.len() < 2 {
            return;
        }
        // what is shown is the choice, not what is running: it can be
        // changed back before the next start
        let chosen = core.setting(INSTALL_SETTING).map(PathBuf::from).unwrap_or_else(|| self.install.dir.clone());
        let mut pick = chosen.clone();
        let text = others.iter().find(|i| i.dir == chosen).map(name_of).unwrap_or_else(|| chosen.display().to_string());
        ui.horizontal(|ui| {
            ui.label(t!("settings-terminal-pick"));
            egui::ComboBox::from_id_salt("terminal-install").selected_text(text).show_ui(ui, |ui| {
                for install in &others {
                    ui.selectable_value(&mut pick, install.dir.clone(), name_of(install));
                }
            });
        });
        if pick != chosen {
            core.set_setting(INSTALL_SETTING, &pick.to_string_lossy());
            if pick == self.install.dir {
                notices.push(t!("settings-terminal-picked-current", dir = pick.display().to_string()));
            } else {
                notices.push(t!("settings-terminal-picked", dir = pick.display().to_string()));
            }
        }
    }

    pub fn settings_ui(&mut self, ui: &mut egui::Ui, core: Option<&Core>, notices: &mut Vec<String>) {
        ui.label(t!(
            "settings-terminal",
            dir = self.install.dir.display().to_string(),
            kind = kind_name(&self.install.kind)
        ));
        if let Some(version) = self.install.version {
            ui.label(t!("settings-terminal-version", version = version.to_string()));
        }
        self.install_choice(ui, core, notices);
        ui.label(t!("settings-profile", status = self.describe()));
        ui.horizontal(|ui| {
            let installed = matches!(self.status, Status::Installed | Status::Outdated { .. } | Status::Disabled);
            if ui
                .add_enabled(!matches!(self.status, Status::Installed), egui::Button::new(t!("profile-install-update")))
                .clicked()
            {
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
                    Ok(Some(backup)) => {
                        notices.push(t!("settings-terminal-changed", backup = backup.display().to_string()))
                    }
                    Ok(None) => {}
                    Err(e) => notices.push(t!("settings-terminal-change-failed", error = e)),
                }
            }
        }
    }
}

/// How an install is named in the picker: its kind, version and folder.
fn name_of(install: &Install) -> String {
    let version = install.version.map(|v| v.to_string()).unwrap_or_default();
    format!("{} {version} — {}", kind_name(&install.kind), install.dir.display())
}

fn kind_name(kind: &Kind) -> String {
    match kind {
        Kind::Packaged => t!("terminal-kind-packaged"),
        Kind::Portable => t!("terminal-kind-portable"),
        Kind::Unpackaged => t!("terminal-kind-unpackaged"),
    }
}
