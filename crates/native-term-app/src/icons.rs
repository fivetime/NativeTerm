//! Icon glyphs from Segoe Fluent Icons (Windows 11) or Segoe MDL2 Assets
//! (Windows 10); the same code points in both. The font is a fallback of
//! the normal text font, so a glyph can sit inside any label.

pub const SETTINGS: char = '\u{E713}';
pub const REFRESH: char = '\u{E72C}';
pub const ADD: char = '\u{E710}';
pub const IMPORT: char = '\u{E8B5}';
pub const FOLDER: char = '\u{E8B7}';
pub const FOLDER_OPEN: char = '\u{E838}';
pub const CHEVRON_RIGHT: char = '\u{E76C}';
pub const CHEVRON_DOWN: char = '\u{E70D}';
pub const HOST: char = '\u{E756}';
/// Non-SSH sessions: Telnet, raw, … (Ethernet) and serial (USB).
pub const NETWORK: char = '\u{E839}';
pub const SERIAL: char = '\u{E88E}';
pub const PIN: char = '\u{E718}';
pub const CLEAR: char = '\u{E711}';
pub const SEARCH: char = '\u{E721}';
pub const TAB: char = '\u{E7C3}';
pub const TABS: char = '\u{E8A9}';
pub const OPEN: char = '\u{E8A7}';
pub const SEND: char = '\u{E724}';
pub const KEY: char = '\u{E8D7}';
pub const LOCK: char = '\u{E72E}';
pub const UNLOCK: char = '\u{E785}';
pub const STAR_FILLED: char = '\u{E735}';
pub const CONNECT: char = '\u{E703}';
pub const SAVE: char = '\u{E74E}';
pub const UP: char = '\u{E74A}';
pub const UPLOAD: char = '\u{E898}';
pub const DOWNLOAD: char = '\u{E896}';
pub const NEW_FOLDER: char = '\u{E8F4}';
pub const DOCUMENT: char = '\u{E8A5}';
pub const LINK: char = '\u{E71B}';
pub const EDIT: char = '\u{E70F}';
pub const RENAME: char = '\u{E8AC}';
pub const DELETE: char = '\u{E74D}';
pub const ACCEPT: char = '\u{E73E}';
pub const PLAY: char = '\u{E768}';
pub const PAUSE: char = '\u{E769}';
pub const SYNC: char = '\u{E895}';
pub const LIST: char = '\u{E8FD}';
pub const GRID: char = '\u{F0E2}';

/// `glyph` then `text`, for buttons.
pub fn with(glyph: char, text: impl AsRef<str>) -> String {
    format!("{glyph}  {}", text.as_ref())
}

/// The icon font's file: Fluent on Windows 11, MDL2 otherwise.
pub fn font_file(fonts: &std::path::Path) -> Option<std::path::PathBuf> {
    ["SegoeIcons.ttf", "segmdl2.ttf"].iter().map(|f| fonts.join(f)).find(|p| p.exists())
}
