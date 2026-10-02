//! NativeTerm's window: the session tree from `~/.ssh` and the open
//! sessions. First slice: connect (here or in a new window), focus,
//! reconnect, disconnect, close.
//!
//! `nativeterm [--terminal-dir <portable Terminal folder>] [--ssh-dir <dir>]
//! [--data-dir <dir>] [--terminal wezterm[=<folder>]]` (also
//! `NATIVETERM_TERMINAL_DIR`). Without a folder the installed Windows
//! Terminal is used; `--terminal wezterm` drives WezTerm through its CLI
//! instead. Off Windows WezTerm is the terminal (on `PATH`, or on macOS
//! its app in `/Applications` or `~/Applications`).
//! `--from-shim`: started by a restored tab; exits quietly if NativeTerm
//! is already running.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod agent;
mod app;
mod app_host;
mod cleanup;
mod commands_import;
mod credential_sets;
mod dialog_window;
mod dialogs;
mod dock;
mod fab;
mod files_sync;
mod files_window;
mod find_window;
mod host_key_window;
mod icons;
mod import_dialog;
mod key_dialog;
mod layout;
mod log_page;
mod logon_page;
mod logos;
mod looks;
mod options_dialog;
mod part_window;
mod password_window;
mod plink_dialog;
mod properties;
mod quotation_window;
mod send_dialog;
mod send_line;
mod server_sessions;
mod shell;
mod shortcut_ui;
mod skinned;
#[cfg(test)]
mod snapshots;
mod storage;
mod tab_list;
mod tab_title_window;
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
    /// `--terminal …`: which terminal to drive instead of the platform's
    /// own.
    terminal: Option<Chosen>,
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
    let mut terminal = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().map(PathBuf::from).ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--terminal-dir" => terminal_dir = Some(value()?),
            "--ssh-dir" => ssh_dir = Some(value()?),
            "--data-dir" => data_dir = Some(value()?),
            "--from-shim" => from_shim = true,
            "--terminal" => {
                let which = value()?;
                let which = which.to_string_lossy();
                terminal = Some(match which.split_once('=') {
                    Some(("wezterm", dir)) => Chosen::WezTerm(Some(PathBuf::from(dir))),
                    None if which == "wezterm" => Chosen::WezTerm(None),
                    _ => return Err(format!("unknown terminal {which} (wezterm, wezterm=<folder>)")),
                });
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Options { terminal_dir, terminal, ssh_dir: ssh_dir.ok_or("the home folder is not known")?, data_dir, from_shim })
}

/// The folder `--terminal wezterm=<folder>` named last, for a start
/// without arguments (a desktop launcher).
const WEZTERM_DIR_SETTING: &str = "wezterm.dir";

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
    let same = |install: &&Install, want: &Path| {
        install.dir.as_os_str().eq_ignore_ascii_case(want.as_os_str()) || install.dir == want
    };
    if let Some(want) = chosen {
        if let Some(install) = found.iter().find(|i| same(i, want)) {
            return Ok(install.clone());
        }
        // a folder that was picked by hand (a portable copy) is not among
        // the installed packages, but it is still a Terminal, installed
        // ones or not
        if let Ok(install) = Install::from_dir(want) {
            return Ok(install);
        }
    }
    let first = found.first().ok_or_else(|| t!("fatal-no-terminal"))?;
    if let Some(want) = chosen {
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
    /// The Windows Terminal the profile and settings pages are about;
    /// none when the tabs go to WezTerm and no Terminal is installed
    #[cfg(windows)]
    install: Option<Install>,
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
    // another terminal than the platform's own: asked for, or the default
    // where there is none of the platform's own to drive — WezTerm on
    // Linux and macOS alike (on `PATH`, or its macOS app)
    let wezterm_at = |dir: Option<&std::path::Path>| native_term_wezterm::WezTerm::new(dir, &shim).available();
    let chosen = options.terminal.clone().or_else(|| {
        if cfg!(windows) {
            return None;
        }
        // the folder a `--terminal wezterm=<folder>` start named last time,
        // while it holds a WezTerm: a start from the desktop has no
        // arguments, and must not fall back to another WezTerm
        let remembered = settings
            .get(WEZTERM_DIR_SETTING)
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .filter(|dir| wezterm_at(Some(dir)));
        if let Some(dir) = remembered {
            return Some(Chosen::WezTerm(Some(dir)));
        }
        // one's own WezTerm before the distribution's on PATH
        native_term_wezterm::app_dirs()
            .into_iter()
            .find(|dir| wezterm_at(Some(dir)))
            .map(|dir| Chosen::WezTerm(Some(dir)))
            .or_else(|| wezterm_at(None).then_some(Chosen::WezTerm(None)))
    });
    if let Some(Chosen::WezTerm(Some(dir))) = &options.terminal {
        if let Err(e) = settings.set(WEZTERM_DIR_SETTING, &dir.to_string_lossy()) {
            diag::line(&format!("the WezTerm folder could not be remembered: {e}"));
        }
    }
    if let Some(Chosen::WezTerm(dir)) = &chosen {
        diag::line(&format!(
            "WezTerm from {}",
            dir.as_ref().map_or_else(|| "PATH".to_string(), |d| d.display().to_string())
        ));
    }
    if let Some(chosen) = chosen {
        let other_ssh_dir = Some(&options.ssh_dir) != default_ssh_dir().as_ref();
        let core = match chosen {
            Chosen::WezTerm(dir) => {
                // the windows look like the desktop from the start; the theme
                // and GPU settings are applied over it once the window is up
                native_term_os::appearance::refresh();
                let look = app::terminal_look(None, false, Default::default());
                let mut terminal =
                    native_term_wezterm::WezTerm::new(dir.as_deref(), &shim).with_config_dir(&data_dir, &look);
                if other_ssh_dir {
                    terminal = terminal.with_ssh_dir(&options.ssh_dir);
                }
                if !terminal.available() {
                    return Err(t!("notice-wezterm-missing"));
                }
                start_core(terminal, registry, options.from_shim, &mut notices)
            }
        };
        let core = match core {
            Ok(core) => core,
            Err(start) => return Ok(start),
        };
        if let Some(core) = &core {
            core.set_settings(settings.clone());
            if let Some(language) = core.language_setting() {
                native_term_app::i18n::set_language(Some(&language));
            }
        }
        return Ok(Start::Run(Box::new(Setup {
            options,
            _lock: lock,
            #[cfg(windows)]
            install: install_for_wezterm(&settings, &mut notices),
            shim,
            core,
            data_dir,
            data_source,
            notices,
        })));
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
    let core = match start_core(terminal, registry, options.from_shim, &mut notices) {
        Ok(core) => core,
        Err(start) => return Ok(start),
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
        install: Some(install),
        shim,
        core,
        data_dir,
        data_source,
        notices,
    })))
}

/// A terminal asked for with `--terminal`, instead of the platform's own.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Chosen {
    /// WezTerm, from this folder or from `PATH`.
    WezTerm(Option<PathBuf>),
}

/// The core on the pipe, or nothing (said in `notices`); `Err` when
/// another NativeTerm already serves it.
fn start_core(
    terminal: impl native_term_platform::TerminalBackend,
    registry: Option<Registry>,
    from_shim: bool,
    notices: &mut Vec<String>,
) -> Result<Option<Core>, Start> {
    match Core::start(terminal, registry) {
        Ok(core) => Ok(Some(core)),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => Err(Start::AlreadyRunning { quiet: from_shim }),
        Err(e) => {
            notices.push(t!("notice-no-pipe", error = e.to_string()));
            Ok(None)
        }
    }
}

/// The Windows Terminal install the settings and profile pages are about,
/// found the usual way, when the tabs go to WezTerm instead: none when no
/// Terminal is installed (Windows 10 has none of its own), which WezTerm
/// does not need.
#[cfg(windows)]
fn install_for_wezterm(settings: &settings::Settings, notices: &mut Vec<String>) -> Option<Install> {
    let chosen = settings.get(terminal_profile::INSTALL_SETTING).filter(|dir| !dir.is_empty()).map(PathBuf::from);
    choose_install(None, chosen.as_deref(), notices).ok()
}

fn main() {
    // the child that renders the GTK theme's title bar (see
    // native_term_os::titlebar): its answer on stdout, and done
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, dir, scheme] = args.as_slice() {
        if flag == "--desktop-titlebar" {
            std::process::exit(match native_term_os::titlebar::export(std::path::Path::new(dir), scheme == "dark") {
                Ok(answer) => {
                    print!("{answer}");
                    0
                }
                Err(err) => {
                    eprintln!("{err}");
                    1
                }
            });
        }
    }
    // Pantheon's Wayland (gala, elementary OS 8) shows no window of ours
    // and ends WezTerm's 2024 release at once; both work through
    // Xwayland, which is what removing the Wayland display picks
    #[cfg(all(unix, not(target_os = "macos")))]
    if std::env::var_os("XDG_CURRENT_DESKTOP").is_some_and(|d| d.to_string_lossy().contains("Pantheon")) {
        std::env::remove_var("WAYLAND_DISPLAY");
    }
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
        .with_inner_size([960.0, 640.0])
        .with_min_inner_size([560.0, 400.0]);
    let viewport = native_term_skin::undecorated(viewport);
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
        Ok(setup) => {
            // shared with the dialogs' windows (see app_host)
            let app = std::rc::Rc::new(std::cell::RefCell::new(App::new(ctx, setup)));
            app.borrow_mut().me = std::rc::Rc::downgrade(&app);
            Box::new(part_window::Shared(app))
        }
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

/// The design's faces, the same on every platform (SIL OFL 1.1, the texts
/// beside them in `assets/fonts`): Inter for the interface in the weights
/// it uses, JetBrains Mono for paths and numbers.
const BUNDLED_FONTS: [(&str, &[u8]); 4] = [
    ("Inter-Regular", include_bytes!("../assets/fonts/Inter-Regular.ttf")),
    ("Inter-Medium", include_bytes!("../assets/fonts/Inter-Medium.ttf")),
    ("Inter-SemiBold", include_bytes!("../assets/fonts/Inter-SemiBold.ttf")),
    ("JetBrainsMono-Regular", include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf")),
];

/// How far down (a fraction of the font size) a fallback face's glyphs go
/// for its baseline to be the first font's: egui puts the face's baseline
/// at its own ascent plus half the difference of the two line heights.
fn baseline_shift(first: &[u8], face: &[u8], index: u32) -> Option<f32> {
    use skrifa::MetadataProvider;
    let em = |bytes: &[u8], index: u32| {
        let font = skrifa::FontRef::from_index(bytes, index).ok()?;
        let m = font.metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
        let upem = f32::from(m.units_per_em.max(1));
        let (ascent, descent, gap) = (m.ascent / upem, m.descent / upem, m.leading / upem);
        Some((ascent, ascent - descent + gap))
    };
    let ((first_ascent, first_height), (face_ascent, face_height)) = (em(first, 0)?, em(face, index)?);
    Some(first_ascent - face_ascent - 0.5 * (first_height - face_height))
}

/// Inter and JetBrains Mono first, egui's own behind them (emoji), then
/// the system's CJK font (egui's have none) and the icons.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    use egui::FontFamily;
    use native_term_os::fonts;
    use native_term_skin::font::{MEDIUM, SEMIBOLD};
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in BUNDLED_FONTS {
        fonts.font_data.insert(name.into(), std::sync::Arc::new(egui::FontData::from_static(bytes)));
    }
    let behind = |family: FontFamily| fonts.families.get(&family).cloned().unwrap_or_default();
    let (proportional, monospace) = (behind(FontFamily::Proportional), behind(FontFamily::Monospace));
    let first = |face: &str, rest: &[String]| std::iter::once(face.to_string()).chain(rest.iter().cloned()).collect();
    let families = [
        (FontFamily::Proportional, first("Inter-Regular", &proportional)),
        (FontFamily::Monospace, first("JetBrainsMono-Regular", &monospace)),
        (FontFamily::Name(MEDIUM.into()), first("Inter-Medium", &proportional)),
        (FontFamily::Name(SEMIBOLD.into()), first("Inter-SemiBold", &proportional)),
    ];
    let names: Vec<FontFamily> = families.iter().map(|(family, _)| family.clone()).collect();
    fonts.families.extend(families);
    // the icons before the CJK font: their code points are the private
    // use area's, where a system's CJK font may have glyphs of its own
    // (macOS's PingFang drew "ǹ" for an icon)
    let glyphs = fonts::icon_file().and_then(|f| fonts::map_file(&f).ok()).map(|bytes| (bytes, 0));
    // mapped, not read: egui would keep two private copies of a 20 MB file
    let cjk = fonts::cjk_font().and_then(|(f, index)| Some((fonts::map_file(&f).ok()?, index)));
    let mut fallbacks = Vec::new();
    // a font that is there, under `name`
    let add = |fonts: &mut egui::FontDefinitions, name: &'static str, font: Option<(&'static [u8], u32)>| {
        let (bytes, index) = font?;
        let mut data = egui::FontData::from_static(bytes);
        data.index = index;
        fonts.font_data.insert(name.into(), std::sync::Arc::new(data));
        Some(name)
    };
    fallbacks.extend(add(&mut fonts, "icons", glyphs));
    // no icon font on the system: Phosphor, bundled (see `icons`)
    #[cfg(not(windows))]
    {
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        fonts.families.entry(FontFamily::Proportional).or_default().retain(|f| f != "phosphor");
        fallbacks.push("phosphor");
    }
    fallbacks.extend(add(&mut fonts, "cjk", cjk));
    // its ideographs on Inter's baseline: egui centers a fallback face's
    // line in the first font's (it doesn't line up the baselines), so a
    // face whose ascent and descent are shared out otherwise sits too high
    // or too low (PingFang: 0.26 em too high)
    if let (Some((bytes, index)), Some(data)) = (cjk, fonts.font_data.get_mut("cjk")) {
        if let Some(shift) = baseline_shift(BUNDLED_FONTS[0].1, bytes, index) {
            std::sync::Arc::make_mut(data).tweak.y_offset_factor = shift;
        }
    }
    for family in names {
        fonts.families.entry(family).or_default().extend(fallbacks.iter().map(|f| f.to_string()));
    }
    ctx.set_fonts(fonts);
}

struct Fatal(String);

impl window::Ui for Fatal {
    fn ui(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
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
