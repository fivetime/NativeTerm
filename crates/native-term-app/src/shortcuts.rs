//! Keyboard shortcuts for NativeTerm's commands: each command may have a
//! shortcut in NativeTerm's window and a global one (`RegisterHotKey`,
//! working while another program, usually Windows Terminal, is in front).
//! Defaults are few (Ctrl+F, Ctrl+T in the window) and no global one is
//! on; the file window's and text fields' keys (F5, F2, Delete, Enter…)
//! stay as they are.
//!
//! Key combinations are written the way Windows Terminal writes them
//! (`ctrl+alt+f`, `ctrl+shift+comma`), so they compare with its bindings
//! as they are: a global shortcut Windows Terminal also uses would take the
//! key from it, and is pointed out.

use std::collections::HashMap;
use std::fmt;

/// What a shortcut does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Command {
    /// Bring NativeTerm's window out, the host search focused.
    ShowNativeTerm,
    /// The host search (in the window).
    SearchHosts,
    /// "All tabs", its search focused.
    AllTabs,
    /// The send dialog for the active session (the tab in front).
    SendToActive,
    /// The send dialog for several sessions.
    SendToSeveral,
    /// Files (SFTP) for the active session.
    FilesForActive,
}

impl Command {
    pub const ALL: [Command; 6] = [
        Command::ShowNativeTerm,
        Command::SearchHosts,
        Command::AllTabs,
        Command::SendToActive,
        Command::SendToSeveral,
        Command::FilesForActive,
    ];

    /// The name kept in the settings.
    pub fn key(self) -> &'static str {
        match self {
            Command::ShowNativeTerm => "show",
            Command::SearchHosts => "search",
            Command::AllTabs => "tabs",
            Command::SendToActive => "send-active",
            Command::SendToSeveral => "send-several",
            Command::FilesForActive => "files-active",
        }
    }

    fn from_key(key: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.key() == key)
    }
}

/// A key combination: modifiers and one key, by Windows Terminal's names.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Combo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// `a`…`z`, `0`…`9`, `f1`…`f24`, `comma`, `up`, `pageup`, …
    pub key: String,
}

impl fmt::Display for Combo {
    /// `ctrl+alt+shift+key`, Windows Terminal's order.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [(self.ctrl, "ctrl+"), (self.alt, "alt+"), (self.shift, "shift+")] {
            if on {
                f.write_str(name)?;
            }
        }
        f.write_str(&self.key)
    }
}

impl Combo {
    /// `ctrl+alt+f`, any order and case; `None` for anything else (no
    /// key, two keys, a key name that isn't one, `win+`).
    pub fn parse(text: &str) -> Option<Combo> {
        let mut combo = Combo { ctrl: false, alt: false, shift: false, key: String::new() };
        for part in text.trim().to_ascii_lowercase().split('+') {
            match part.trim() {
                "ctrl" => combo.ctrl = true,
                "alt" => combo.alt = true,
                "shift" => combo.shift = true,
                key if vk(key).is_some() && combo.key.is_empty() => combo.key = key.to_string(),
                _ => return None,
            }
        }
        (!combo.key.is_empty()).then_some(combo)
    }

    /// As shown to the user: `Ctrl+Alt+F`.
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        for (on, name) in [(self.ctrl, "Ctrl"), (self.alt, "Alt"), (self.shift, "Shift")] {
            if on {
                parts.push(name.to_string());
            }
        }
        parts.push(match self.key.as_str() {
            k if k.len() == 1 => k.to_ascii_uppercase(),
            k if k.starts_with('f') && k[1..].parse::<u8>().is_ok() => k.to_ascii_uppercase(),
            "pageup" => "PgUp".into(),
            "pagedown" => "PgDn".into(),
            k => {
                let mut c = k.chars();
                c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
            }
        });
        parts.join("+")
    }

    /// From a key press in NativeTerm's window; `None` for a key no
    /// shortcut can use.
    pub fn from_egui(key: egui::Key, modifiers: egui::Modifiers) -> Option<Combo> {
        let key = key_name(key)?;
        Some(Combo { ctrl: modifiers.ctrl || modifiers.command, alt: modifiers.alt, shift: modifiers.shift, key })
    }

    /// The same combination for egui (to catch it in the window).
    pub fn to_egui(&self) -> Option<egui::KeyboardShortcut> {
        let key = egui_key(&self.key)?;
        let modifiers =
            egui::Modifiers { alt: self.alt, ctrl: self.ctrl, shift: self.shift, mac_cmd: false, command: self.ctrl };
        Some(egui::KeyboardShortcut::new(modifiers, key))
    }

    /// `RegisterHotKey`'s modifiers (`MOD_ALT` 1, `MOD_CONTROL` 2,
    /// `MOD_SHIFT` 4, `MOD_NOREPEAT` 0x4000) and virtual key.
    pub fn hotkey(&self) -> Option<(u32, u32)> {
        let mods = u32::from(self.alt) | u32::from(self.ctrl) << 1 | u32::from(self.shift) << 2 | 0x4000;
        Some((mods, vk(&self.key)?))
    }
}

/// Windows' virtual key for a key name.
fn vk(key: &str) -> Option<u32> {
    let named = match key {
        "backspace" => 0x08,
        "tab" => 0x09,
        "enter" => 0x0D,
        "pause" => 0x13,
        "esc" | "escape" => 0x1B,
        "space" => 0x20,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" => 0x2D,
        "delete" => 0x2E,
        "semicolon" => 0xBA,
        "plus" => 0xBB,
        "comma" => 0xBC,
        "minus" => 0xBD,
        "period" => 0xBE,
        "slash" => 0xBF,
        "backtick" => 0xC0,
        "openbracket" => 0xDB,
        "backslash" => 0xDC,
        "closebracket" => 0xDD,
        "quote" => 0xDE,
        _ => 0,
    };
    if named != 0 {
        return Some(named);
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c @ ('a'..='z' | '0'..='9')), None) => Some(c.to_ascii_uppercase() as u32),
        (Some('f'), Some(_)) => match key[1..].parse::<u32>() {
            Ok(n @ 1..=24) => Some(0x6F + n),
            _ => None,
        },
        _ => None,
    }
}

/// egui's key → Windows Terminal's name.
fn key_name(key: egui::Key) -> Option<String> {
    use egui::Key;
    let name = match key {
        Key::Backspace => "backspace",
        Key::Tab => "tab",
        Key::Enter => "enter",
        Key::Escape => "esc",
        Key::Space => "space",
        Key::PageUp => "pageup",
        Key::PageDown => "pagedown",
        Key::End => "end",
        Key::Home => "home",
        Key::ArrowLeft => "left",
        Key::ArrowUp => "up",
        Key::ArrowRight => "right",
        Key::ArrowDown => "down",
        Key::Insert => "insert",
        Key::Delete => "delete",
        Key::Semicolon => "semicolon",
        Key::Plus | Key::Equals => "plus",
        Key::Comma => "comma",
        Key::Minus => "minus",
        Key::Period => "period",
        Key::Slash => "slash",
        Key::Backtick => "backtick",
        Key::OpenBracket => "openbracket",
        Key::Backslash => "backslash",
        Key::CloseBracket => "closebracket",
        Key::Quote => "quote",
        other => {
            let n = other.name().to_ascii_lowercase();
            return (vk(&n).is_some()).then_some(n);
        }
    };
    Some(name.to_string())
}

/// Windows Terminal's name → egui's key.
fn egui_key(name: &str) -> Option<egui::Key> {
    use egui::Key;
    Some(match name {
        "backspace" => Key::Backspace,
        "tab" => Key::Tab,
        "enter" => Key::Enter,
        "esc" | "escape" => Key::Escape,
        "space" => Key::Space,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "end" => Key::End,
        "home" => Key::Home,
        "left" => Key::ArrowLeft,
        "up" => Key::ArrowUp,
        "right" => Key::ArrowRight,
        "down" => Key::ArrowDown,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        "semicolon" => Key::Semicolon,
        "plus" => Key::Plus,
        "comma" => Key::Comma,
        "minus" => Key::Minus,
        "period" => Key::Period,
        "slash" => Key::Slash,
        "backtick" => Key::Backtick,
        "openbracket" => Key::OpenBracket,
        "backslash" => Key::Backslash,
        "closebracket" => Key::CloseBracket,
        "quote" => Key::Quote,
        other => Key::from_name(&other.to_ascii_uppercase())?,
    })
}

/// A command's shortcuts: in NativeTerm's window, and global.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keys {
    pub local: Option<Combo>,
    pub global: Option<Combo>,
}

/// Every command's shortcuts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcuts(pub HashMap<Command, Keys>);

impl Default for Shortcuts {
    /// Few: the window's Ctrl+F and Ctrl+T; nothing global.
    fn default() -> Shortcuts {
        let mut map: HashMap<Command, Keys> = Command::ALL.into_iter().map(|c| (c, Keys::default())).collect();
        map.insert(Command::SearchHosts, Keys { local: Combo::parse("ctrl+f"), global: None });
        map.insert(Command::AllTabs, Keys { local: Combo::parse("ctrl+t"), global: None });
        Shortcuts(map)
    }
}

impl Shortcuts {
    pub fn get(&self, command: Command) -> &Keys {
        static NONE: Keys = Keys { local: None, global: None };
        self.0.get(&command).unwrap_or(&NONE)
    }

    pub fn set(&mut self, command: Command, global: bool, combo: Option<Combo>) {
        let keys = self.0.entry(command).or_default();
        if global {
            keys.global = combo;
        } else {
            keys.local = combo;
        }
    }

    /// The setting's text: a line per command that differs from the
    /// defaults, `command local global` (`-` for none), so a new default
    /// later reaches those who never changed it.
    pub fn save(&self) -> String {
        let defaults = Shortcuts::default();
        let text = |c: &Option<Combo>| c.as_ref().map_or("-".to_string(), Combo::to_string);
        Command::ALL
            .into_iter()
            .filter(|c| self.get(*c) != defaults.get(*c))
            .map(|c| format!("{} {} {}\n", c.key(), text(&self.get(c).local), text(&self.get(c).global)))
            .collect()
    }

    /// The defaults with what `text` changes (lines it can't read are
    /// left out).
    pub fn load(text: Option<&str>) -> Shortcuts {
        let mut shortcuts = Shortcuts::default();
        for line in text.unwrap_or("").lines() {
            let mut parts = line.split_whitespace();
            let (Some(command), Some(local), Some(global)) = (parts.next(), parts.next(), parts.next()) else {
                continue;
            };
            let Some(command) = Command::from_key(command) else { continue };
            let read = |t: &str| if t == "-" { Some(None) } else { Combo::parse(t).map(Some) };
            if let (Some(local), Some(global)) = (read(local), read(global)) {
                shortcuts.0.insert(command, Keys { local, global });
            }
        }
        shortcuts
    }
}

/// What is wrong with a command's shortcut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Another command has the same one (in the same place).
    Taken(Command),
    /// A global shortcut needs Ctrl or Alt (a plain key or Shift+key
    /// would be taken from every program), and a few belong to Windows.
    NotForGlobal,
    /// Windows Terminal uses it (for the action named).
    Terminal(String),
    /// In the window, a key without Ctrl or Alt (other than F1…F24) would
    /// fire while typing in a text field.
    NeedsModifier,
}

impl Combo {
    /// F1…F24.
    fn is_function_key(&self) -> bool {
        self.key.starts_with('f') && self.key.len() > 1 && self.key[1..].parse::<u8>().is_ok()
    }
}

/// Combinations Windows itself keeps.
const WINDOWS_KEYS: [&str; 5] = ["alt+f4", "alt+tab", "alt+esc", "alt+space", "ctrl+esc"];

/// Windows Terminal's default key bindings (1.2x), the ones with a
/// modifier.
pub const TERMINAL_DEFAULTS: &[(&str, &str)] = &[
    ("alt+enter", "toggleFullscreen"),
    ("alt+shift+d", "splitPane duplicate"),
    ("alt+shift+minus", "splitPane down"),
    ("alt+shift+plus", "splitPane right"),
    ("alt+left", "moveFocus left"),
    ("alt+right", "moveFocus right"),
    ("alt+up", "moveFocus up"),
    ("alt+down", "moveFocus down"),
    ("alt+shift+left", "resizePane left"),
    ("alt+shift+right", "resizePane right"),
    ("alt+shift+up", "resizePane up"),
    ("alt+shift+down", "resizePane down"),
    ("ctrl+shift+space", "openNewTabDropdown"),
    ("ctrl+shift+t", "newTab"),
    ("ctrl+shift+n", "newWindow"),
    ("ctrl+shift+d", "duplicateTab"),
    ("ctrl+shift+w", "closePane"),
    ("ctrl+shift+f", "find"),
    ("ctrl+shift+p", "commandPalette"),
    ("ctrl+shift+a", "selectAll"),
    ("ctrl+shift+m", "markMode"),
    ("ctrl+shift+c", "copy"),
    ("ctrl+shift+v", "paste"),
    ("ctrl+c", "copy"),
    ("ctrl+v", "paste"),
    ("ctrl+insert", "copy"),
    ("shift+insert", "paste"),
    ("ctrl+tab", "nextTab"),
    ("ctrl+shift+tab", "prevTab"),
    ("ctrl+comma", "openSettings"),
    ("ctrl+shift+comma", "openSettings (file)"),
    ("ctrl+alt+comma", "openSettings (defaults)"),
    ("ctrl+plus", "adjustFontSize"),
    ("ctrl+minus", "adjustFontSize"),
    ("ctrl+0", "resetFontSize"),
    ("ctrl+shift+up", "scrollUp"),
    ("ctrl+shift+down", "scrollDown"),
    ("ctrl+shift+pageup", "scrollUpPage"),
    ("ctrl+shift+pagedown", "scrollDownPage"),
    ("ctrl+shift+home", "scrollToTop"),
    ("ctrl+shift+end", "scrollToBottom"),
    ("ctrl+shift+1", "newTab 1"),
    ("ctrl+shift+2", "newTab 2"),
    ("ctrl+shift+3", "newTab 3"),
    ("ctrl+shift+4", "newTab 4"),
    ("ctrl+shift+5", "newTab 5"),
    ("ctrl+shift+6", "newTab 6"),
    ("ctrl+shift+7", "newTab 7"),
    ("ctrl+shift+8", "newTab 8"),
    ("ctrl+shift+9", "newTab 9"),
    ("ctrl+alt+1", "switchToTab 1"),
    ("ctrl+alt+2", "switchToTab 2"),
    ("ctrl+alt+3", "switchToTab 3"),
    ("ctrl+alt+4", "switchToTab 4"),
    ("ctrl+alt+5", "switchToTab 5"),
    ("ctrl+alt+6", "switchToTab 6"),
    ("ctrl+alt+7", "switchToTab 7"),
    ("ctrl+alt+8", "switchToTab 8"),
    ("ctrl+alt+9", "switchToTab 9"),
];

/// The key bindings in a Windows Terminal `settings.json` (parsed): every
/// `keys` in `actions` and `keybindings` (both formats: a key on the
/// action, or bindings to an action's `id`), with what it runs.
pub fn terminal_bindings(settings: &serde_json::Value) -> Vec<(Combo, String)> {
    let mut out = Vec::new();
    for list in ["actions", "keybindings"] {
        for item in settings.get(list).and_then(|v| v.as_array()).into_iter().flatten() {
            let what = match item.get("command") {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(c) => c.get("action").and_then(|a| a.as_str()).unwrap_or("command").to_string(),
                None => item.get("id").and_then(|i| i.as_str()).unwrap_or("command").to_string(),
            };
            let keys: Vec<&str> = match item.get("keys") {
                Some(serde_json::Value::String(k)) => vec![k.as_str()],
                Some(serde_json::Value::Array(ks)) => ks.iter().filter_map(|k| k.as_str()).collect(),
                _ => Vec::new(),
            };
            out.extend(keys.into_iter().filter_map(Combo::parse).map(|c| (c, what.clone())));
        }
    }
    out
}

/// What is wrong with each command's shortcuts, in the window and
/// global; `terminal`: Windows Terminal's bindings (its defaults and its
/// `settings.json`'s), which only a global shortcut can take from it.
pub fn problems(shortcuts: &Shortcuts, terminal: &[(Combo, String)]) -> HashMap<(Command, bool), Problem> {
    let mut out = HashMap::new();
    for global in [false, true] {
        let mut seen: HashMap<Combo, Command> = HashMap::new();
        for command in Command::ALL {
            let keys = shortcuts.get(command);
            let Some(combo) = (if global { &keys.global } else { &keys.local }) else { continue };
            if let Some(first) = seen.get(combo) {
                out.insert((command, global), Problem::Taken(*first));
                continue;
            }
            seen.insert(combo.clone(), command);
            if !global && !(combo.ctrl || combo.alt || combo.is_function_key()) {
                out.insert((command, false), Problem::NeedsModifier);
            }
            if global {
                if !(combo.ctrl || combo.alt) || WINDOWS_KEYS.contains(&combo.to_string().as_str()) {
                    out.insert((command, true), Problem::NotForGlobal);
                } else if let Some((_, action)) = terminal.iter().find(|(c, _)| c == combo) {
                    out.insert((command, true), Problem::Terminal(action.clone()));
                }
            }
        }
    }
    out
}

/// Windows Terminal's defaults as combinations.
pub fn terminal_defaults() -> Vec<(Combo, String)> {
    TERMINAL_DEFAULTS.iter().filter_map(|(k, a)| Some((Combo::parse(k)?, a.to_string()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combinations_read_and_written() {
        let c = Combo::parse("Shift+CTRL+comma").unwrap();
        assert_eq!(c.to_string(), "ctrl+shift+comma", "Windows Terminal's order");
        assert_eq!(c.label(), "Ctrl+Shift+Comma");
        assert_eq!(Combo::parse("ctrl+alt+f").unwrap().label(), "Ctrl+Alt+F");
        assert_eq!(Combo::parse("alt+f11").unwrap().label(), "Alt+F11");
        for bad in ["", "ctrl", "ctrl+a+b", "win+a", "ctrl+f25", "ctrl+ü"] {
            assert_eq!(Combo::parse(bad), None, "{bad}");
        }
        assert_eq!(Combo::parse("ctrl+alt+f").unwrap().hotkey(), Some((0x4003, 0x46)));
        assert_eq!(Combo::parse("shift+f12").unwrap().hotkey(), Some((0x4004, 0x7B)));
        assert_eq!(Combo::parse("ctrl+pageup").unwrap().hotkey(), Some((0x4002, 0x21)));
    }

    #[test]
    fn egui_keys_both_ways() {
        let m = egui::Modifiers { ctrl: true, command: true, alt: true, ..Default::default() };
        let c = Combo::from_egui(egui::Key::F, m).unwrap();
        assert_eq!(c.to_string(), "ctrl+alt+f");
        assert_eq!(c.to_egui().unwrap().logical_key, egui::Key::F);
        assert_eq!(Combo::from_egui(egui::Key::Comma, egui::Modifiers::CTRL).unwrap().to_string(), "ctrl+comma");
        assert_eq!(Combo::from_egui(egui::Key::F5, egui::Modifiers::NONE).unwrap().to_string(), "f5");
        for key in ["a", "9", "f24", "comma", "pageup", "left", "backtick", "delete"] {
            let c = Combo::parse(key).unwrap();
            assert_eq!(Combo::from_egui(c.to_egui().unwrap().logical_key, egui::Modifiers::NONE), Some(c), "{key}");
        }
    }

    #[test]
    fn saved_as_changes_from_the_defaults() {
        let mut s = Shortcuts::default();
        assert_eq!(s.save(), "", "the defaults save as nothing");
        s.set(Command::FilesForActive, true, Combo::parse("ctrl+alt+f"));
        s.set(Command::SearchHosts, false, None);
        let text = s.save();
        assert_eq!(text, "search - -\nfiles-active - ctrl+alt+f\n");
        assert_eq!(Shortcuts::load(Some(&text)), s);
        let loaded = Shortcuts::load(Some("tabs ctrl+alt+t -\nnonsense x y\nsearch ctrl+nope -\n"));
        assert_eq!(loaded.get(Command::AllTabs).local, Combo::parse("ctrl+alt+t"));
        assert_eq!(loaded.get(Command::SearchHosts).local, Combo::parse("ctrl+f"), "a bad line is left out");
    }

    #[test]
    fn the_terminals_bindings_from_its_settings() {
        let settings: serde_json::Value = serde_json::from_str(
            r#"{
                "actions": [
                    { "command": "find", "keys": "ctrl+alt+f" },
                    { "command": { "action": "splitPane", "split": "auto" }, "keys": ["alt+shift+s", "nonsense+"] },
                    { "command": "closeTab", "id": "User.closeTab" }
                ],
                "keybindings": [ { "id": "User.closeTab", "keys": "ctrl+alt+w" } ]
            }"#,
        )
        .unwrap();
        let bound = terminal_bindings(&settings);
        let text: Vec<(String, String)> = bound.iter().map(|(c, a)| (c.to_string(), a.clone())).collect();
        assert_eq!(
            text,
            [
                ("ctrl+alt+f".to_string(), "find".to_string()),
                ("alt+shift+s".into(), "splitPane".into()),
                ("ctrl+alt+w".into(), "User.closeTab".into()),
            ]
        );
    }

    #[test]
    fn problems_found() {
        let mut s = Shortcuts::default();
        s.set(Command::AllTabs, false, Combo::parse("ctrl+f"));
        s.set(Command::FilesForActive, true, Combo::parse("ctrl+shift+d"));
        s.set(Command::SendToActive, true, Combo::parse("shift+f1"));
        s.set(Command::ShowNativeTerm, true, Combo::parse("alt+space"));
        s.set(Command::SendToSeveral, true, Combo::parse("ctrl+alt+s"));
        s.set(Command::SendToSeveral, false, Combo::parse("shift+s"));
        s.set(Command::SendToActive, false, Combo::parse("f9"));
        let p = problems(&s, &terminal_defaults());
        assert_eq!(p.get(&(Command::SendToSeveral, false)), Some(&Problem::NeedsModifier), "typed in fields");
        assert_eq!(p.get(&(Command::SendToActive, false)), None, "a function key alone is fine");
        assert_eq!(p.get(&(Command::AllTabs, false)), Some(&Problem::Taken(Command::SearchHosts)));
        assert_eq!(p.get(&(Command::FilesForActive, true)), Some(&Problem::Terminal("duplicateTab".into())));
        assert_eq!(p.get(&(Command::SendToActive, true)), Some(&Problem::NotForGlobal), "no Ctrl or Alt");
        assert_eq!(p.get(&(Command::ShowNativeTerm, true)), Some(&Problem::NotForGlobal), "Windows' own");
        assert_eq!(p.get(&(Command::SendToSeveral, true)), None);
        assert_eq!(p.len(), 5);
    }
}
