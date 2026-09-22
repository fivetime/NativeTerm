//! The shortcuts in the app (see `native_term_app::shortcuts`): kept in
//! `state.db` (`shortcuts`), the in-window ones caught each frame, the
//! global ones registered on the hotkey thread, and the settings section
//! that records a new combination by pressing it.

use std::path::Path;
use std::sync::mpsc::{self, Receiver};

use native_term_app::shortcuts::{self, Combo, Command, Problem, Shortcuts};
use native_term_app::{t, Core};
use native_term_os::hotkey::Hotkeys;

const SETTING: &str = "shortcuts";
const RED: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x9a, 0x1a);

pub struct ShortcutUi {
    shortcuts: Shortcuts,
    hotkeys: Option<Hotkeys>,
    pressed: Receiver<Command>,
    /// The shortcut being recorded: which command, and whether global.
    recording: Option<(Command, bool)>,
    /// Windows Terminal's bindings (its defaults and its settings.json's).
    terminal: Vec<(Combo, String)>,
}

impl ShortcutUi {
    pub fn new(ctx: &egui::Context, core: Option<&Core>, terminal_settings: &Path) -> ShortcutUi {
        let shortcuts = Shortcuts::load(core.and_then(|c| c.setting(SETTING)).as_deref());
        let (tx, pressed) = mpsc::channel();
        let wake = ctx.clone();
        // a press on the hotkey thread: its command, and a frame to run it
        let hotkeys = Hotkeys::start(move |id| {
            if let Some(command) = Command::ALL.get(id as usize) {
                let _ = tx.send(*command);
                wake.request_repaint();
            }
        })
        .ok();
        let mut terminal = shortcuts::terminal_defaults();
        if let Some(settings) = std::fs::read_to_string(terminal_settings)
            .ok()
            .and_then(|text| native_term_platform::jsonc::parse(&text).ok())
        {
            terminal.extend(shortcuts::terminal_bindings(&settings));
        }
        let ui = ShortcutUi { shortcuts, hotkeys, pressed, recording: None, terminal };
        ui.register();
        ui
    }

    /// The global shortcuts that can be, on the hotkey thread.
    fn register(&self) {
        let Some(hotkeys) = &self.hotkeys else { return };
        let problems = shortcuts::problems(&self.shortcuts, &self.terminal);
        let keys = Command::ALL
            .iter()
            .enumerate()
            .filter(|(_, c)| !matches!(problems.get(&(**c, true)), Some(Problem::NotForGlobal | Problem::Taken(_))))
            .filter_map(|(id, c)| {
                let (mods, vk) = self.shortcuts.get(*c).global.as_ref()?.hotkey()?;
                Some((id as i32, mods, vk))
            })
            .collect();
        hotkeys.set(keys);
    }

    fn changed(&mut self, core: Option<&Core>) {
        if let Some(core) = core {
            core.set_setting(SETTING, &self.shortcuts.save());
        }
        self.register();
    }

    /// The commands asked for since the last frame: global presses, and
    /// the window's own shortcuts (none while one is being recorded).
    pub fn take(&mut self, ctx: &egui::Context) -> Vec<(Command, bool)> {
        let mut out: Vec<(Command, bool)> = self.pressed.try_iter().map(|c| (c, true)).collect();
        if self.recording.is_some() {
            return out;
        }
        let problems = shortcuts::problems(&self.shortcuts, &self.terminal);
        for command in Command::ALL {
            if problems.contains_key(&(command, false)) {
                continue;
            }
            let Some(shortcut) = self.shortcuts.get(command).local.as_ref().and_then(Combo::to_egui) else { continue };
            if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
                out.push((command, false));
            }
        }
        out
    }

    /// The settings section: each command's shortcut in the window and
    /// global, recorded by pressing it, with what is wrong with it.
    pub fn settings_ui(&mut self, ui: &mut egui::Ui, core: Option<&Core>) {
        egui::CollapsingHeader::new(t!("keys-title")).id_salt("keys").show(ui, |ui| {
            ui.weak(t!("keys-intro"));
            if let Some((command, global)) = self.recording {
                self.record(ui, core, command, global);
            }
            let problems = shortcuts::problems(&self.shortcuts, &self.terminal);
            let registered = self.hotkeys.as_ref().map(Hotkeys::results).unwrap_or_default();
            egui::Grid::new("keys-grid").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.weak(t!("keys-command"));
                ui.weak(t!("keys-local"));
                ui.weak(t!("keys-global"));
                ui.end_row();
                for command in Command::ALL {
                    ui.label(command_name(command));
                    for global in [false, true] {
                        ui.vertical(|ui| {
                            self.slot(ui, core, command, global);
                            let problem = problems.get(&(command, global));
                            let failed = registered.get(&(command_index(command) as i32)).and_then(|r| r.clone().err());
                            match (problem, failed) {
                                (Some(p), _) => {
                                    let (text, color) = problem_text(p);
                                    ui.colored_label(color, text);
                                }
                                (None, Some(e)) if global => {
                                    ui.colored_label(RED, t!("keys-held", error = e.trim()));
                                }
                                _ => {}
                            }
                        });
                    }
                    ui.end_row();
                }
            });
            if ui.button(t!("keys-defaults")).clicked() {
                self.shortcuts = Shortcuts::default();
                self.recording = None;
                self.changed(core);
            }
        });
    }

    /// One shortcut's button (press to record) and its clear button.
    fn slot(&mut self, ui: &mut egui::Ui, core: Option<&Core>, command: Command, global: bool) {
        let current = {
            let keys = self.shortcuts.get(command);
            if global {
                keys.global.clone()
            } else {
                keys.local.clone()
            }
        };
        let recording = self.recording == Some((command, global));
        ui.horizontal(|ui| {
            let text = if recording {
                t!("keys-press")
            } else {
                current.as_ref().map_or_else(|| t!("keys-none"), Combo::label)
            };
            let button = ui.add(egui::Button::new(text).selected(recording).min_size(egui::vec2(150.0, 0.0)));
            if button.clicked() {
                self.recording = if recording { None } else { Some((command, global)) };
            }
            let clear = crate::icons::CLEAR.to_string();
            if current.is_some() && ui.small_button(clear).on_hover_text(t!("keys-clear")).clicked() {
                self.shortcuts.set(command, global, None);
                self.recording = None;
                self.changed(core);
            }
        });
    }

    /// The next key pressed (with its modifiers) becomes the shortcut;
    /// Esc cancels.
    fn record(&mut self, ui: &mut egui::Ui, core: Option<&Core>, command: Command, global: bool) {
        let pressed = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            })
        });
        let Some((key, modifiers)) = pressed else { return };
        self.recording = None;
        if key == egui::Key::Escape && modifiers.is_none() {
            return;
        }
        if let Some(combo) = Combo::from_egui(key, modifiers) {
            self.shortcuts.set(command, global, Some(combo));
            self.changed(core);
        }
    }
}

fn command_index(command: Command) -> usize {
    Command::ALL.iter().position(|c| *c == command).unwrap_or(0)
}

fn command_name(command: Command) -> String {
    match command {
        Command::ShowNativeTerm => t!("keys-cmd-show"),
        Command::SearchHosts => t!("keys-cmd-search"),
        Command::AllTabs => t!("keys-cmd-tabs"),
        Command::SendToActive => t!("keys-cmd-send-active"),
        Command::SendToSeveral => t!("keys-cmd-send-several"),
        Command::FilesForActive => t!("keys-cmd-files-active"),
    }
}

fn problem_text(problem: &Problem) -> (String, egui::Color32) {
    match problem {
        Problem::Taken(other) => (t!("keys-taken", command = command_name(*other)), RED),
        Problem::NotForGlobal => (t!("keys-not-global"), RED),
        Problem::NeedsModifier => (t!("keys-needs-modifier"), RED),
        Problem::Terminal(action) => (t!("keys-terminal", action = action.as_str()), AMBER),
    }
}
