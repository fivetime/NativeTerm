//! NativeTerm's window: the session tree from `~/.ssh` and the open
//! sessions. First slice: connect (here or in a new window), focus,
//! reconnect, disconnect, close.
//!
//! `nativeterm [--terminal-dir <portable Terminal folder>] [--ssh-dir <dir>]
//! [--data-dir <dir>]` (also `NATIVETERM_TERMINAL_DIR`). Without a folder
//! the installed Windows Terminal is used. `--from-shim`: started by a
//! restored tab; exits quietly if NativeTerm is already running.

#![windows_subsystem = "windows"]

mod app;
mod dialogs;
mod dock;
mod import_dialog;
mod terminal_profile;
mod tree_view;
mod window;

use std::path::PathBuf;

use native_term_app::registry::Registry;
use native_term_app::{data_dir, default_shim_path, t, Core};
use native_term_platform::windows_terminal::install::Install;
use native_term_platform::windows_terminal::WindowsTerminal;

use app::App;

pub struct Options {
    terminal_dir: Option<PathBuf>,
    pub ssh_dir: PathBuf,
    data_dir: Option<PathBuf>,
    from_shim: bool,
}

fn options() -> Result<Options, String> {
    let mut terminal_dir = std::env::var_os("NATIVETERM_TERMINAL_DIR").map(PathBuf::from);
    let mut ssh_dir = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".ssh"));
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
    Ok(Options { terminal_dir, ssh_dir: ssh_dir.ok_or("USERPROFILE is not set")?, data_dir, from_shim })
}

fn choose_install(dir: Option<&PathBuf>) -> Result<Install, String> {
    match dir {
        Some(dir) => Install::from_dir(dir).map_err(|e| format!("{}: {e}", dir.display())),
        None => Install::discover(&[]).into_iter().next().ok_or_else(|| "Windows Terminal is not installed".to_string()),
    }
}

pub struct Setup {
    options: Options,
    install: Install,
    shim: PathBuf,
    core: Option<Core>,
    data_dir: PathBuf,
    notices: Vec<String>,
}

enum Start {
    Run(Box<Setup>),
    /// Another NativeTerm serves the pipe.
    AlreadyRunning { quiet: bool },
}

fn setup() -> Result<Start, String> {
    let options = options()?;
    let install = choose_install(options.terminal_dir.as_ref())?;
    let shim = default_shim_path().map_err(|e| e.to_string())?;
    let mut notices = Vec::new();
    let inputs = data_dir::Inputs::from_system(options.data_dir.clone()).map_err(|e| e.to_string())?;
    let (data_dir, registry) = match data_dir::resolve(&inputs) {
        Ok((dir, _)) => match Registry::open(&dir.join("state.db")) {
            Ok(registry) => (dir, Some(registry)),
            Err(e) => {
                notices.push(t!("notice-db-unavailable", path = dir.join("state.db").display().to_string(), error = e.to_string()));
                (dir, None)
            }
        },
        Err(e) => {
            notices.push(t!("notice-no-data-dir", error = e.to_string()));
            (std::env::temp_dir().join("NativeTerm"), None)
        }
    };
    if !shim.exists() {
        notices.push(t!("notice-shim-missing", path = shim.display().to_string()));
    }
    // before the window: restored tabs may already be waiting for an answer
    let core = match Core::start(WindowsTerminal::new(install.clone(), &shim), registry) {
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
        if let Some(language) = core.language_setting() {
            native_term_app::i18n::set_language(Some(&language));
        }
    }
    Ok(Start::Run(Box::new(Setup { options, install, shim, core, data_dir, notices })))
}

fn main() {
    let setup = match setup() {
        Ok(Start::AlreadyRunning { quiet }) => {
            if !quiet {
                if let Ok(exe) = std::env::current_exe() {
                    for window in native_term_win::desktop::windows_of_other_instances(&exe) {
                        native_term_win::desktop::bring_to_front(window);
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
    let placement = settings
        .as_ref()
        .and_then(|core| core.setting(WINDOW_SETTING))
        .and_then(|text| window::Placement::from_setting(&text));
    let save: window::SavePlacement = Box::new(move |p| {
        if let Some(core) = &settings {
            core.set_setting(WINDOW_SETTING, &p.to_setting());
        }
    });
    let result = window::run(viewport, placement, save, move |ctx| {
        install_fonts(ctx);
        match setup {
            Ok(setup) => Box::new(App::new(ctx, setup)),
            Err(e) => Box::new(Fatal(e)),
        }
    });
    if let Err(e) = result {
        native_term_win::desktop::message_box("NativeTerm", &t!("fatal-window", error = e));
        std::process::exit(1);
    }
}

/// `state.db` setting: the window's placement.
const WINDOW_SETTING: &str = "window";

/// Chinese text needs a system font; egui's own fonts have no CJK.
fn install_fonts(ctx: &egui::Context) {
    let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    // mapped, not read: egui would keep two private copies of a 20 MB file
    let Some(bytes) = ["msyh.ttc", "simsun.ttc"]
        .iter()
        .find_map(|f| native_term_win::map_file_for_process(&windir.join("Fonts").join(f)).ok())
    else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("cjk".into(), std::sync::Arc::new(egui::FontData::from_static(bytes)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("cjk".into());
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
