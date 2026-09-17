//! NativeTerm's window: the session tree from `~/.ssh` and the open
//! sessions. First slice: connect (here or in a new window), focus,
//! reconnect, disconnect, close.
//!
//! `nativeterm [--terminal-dir <portable Terminal folder>] [--ssh-dir <dir>]`
//! (also `NATIVETERM_TERMINAL_DIR`). Without a folder the installed
//! Windows Terminal is used.

#![windows_subsystem = "windows"]

use std::path::PathBuf;

use eframe::egui;
use native_term_app::{default_shim_path, Core, HostRequest, SessionView, State};
use native_term_config::SessionTree;
use native_term_platform::windows_terminal::install::Install;
use native_term_platform::windows_terminal::{command, profile, WindowsTerminal};
use native_term_platform::Target;

struct Options {
    terminal_dir: Option<PathBuf>,
    ssh_dir: PathBuf,
}

fn options() -> Result<Options, String> {
    let mut terminal_dir = std::env::var_os("NATIVETERM_TERMINAL_DIR").map(PathBuf::from);
    let mut ssh_dir = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".ssh"));
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().map(PathBuf::from).ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--terminal-dir" => terminal_dir = Some(value()?),
            "--ssh-dir" => ssh_dir = Some(value()?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Options { terminal_dir, ssh_dir: ssh_dir.ok_or("USERPROFILE is not set")? })
}

fn choose_install(dir: Option<&PathBuf>) -> Result<Install, String> {
    match dir {
        Some(dir) => Install::from_dir(dir).map_err(|e| format!("{}: {e}", dir.display())),
        None => Install::discover(&[]).into_iter().next().ok_or_else(|| "Windows Terminal is not installed".to_string()),
    }
}

/// Whether the chosen Terminal knows the "NativeTerm SSH" profile.
fn has_profile(install: &Install) -> bool {
    let in_settings = std::fs::read_to_string(install.settings_json())
        .is_ok_and(|s| s.contains(&format!("\"{}\"", command::PROFILE_NAME)));
    let in_fragment = profile::user_fragments_root().is_some_and(|root| profile::fragment_path(&root).exists());
    in_settings || in_fragment
}

fn main() -> eframe::Result<()> {
    let setup = options().and_then(|o| {
        let install = choose_install(o.terminal_dir.as_ref())?;
        let shim = default_shim_path().map_err(|e| e.to_string())?;
        Ok((o, install, shim))
    });
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
                Ok((options, install, shim)) => Box::new(App::new(cc, options, install, shim)),
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
    terminal_label: String,
    profile_missing: bool,
    notices: Vec<String>,
    selected_host: Option<String>,
}

impl App {
    fn new(cc: &eframe::CreationContext, options: Options, install: Install, shim: PathBuf) -> App {
        let ctx = cc.egui_ctx.clone();
        let terminal_label = format!("{} ({:?})", install.dir.display(), install.kind);
        let profile_missing = !has_profile(&install);
        let mut notices = Vec::new();
        if !shim.exists() {
            notices.push(format!("{} is missing; tabs can't start", shim.display()));
        }
        let core = match Core::start(WindowsTerminal::new(install, &shim), move || ctx.request_repaint()) {
            Ok(core) => Some(core),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                notices.push("NativeTerm is already running.".into());
                None
            }
            Err(e) => {
                notices.push(format!("NativeTerm can't serve its pipe: {e}"));
                None
            }
        };
        let tree = SessionTree::load(&options.ssh_dir);
        App {
            core,
            tree,
            ssh_dir: options.ssh_dir,
            terminal_label,
            profile_missing,
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
        let ended = matches!(s.state, State::LoginFailed(_) | State::Disconnected(_) | State::Ended(_));
        if ui.add_enabled(open && s.linked && ended, egui::Button::new("Reconnect")).clicked() {
            core.reconnect(&s.id);
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
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Windows Terminal: {}", self.terminal_label));
                if self.profile_missing {
                    ui.colored_label(
                        egui::Color32::from_rgb(0xd0, 0x3a, 0x3a),
                        "The \"NativeTerm SSH\" profile is missing in this Terminal.",
                    );
                }
            });
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
