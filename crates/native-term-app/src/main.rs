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
mod terminal_profile;
mod tree_view;

use std::path::PathBuf;

use eframe::egui;
use native_term_app::registry::Registry;
use native_term_app::{data_dir, default_shim_path, Core};
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
                notices.push(format!("{}: {e}; open sessions won't be remembered", dir.join("state.db").display()));
                (dir, None)
            }
        },
        Err(e) => {
            notices.push(format!("No data directory: {e}; open sessions won't be remembered"));
            (std::env::temp_dir().join("NativeTerm"), None)
        }
    };
    if !shim.exists() {
        notices.push(format!("{} is missing; tabs can't start", shim.display()));
    }
    // before the window: restored tabs may already be waiting for an answer
    let core = match Core::start(WindowsTerminal::new(install.clone(), &shim), registry) {
        Ok(core) => Some(core),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            return Ok(Start::AlreadyRunning { quiet: options.from_shim });
        }
        Err(e) => {
            notices.push(format!("NativeTerm can't serve its pipe: {e}"));
            None
        }
    };
    Ok(Start::Run(Box::new(Setup { options, install, shim, core, data_dir, notices })))
}

fn main() -> eframe::Result<()> {
    let setup = match setup() {
        Ok(Start::AlreadyRunning { quiet }) => {
            if !quiet {
                if let Ok(exe) = std::env::current_exe() {
                    for window in native_term_win::desktop::windows_of_other_instances(&exe) {
                        native_term_win::desktop::bring_to_front(window);
                    }
                }
            }
            return Ok(());
        }
        Ok(Start::Run(setup)) => Ok(*setup),
        Err(e) => Err(e),
    };
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("NativeTerm").with_inner_size([960.0, 640.0]),
        wgpu_options: wgpu_options(),
        ..Default::default()
    };
    let result = eframe::run_native(
        "NativeTerm",
        native,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            let app: Box<dyn eframe::App> = match setup {
                Ok(setup) => Box::new(App::new(cc, setup)),
                Err(e) => Box::new(Fatal(e)),
            };
            Ok(app)
        }),
    );
    // no Vulkan (VMs, remote sessions, old drivers): a window system can
    // only be set up once per process, so start again with Direct3D 12
    if let Err(eframe::Error::Wgpu(_)) = &result {
        let first_try = std::env::var_os(FALLBACK_ENV).is_none() && std::env::var_os("WGPU_BACKEND").is_none();
        if let (true, Ok(exe)) = (first_try, std::env::current_exe()) {
            if std::process::Command::new(exe).args(std::env::args_os().skip(1)).env(FALLBACK_ENV, "1").spawn().is_ok() {
                return Ok(());
            }
        }
    }
    result
}

/// Set when NativeTerm restarted itself because the first graphics API
/// didn't work.
const FALLBACK_ENV: &str = "NATIVETERM_RENDERER_FALLBACK";

/// A small UI: the integrated GPU where there is a choice (a discrete one
/// costs battery), one graphics API, and wgpu's memory-saving allocator.
/// Vulkan is the default: idle, it took 79 MB private memory against 165 MB
/// with Direct3D 12 (106 MB with both enabled). `WGPU_BACKEND` overrides it.
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::wgpu;
    let defaults = eframe::egui_wgpu::WgpuConfiguration::default();
    let base = defaults.device_descriptor.clone();
    let preferred = if std::env::var_os(FALLBACK_ENV).is_some() { wgpu::Backends::DX12 } else { wgpu::Backends::VULKAN };
    eframe::egui_wgpu::WgpuConfiguration {
        supported_backends: wgpu::util::backend_bits_from_env().unwrap_or(preferred),
        power_preference: wgpu::util::power_preference_from_env().unwrap_or(wgpu::PowerPreference::LowPower),
        device_descriptor: std::sync::Arc::new(move |adapter| wgpu::DeviceDescriptor {
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..base(adapter)
        }),
        ..defaults
    }
}

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
    fonts.font_data.insert("cjk".into(), egui::FontData::from_static(bytes));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("cjk".into());
    }
    ctx.set_fonts(fonts);
}

struct Fatal(String);

impl eframe::App for Fatal {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("NativeTerm can't start");
            ui.label(&self.0);
        });
    }
}
