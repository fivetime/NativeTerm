//! NativeTerm's window: the session tree from `~/.ssh` and the open
//! sessions. First slice: connect (here or in a new window), focus,
//! reconnect, disconnect, close.
//!
//! `nativeterm [--terminal-dir <portable Terminal folder>] [--ssh-dir <dir>]
//! [--data-dir <dir>]` (also `NATIVETERM_TERMINAL_DIR`). Without a folder
//! the installed Windows Terminal is used. `--from-shim`: started by a
//! restored tab; exits quietly if NativeTerm is already running.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod agent;
mod app;
mod cleanup;
mod commands_import;
mod credential_sets;
mod dialogs;
mod dock;
mod fab;
mod files_sync;
mod files_window;
mod icons;
mod import_dialog;
mod key_dialog;
mod looks;
mod options_dialog;
mod plink_dialog;
mod send_dialog;
mod send_line;
mod server_sessions;
mod shell;
mod shortcut_ui;
mod storage;
mod tab_list;
#[cfg(windows)]
mod terminal_profile;
#[cfg(not(windows))]
#[path = "terminal_profile_stub.rs"]
mod terminal_profile;
mod tree_view;
mod window;
mod wizard;

#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;

use native_term_app::registry::Registry;
use native_term_app::{data_dir, data_lock, default_shim_path, diag, settings, t, Core};
#[cfg(windows)]
use native_term_platform::windows_terminal::install::{self, Install};
#[cfg(windows)]
use native_term_platform::windows_terminal::{Mismatch, WindowsTerminal};

use app::App;

pub struct Options {
    /// A portable Terminal's folder: meaningful where Windows Terminal is
    /// driven.
    #[cfg_attr(not(windows), allow(dead_code))]
    terminal_dir: Option<PathBuf>,
    pub ssh_dir: PathBuf,
    data_dir: Option<PathBuf>,
    from_shim: bool,
}

fn default_ssh_dir() -> Option<PathBuf> {
    native_term_os::home::ssh_dir()
}

fn options() -> Result<Options, String> {
    let mut terminal_dir = std::env::var_os("NATIVETERM_TERMINAL_DIR").map(PathBuf::from);
    let mut ssh_dir = default_ssh_dir();
    let mut data_dir = None;
    let mut from_shim = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().map(PathBuf::from).ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--terminal-dir" => terminal_dir = Some(value()?),
            "--ssh-dir" => ssh_dir = Some(value()?),
            "--data-dir" => data_dir = Some(value()?),
            "--from-shim" => from_shim = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Options { terminal_dir, ssh_dir: ssh_dir.ok_or("the home folder is not known")?, data_dir, from_shim })
}

/// The Terminal to drive: the one `--terminal-dir` names, the one that was
/// chosen in the settings, or the first one found. Several installs are
/// each their own single-instance app, so NativeTerm has to pick one; when
/// there is a choice and none was made, it says so.
#[cfg(windows)]
fn choose_install(dir: Option<&PathBuf>, chosen: Option<&Path>, notices: &mut Vec<String>) -> Result<Install, String> {
    if let Some(dir) = dir {
        return Install::from_dir(dir).map_err(|e| format!("{}: {e}", dir.display()));
    }
    let found = Install::discover(&[]);
    let first = found.first().ok_or_else(|| t!("fatal-no-terminal"))?;
    let same = |install: &&Install, want: &Path| {
        install.dir.as_os_str().eq_ignore_ascii_case(want.as_os_str()) || install.dir == want
    };
    if let Some(want) = chosen {
        if let Some(install) = found.iter().find(|i| same(i, want)) {
            return Ok(install.clone());
        }
        // a folder that was picked by hand (a portable copy) is not among
        // the installed packages, but it is still a Terminal
        if let Ok(install) = Install::from_dir(want) {
            return Ok(install);
        }
        notices.push(t!(
            "notice-terminal-choice-gone",
            chosen = want.display().to_string(),
            dir = first.dir.display().to_string()
        ));
    } else if found.len() > 1 {
        notices.push(t!("notice-terminal-several", count = found.len(), dir = first.dir.display().to_string()));
    }
    Ok(first.clone())
}

/// What the start checks found about the Terminal that was picked.
#[cfg(windows)]
fn install_notices(install: &Install, notices: &mut Vec<String>) {
    if let Some(version) = install.version.filter(|v| v.old()) {
        let (major, minor) = install::OLDEST;
        notices.push(t!("notice-terminal-old", version = version.to_string(), oldest = format!("{major}.{minor}")));
    }
    if install.alias_off() {
        match install.launcher_now() {
            Some(other) => notices.push(t!("notice-wt-alias-off", path = other.display().to_string())),
            None => notices.push(t!("notice-wt-missing", dir = install.dir.display().to_string())),
        }
    }
}

pub struct Setup {
    options: Options,
    /// Held while NativeTerm runs: this data directory is ours.
    _lock: Option<data_lock::DataLock>,
    #[cfg(windows)]
    install: Install,
    shim: PathBuf,
    core: Option<Core>,
    data_dir: PathBuf,
    /// How the data directory was chosen (see `data_dir`).
    data_source: &'static str,
    notices: Vec<String>,
}

enum Start {
    Run(Box<Setup>),
    /// Another NativeTerm serves the pipe.
    AlreadyRunning {
        quiet: bool,
    },
}

fn setup() -> Result<Start, String> {
    let options = options()?;
    let shim = default_shim_path().map_err(|e| e.to_string())?;
    let mut notices = Vec::new();
    let inputs = data_dir::Inputs::from_system(options.data_dir.clone()).map_err(|e| e.to_string())?;
    let (data_dir, data_source, registry) = match data_dir::resolve(&inputs) {
        Ok((dir, source)) => match Registry::open(&dir.join("state.db")) {
            Ok(registry) => (dir, source, Some(registry)),
            Err(e) => {
                notices.push(t!(
                    "notice-db-unavailable",
                    path = dir.join("state.db").display().to_string(),
                    error = e.to_string()
                ));
                (dir, source, None)
            }
        },
        Err(e) => {
            notices.push(t!("notice-no-data-dir", error = e.to_string()));
            (std::env::temp_dir().join("NativeTerm"), "--data-dir", None)
        }
    };
    // the settings live in a file of their own; an older data directory
    // has them in state.db, and they are taken over once
    let settings = std::sync::Arc::new(settings::Settings::open(&data_dir));
    if let Some(problem) = settings.problem() {
        notices.push(t!("notice-settings-unreadable", path = settings.path().display().to_string(), error = problem));
    } else if let Some(registry) = &registry {
        match settings.take_over(registry.all_settings().unwrap_or_default()) {
            Ok(0) => {}
            Ok(count) => diag::line(&format!("{count} settings taken over from state.db")),
            Err(e) => notices.push(t!(
                "notice-settings-not-written",
                path = settings.path().display().to_string(),
                error = e.to_string()
            )),
        }
    }
    diag::open(&data_dir.join("logs"));
    diag::line(&format!("data directory {} (from {data_source})", data_dir.display()));
    // a data directory can be on a share or a synced folder: say so when
    // another NativeTerm already has it
    let lock = match data_lock::take(&data_dir) {
        data_lock::Taken::Ours(lock) => Some(lock),
        data_lock::Taken::Busy(holder) => {
            notices.push(t!("notice-data-dir-busy", holder = holder.describe()));
            None
        }
        data_lock::Taken::Unavailable(e) => {
            diag::line(&format!("data directory lock: {e}"));
            None
        }
    };
    if !shim.exists() {
        notices.push(t!("notice-shim-missing", path = shim.display().to_string()));
    }
    // which Terminal, and is it one NativeTerm can work with (the settings
    // hold the choice, so this waits for the database)
    #[cfg(windows)]
    let (install, terminal) = {
        let chosen = settings.get(terminal_profile::INSTALL_SETTING).filter(|dir| !dir.is_empty()).map(PathBuf::from);
        let install = choose_install(options.terminal_dir.as_ref(), chosen.as_deref(), &mut notices)?;
        install_notices(&install, &mut notices);
        // before the window: restored tabs may already be waiting for an answer
        // another ssh folder than ~/.ssh: the tabs' shims look sessions up there
        let mut terminal = WindowsTerminal::new(install.clone(), &shim);
        if Some(&options.ssh_dir) != default_ssh_dir().as_ref() {
            terminal = terminal.with_ssh_dir(&options.ssh_dir);
        }
        match terminal.mismatch() {
            Some(Mismatch::WeAreElevated) => notices.push(t!("notice-we-are-elevated")),
            Some(Mismatch::TerminalElevated) => notices.push(t!("notice-terminal-elevated")),
            None => {}
        }
        (install, terminal)
    };
    // no terminal is driven here yet: the list and the settings, no tabs
    #[cfg(not(windows))]
    let terminal = {
        notices.push(t!("notice-no-terminal-backend"));
        native_term_platform::stub::NoTerminal::new(&shim)
    };
    let core = match Core::start(terminal, registry) {
        Ok(core) => Some(core),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            return Ok(Start::AlreadyRunning { quiet: options.from_shim });
        }
        Err(e) => {
            notices.push(t!("notice-no-pipe", error = e.to_string()));
            None
        }
    };
    if let Some(core) = &core {
        core.set_settings(settings.clone());
        if let Some(language) = core.language_setting() {
            native_term_app::i18n::set_language(Some(&language));
        }
    }
    Ok(Start::Run(Box::new(Setup {
        options,
        _lock: lock,
        #[cfg(windows)]
        install,
        shim,
        core,
        data_dir,
        data_source,
        notices,
    })))
}

fn main() {
    let setup = match setup() {
        Ok(Start::AlreadyRunning { quiet }) => {
            if !quiet {
                if let Ok(exe) = std::env::current_exe() {
                    for window in native_term_os::desktop::windows_of_other_instances(&exe) {
                        native_term_os::desktop::bring_to_front(window);
                    }
                }
            }
            return;
        }
        Ok(Start::Run(setup)) => Ok(*setup),
        Err(e) => Err(e),
    };
    let viewport = egui::ViewportBuilder::default()
        .with_title("NativeTerm")
        .with_app_id("NativeTerm")
        .with_inner_size([960.0, 640.0]);
    // where the window was, and a way to remember it
    let settings = setup.as_ref().ok().and_then(|s| s.core.clone());
    let settings_core = settings.clone();
    let placement = settings
        .as_ref()
        .and_then(|core| core.setting(WINDOW_SETTING))
        .and_then(|text| window::Placement::from_setting(&text));
    let save: window::SavePlacement = Box::new(move |p| {
        if let Some(core) = &settings {
            core.set_setting(WINDOW_SETTING, &p.to_setting());
        }
    });
    let button = floating_button(settings_core.clone());
    let result = window::run(viewport, placement, save, Some(button), move |ctx| match setup {
        Ok(setup) => Box::new(App::new(ctx, setup)),
        Err(e) => Box::new(Fatal(e)),
    });
    if let Err(e) = result {
        diag::close(&format!("window failed: {e}"));
        native_term_os::desktop::message_box("NativeTerm", &t!("fatal-window", error = e));
        std::process::exit(1);
    }
    diag::close("NativeTerm stopped");
}

/// `state.db` setting: where the floating button was (`x,y`).
const BUTTON_SETTING: &str = "fab";

fn floating_button(core: Option<Core>) -> window::FloatingButton {
    let position = core.as_ref().and_then(|c| c.setting(BUTTON_SETTING)).and_then(|text| {
        let (x, y) = text.split_once(',')?;
        Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
    });
    let saver = core.clone();
    window::FloatingButton {
        viewport: egui::ViewportBuilder::default()
            .with_title("NativeTerm")
            .with_inner_size([fab::BUTTON, fab::BUTTON])
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false),
        position,
        save: Box::new(move |x, y| {
            if let Some(core) = &saver {
                core.set_setting(BUTTON_SETTING, &format!("{x},{y}"));
            }
        }),
        factory: Box::new(move |_| Box::new(fab::Fab::new(core))),
    }
}

/// `state.db` setting: the window's placement.
const WINDOW_SETTING: &str = "window";

/// Chinese text needs a system font; egui's own fonts have no CJK.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    use native_term_os::fonts;
    // mapped, not read: egui would keep two private copies of a 20 MB file
    let mut fonts = egui::FontDefinitions::default();
    let cjk = fonts::cjk_file().and_then(|f| fonts::map_file(&f).ok());
    // icons last: their code points (private use area) are in no other font
    let glyphs = fonts::icon_file().and_then(|f| fonts::map_file(&f).ok());
    for (name, bytes) in [("cjk", cjk), ("icons", glyphs)] {
        let Some(bytes) = bytes else { continue };
        fonts.font_data.insert(name.into(), std::sync::Arc::new(egui::FontData::from_static(bytes)));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(name.into());
        }
    }
    // no icon font on the system: Phosphor, bundled (see `icons`)
    #[cfg(not(windows))]
    {
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        fonts.families.entry(egui::FontFamily::Monospace).or_default().push("phosphor".into());
    }
    ctx.set_fonts(fonts);
}

struct Fatal(String);

impl window::Ui for Fatal {
    fn ui(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.heading(t!("fatal-title"));
            ui.label(&self.0);
        });
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use native_term_platform::windows_terminal::install::{Kind, Version};

    /// A folder that looks like an unpackaged Terminal.
    fn fake_terminal(dir: &Path) {
        for exe in ["WindowsTerminal.exe", "wt.exe"] {
            std::fs::write(dir.join(exe), "").unwrap();
        }
    }

    #[test]
    fn a_chosen_folder_is_used_even_when_it_is_not_an_installed_package() {
        let tmp = tempfile::tempdir().unwrap();
        fake_terminal(tmp.path());
        std::fs::write(tmp.path().join(".portable"), "").unwrap();
        let mut notices = Vec::new();
        let install = choose_install(None, Some(tmp.path()), &mut notices).unwrap();
        assert_eq!(install.kind, Kind::Portable);
        assert_eq!(install.dir, std::path::absolute(tmp.path()).unwrap());
        assert!(notices.is_empty(), "nothing to say: the choice was there");
    }

    #[test]
    fn a_chosen_folder_that_is_gone_is_said_once() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("moved-away");
        let mut notices = Vec::new();
        match choose_install(None, Some(&gone), &mut notices) {
            // whatever is installed here is used instead, with a word
            Ok(_) => assert_eq!(notices.len(), 1, "{notices:?}"),
            // no Terminal on this machine: that is the fatal error, not a notice
            Err(_) => assert!(notices.is_empty()),
        }
    }

    #[test]
    fn the_start_checks_say_what_is_wrong() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("package");
        std::fs::create_dir(&dir).unwrap();
        fake_terminal(&dir);
        let mut install = Install {
            dir: dir.clone(),
            kind: Kind::Packaged,
            // the alias Windows makes for a packaged install, turned off
            launcher: tmp.path().join("WindowsApps").join("wt.exe"),
            settings_dir: tmp.path().join("LocalState"),
            family: Some("Microsoft.WindowsTerminal_8wekyb3d8bbwe".to_string()),
            version: Some(Version(1, 20, 11781, 0)),
        };
        let mut notices = Vec::new();
        install_notices(&install, &mut notices);
        assert_eq!(notices.len(), 2, "too old, and the alias is off: {notices:?}");
        assert!(notices[0].contains("1.20.11781.0"), "{}", notices[0]);
        assert!(notices[1].contains(&dir.join("wt.exe").display().to_string()), "{}", notices[1]);

        // a current version with its alias in place has nothing to report
        install.version = Some(Version(1, 26, 2609, 0));
        install.launcher = dir.join("wt.exe");
        let mut notices = Vec::new();
        install_notices(&install, &mut notices);
        assert!(notices.is_empty(), "{notices:?}");
    }
}
