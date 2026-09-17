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
pub const PIN: char = '\u{E718}';
pub const CLEAR: char = '\u{E711}';
pub const SEARCH: char = '\u{E721}';
pub const TAB: char = '\u{E7C3}';
pub const TABS: char = '\u{E8A9}';
pub const OPEN: char = '\u{E8A7}';
pub const CONNECT: char = '\u{E703}';
pub const SAVE: char = '\u{E74E}';

/// `glyph` then `text`, for buttons.
pub fn with(glyph: char, text: impl AsRef<str>) -> String {
    format!("{glyph}  {}", text.as_ref())
}

/// The icon font's file: Fluent on Windows 11, MDL2 otherwise.
pub fn font_file(fonts: &std::path::Path) -> Option<std::path::PathBuf> {
    ["SegoeIcons.ttf", "segmdl2.ttf"].iter().map(|f| fonts.join(f)).find(|p| p.exists())
}
