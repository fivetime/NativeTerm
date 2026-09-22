//! The icons as glyphs in the font `install_fonts` registered: Segoe
//! Fluent Icons (Segoe MDL2 Assets on Windows 10) on Windows, Phosphor
//! (bundled) elsewhere. The font is a fallback of the normal text font,
//! so a glyph can sit inside any label.

pub use native_term_platform::Icon;

/// The glyph for `icon` in the icon font of this platform.
#[must_use]
pub const fn glyph(icon: Icon) -> char {
    #[cfg(windows)]
    {
        icon.segoe()
    }
    #[cfg(not(windows))]
    {
        phosphor(icon)
    }
}

pub const SETTINGS: char = glyph(Icon::Settings);
pub const REFRESH: char = glyph(Icon::Refresh);
pub const ADD: char = glyph(Icon::Add);
pub const IMPORT: char = glyph(Icon::Import);
pub const FOLDER: char = glyph(Icon::Folder);
pub const FOLDER_OPEN: char = glyph(Icon::FolderOpen);
pub const CHEVRON_RIGHT: char = glyph(Icon::ChevronRight);
pub const CHEVRON_DOWN: char = glyph(Icon::ChevronDown);
pub const HOST: char = glyph(Icon::Host);
/// Non-SSH sessions: Telnet, raw, … (Ethernet) and serial (USB).
pub const NETWORK: char = glyph(Icon::Network);
pub const SERIAL: char = glyph(Icon::Serial);
pub const PIN: char = glyph(Icon::Pin);
pub const CLEAR: char = glyph(Icon::Clear);
pub const SEARCH: char = glyph(Icon::Search);
pub const TAB: char = glyph(Icon::Tab);
pub const TABS: char = glyph(Icon::Tabs);
pub const OPEN: char = glyph(Icon::Open);
pub const SEND: char = glyph(Icon::Send);
pub const KEY: char = glyph(Icon::Key);
pub const LOCK: char = glyph(Icon::Lock);
pub const UNLOCK: char = glyph(Icon::Unlock);
pub const STAR_FILLED: char = glyph(Icon::StarFilled);
pub const CONNECT: char = glyph(Icon::Connect);
pub const SAVE: char = glyph(Icon::Save);
pub const UP: char = glyph(Icon::Up);
pub const UPLOAD: char = glyph(Icon::Upload);
pub const DOWNLOAD: char = glyph(Icon::Download);
pub const NEW_FOLDER: char = glyph(Icon::NewFolder);
pub const DOCUMENT: char = glyph(Icon::Document);
pub const LINK: char = glyph(Icon::Link);
pub const EDIT: char = glyph(Icon::Edit);
pub const RENAME: char = glyph(Icon::Rename);
pub const DELETE: char = glyph(Icon::Delete);
pub const ACCEPT: char = glyph(Icon::Accept);
pub const PLAY: char = glyph(Icon::Play);
pub const PAUSE: char = glyph(Icon::Pause);
pub const SYNC: char = glyph(Icon::Sync);
pub const LIST: char = glyph(Icon::List);
pub const GRID: char = glyph(Icon::Grid);

/// `glyph` then `text`, for buttons.
pub fn with(glyph: char, text: impl AsRef<str>) -> String {
    format!("{glyph}  {}", text.as_ref())
}

/// The Phosphor (regular) glyph that means the same.
#[cfg(not(windows))]
const fn phosphor(icon: Icon) -> char {
    use egui_phosphor::regular as p;
    first_char(match icon {
        Icon::Settings => p::GEAR,
        Icon::Refresh => p::ARROWS_CLOCKWISE,
        Icon::Add => p::PLUS,
        Icon::Import => p::DOWNLOAD_SIMPLE,
        Icon::Folder => p::FOLDER,
        Icon::FolderOpen => p::FOLDER_OPEN,
        Icon::ChevronRight => p::CARET_RIGHT,
        Icon::ChevronDown => p::CARET_DOWN,
        Icon::Host => p::DESKTOP,
        Icon::Network => p::NETWORK,
        Icon::Serial => p::USB,
        Icon::Pin => p::PUSH_PIN,
        Icon::Clear => p::X,
        Icon::Search => p::MAGNIFYING_GLASS,
        Icon::Tab => p::BROWSER,
        Icon::Tabs => p::BROWSERS,
        Icon::Open => p::ARROW_SQUARE_OUT,
        Icon::Send => p::PAPER_PLANE_TILT,
        Icon::Key => p::KEY,
        Icon::Lock => p::LOCK,
        Icon::Unlock => p::LOCK_OPEN,
        Icon::StarFilled => p::STAR,
        Icon::Connect => p::PLUG,
        Icon::Save => p::FLOPPY_DISK,
        Icon::Up => p::ARROW_UP,
        Icon::Upload => p::UPLOAD_SIMPLE,
        Icon::Download => p::DOWNLOAD_SIMPLE,
        Icon::NewFolder => p::FOLDER_PLUS,
        Icon::Document => p::FILE,
        Icon::Link => p::LINK,
        Icon::Edit => p::PENCIL_SIMPLE,
        Icon::Rename => p::TEXTBOX,
        Icon::Delete => p::TRASH,
        Icon::Accept => p::CHECK,
        Icon::Play => p::PLAY,
        Icon::Pause => p::PAUSE,
        Icon::Sync => p::ARROWS_CLOCKWISE,
        Icon::List => p::LIST,
        Icon::Grid => p::SQUARES_FOUR,
        Icon::Disconnect => p::PLUGS,
        Icon::Clone => p::COPY,
        Icon::Break => p::LIGHTNING,
        Icon::ClearScreen => p::ERASER,
        Icon::CloseOthers => p::X_SQUARE,
        Icon::CloseEnded => p::X_CIRCLE,
        Icon::CloseRight => p::ARROW_LINE_RIGHT,
    })
}

/// The one character a Phosphor constant holds (a private-use code point,
/// three bytes of UTF-8).
#[cfg(not(windows))]
const fn first_char(s: &str) -> char {
    let b = s.as_bytes();
    let value = match b.len() {
        3 => ((b[0] as u32 & 0x0F) << 12) | ((b[1] as u32 & 0x3F) << 6) | (b[2] as u32 & 0x3F),
        _ => b[0] as u32,
    };
    match char::from_u32(value) {
        Some(c) => c,
        None => '\u{FFFD}',
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn glyphs_are_private_use_code_points() {
        for c in [super::SETTINGS, super::GRID, super::CLEAR, super::SYNC] {
            assert!((0xE000..=0xF8FF).contains(&(c as u32)), "{:X}", c as u32);
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn phosphor_constants_decode() {
        assert_eq!(
            super::first_char(egui_phosphor::regular::GEAR),
            egui_phosphor::regular::GEAR.chars().next().unwrap()
        );
        assert_eq!(super::first_char("a"), 'a');
    }
}
