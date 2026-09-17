//! NativeTerm's window: the session tree from `~/.ssh` and the open
//! sessions. First slice: connect (here or in a new window), focus,
//! reconnect, disconnect, close.
//!
//! `nativeterm [--terminal-dir <portable Terminal folder>] [--ssh-dir <dir>]
//! [--data-dir <dir>]` (also `NATIVETERM_TERMINAL_DIR`). Without a folder
//! the installed Windows Terminal is used. `--from-shim`: started by a
//! restored tab; exits quietly if NativeTerm is already running.

#![windows_subsystem = "windows"]

mod terminal_profile;

use std::path::PathBuf;

use eframe::egui;
use native_term_app::registry::Registry;
use native_term_app::{data_dir, default_shim_path, Core, HostRequest, SessionView, State};
use native_term_config::SessionTree;
use native_term_platform::windows_terminal::install::Install;
use native_term_platform::windows_terminal::WindowsTerminal;
use terminal_profile::ProfileSetup;
use native_term_platform::Target;

struct Options {
    terminal_dir: Option<PathBuf>,
    ssh_dir: PathBuf,
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

struct Setup {
    options: Options,
    install: Install,
    shim: PathBuf,
    core: Option<Core>,
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
    let registry = match data_dir::resolve(&inputs) {
        Ok((dir, _)) => match Registry::open(&dir.join("state.db")) {
            Ok(registry) => Some(registry),
            Err(e) => {
                notices.push(format!("{}: {e}; open sessions won't be remembered", dir.join("state.db").display()));
                None
            }
        },
        Err(e) => {
            notices.push(format!("No data directory: {e}; open sessions won't be remembered"));
            None
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
    Ok(Start::Run(Box::new(Setup { options, install, shim, core, notices })))
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

struct App {
    core: Option<Core>,
    tree: SessionTree,
    ssh_dir: PathBuf,
    profile: ProfileSetup,
    show_settings: bool,
    notices: Vec<String>,
    selected_host: Option<String>,
}

impl App {
    fn new(cc: &eframe::CreationContext, setup: Setup) -> App {
        let Setup { options, install, shim, core, mut notices } = setup;
        let mut profile = ProfileSetup::new(install, shim);
        notices.extend(profile.fix_moved());
        if let Some(core) = &core {
            let ctx = cc.egui_ctx.clone();
            core.set_repaint(move || ctx.request_repaint());
        }
        let tree = SessionTree::load(&options.ssh_dir);
        App {
            core,
            tree,
            ssh_dir: options.ssh_dir,
            profile,
            show_settings: false,
            notices,
            selected_host: None,
        }
    }

    fn open(&self, hosts: Vec<HostRequest>, target: Target) {
        if let Some(core) = &self.core {
            if !hosts.is_empty() {
                core.open(&hosts, target);
            }
        }
    }

    fn tree_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Sessions");
            if ui.small_button("⟳").on_hover_text("Reload ~/.ssh").clicked() {
                self.tree = SessionTree::load(&self.ssh_dir);
            }
        });
        ui.separator();
        let mut requests: Vec<(Vec<HostRequest>, Target)> = Vec::new();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for folder in self.tree.folders() {
                let hosts: Vec<HostRequest> = folder
                    .hosts
                    .iter()
                    .map(|h| HostRequest { alias: h.alias().to_string(), label: h.label().to_string() })
                    .collect();
                let title = if folder.name.is_empty() { "~/.ssh/config".to_string() } else { folder.label().to_string() };
                let header = egui::CollapsingHeader::new(format!("{title}  ({})", hosts.len()))
                    .id_salt(&folder.file)
                    .default_open(true)
                    .show(ui, |ui| {
                        for (host, request) in folder.hosts.iter().zip(&hosts) {
                            let selected = self.selected_host.as_deref() == Some(host.alias());
                            let row = ui.selectable_label(selected, host.label()).on_hover_text(format!(
                                "{}{}{}",
                                host.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default(),
                                host.target(),
                                host.port.map(|p| format!(":{p}")).unwrap_or_default()
                            ));
                            if row.clicked() {
                                self.selected_host = Some(host.alias().to_string());
                            }
                            if row.double_clicked() {
                                requests.push((vec![request.clone()], Target::Recent));
                            }
                            row.context_menu(|ui| {
                                if ui.button("Connect").clicked() {
                                    requests.push((vec![request.clone()], Target::Recent));
                                    ui.close_menu();
                                }
                                if ui.button("Connect in New Window").clicked() {
                                    requests.push((vec![request.clone()], Target::NewWindow));
                                    ui.close_menu();
                                }
                            });
                        }
                    });
                header.header_response.context_menu(|ui| {
                    if ui.button("Connect All").clicked() {
                        requests.push((hosts.clone(), Target::Recent));
                        ui.close_menu();
                    }
                    if ui.button("Connect All in New Window").clicked() {
                        requests.push((hosts.clone(), Target::NewWindow));
                        ui.close_menu();
                    }
                });
            }
            if self.tree.hosts().next().is_none() {
                ui.label(format!("No hosts in {}", self.ssh_dir.display()));
            }
        });
        for (hosts, target) in requests {
            self.open(hosts, target);
        }
    }

    fn sessions_panel(&mut self, ui: &mut egui::Ui) {
        let Some(core) = self.core.clone() else { return };
        let sessions = core.sessions();
        ui.horizontal(|ui| {
            ui.heading(format!("Open sessions ({})", sessions.iter().filter(|s| s.state.is_open()).count()));
            if ui.button("Clear finished").clicked() {
                core.clear_finished();
            }
        });
        ui.separator();
        if sessions.is_empty() {
            ui.label("Double-click a host, or right-click it or a folder for more.");
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("sessions").striped(true).num_columns(4).show(ui, |ui| {
                ui.strong("Session");
                ui.strong("State");
                ui.strong("Tab");
                ui.strong("");
                ui.end_row();
                for s in &sessions {
                    session_row(ui, &core, s);
                    ui.end_row();
                }
            });
        });
    }
}

fn state_color(ui: &egui::Ui, state: &State) -> egui::Color32 {
    match state {
        State::Connected => egui::Color32::from_rgb(0x2e, 0xa0, 0x43),
        State::Opening | State::Connecting => egui::Color32::from_rgb(0xd0, 0x9a, 0x1a),
        State::LoginFailed(_) | State::Disconnected(_) | State::Failed(_) => egui::Color32::from_rgb(0xd0, 0x3a, 0x3a),
        _ => ui.visuals().weak_text_color(),
    }
}

fn session_row(ui: &mut egui::Ui, core: &Core, s: &SessionView) {
    let label = ui.label(&s.label);
    if s.label != s.alias {
        label.on_hover_text(&s.alias);
    }
    let mut state = s.state.describe();
    if s.attempt > 1 && s.state.is_open() {
        state.push_str(&format!(" · attempt {}", s.attempt));
    }
    ui.colored_label(state_color(ui, &s.state), state);
    match &s.location {
        Some(l) => {
            let mut text = format!("window {} · tab {}", l.window_number, l.tab_index + 1);
            if l.selected {
                text.push_str(" · selected");
            }
            if l.mixed {
                text.push_str(" · split");
            }
            let response = ui.label(text);
            if l.title != s.label {
                response.on_hover_text(format!("current title: {}", l.title));
            }
        }
        None if s.state.is_open() => {
            ui.weak("not located");
        }
        None => {
            ui.label("");
        }
    }
    ui.horizontal(|ui| {
        let open = s.state.is_open();
        if ui.add_enabled(s.location.is_some(), egui::Button::new("Focus")).clicked() {
            core.focus(&s.id);
        }
        let label = if s.state == State::Waiting { "Connect" } else { "Reconnect" };
        if ui.add_enabled(open && s.linked && s.state.can_connect(), egui::Button::new(label)).clicked() {
            core.connect(&s.id);
        }
        let live = matches!(s.state, State::Connecting | State::Connected);
        if ui.add_enabled(open && s.linked && live, egui::Button::new("Disconnect")).clicked() {
            core.disconnect(&s.id);
        }
        if ui.add_enabled(open, egui::Button::new("Close")).clicked() {
            core.close(&s.id);
        }
    });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(core) = &self.core {
            self.notices.extend(core.take_notices());
        }
        egui::TopBottomPanel::top("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.toggle_value(&mut self.show_settings, "⚙ Settings");
            });
            if self.show_settings {
                ui.group(|ui| self.profile.settings_ui(ui, &mut self.notices));
            }
            self.profile.banner(ui, &mut self.notices);
            if !self.notices.is_empty() {
                let mut clear = false;
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x9a, 0x1a), self.notices.join("  ·  "));
                    clear = ui.small_button("✕").clicked();
                });
                if clear {
                    self.notices.clear();
                }
            }
        });
        egui::SidePanel::left("tree").resizable(true).default_width(300.0).show(ctx, |ui| self.tree_panel(ui));
        egui::CentralPanel::default().show(ctx, |ui| self.sessions_panel(ui));
    }
}
