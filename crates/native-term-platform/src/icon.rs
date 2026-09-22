//! The icons NativeTerm draws, named by what they mean. Each platform
//! draws them with a font of its own: Segoe Fluent Icons (Segoe MDL2
//! Assets on Windows 10) on Windows, whose code points are here because
//! the Windows tab menu draws them itself; Phosphor elsewhere, mapped in
//! the app where that font is bundled.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Settings,
    Refresh,
    Add,
    Import,
    Folder,
    FolderOpen,
    ChevronRight,
    ChevronDown,
    Host,
    /// Non-SSH sessions: Telnet, raw, … (Ethernet).
    Network,
    Serial,
    Pin,
    /// An X: clear a field, close a tab.
    Clear,
    Search,
    Tab,
    Tabs,
    Open,
    Send,
    Key,
    Lock,
    Unlock,
    StarFilled,
    Connect,
    Save,
    Up,
    Upload,
    Download,
    NewFolder,
    Document,
    Link,
    Edit,
    Rename,
    Delete,
    Accept,
    Play,
    Pause,
    Sync,
    List,
    Grid,
    Disconnect,
    Clone,
    /// A serial line's break.
    Break,
    ClearScreen,
    CloseOthers,
    CloseEnded,
    CloseRight,
}

impl Icon {
    /// The glyph in Segoe Fluent Icons / Segoe MDL2 Assets (the same code
    /// points in both).
    #[must_use]
    pub const fn segoe(self) -> char {
        match self {
            Icon::Settings => '\u{E713}',
            Icon::Refresh => '\u{E72C}',
            Icon::Add => '\u{E710}',
            Icon::Import => '\u{E8B5}',
            Icon::Folder => '\u{E8B7}',
            Icon::FolderOpen => '\u{E838}',
            Icon::ChevronRight => '\u{E76C}',
            Icon::ChevronDown => '\u{E70D}',
            Icon::Host => '\u{E756}',
            Icon::Network => '\u{E839}',
            Icon::Serial => '\u{E88E}',
            Icon::Pin => '\u{E718}',
            Icon::Clear => '\u{E711}',
            Icon::Search => '\u{E721}',
            Icon::Tab => '\u{E7C3}',
            Icon::Tabs => '\u{E8A9}',
            Icon::Open => '\u{E8A7}',
            Icon::Send => '\u{E724}',
            Icon::Key => '\u{E8D7}',
            Icon::Lock => '\u{E72E}',
            Icon::Unlock => '\u{E785}',
            Icon::StarFilled => '\u{E735}',
            Icon::Connect => '\u{E703}',
            Icon::Save => '\u{E74E}',
            Icon::Up => '\u{E74A}',
            Icon::Upload => '\u{E898}',
            Icon::Download => '\u{E896}',
            Icon::NewFolder => '\u{E8F4}',
            Icon::Document => '\u{E8A5}',
            Icon::Link => '\u{E71B}',
            Icon::Edit => '\u{E70F}',
            Icon::Rename => '\u{E8AC}',
            Icon::Delete => '\u{E74D}',
            Icon::Accept => '\u{E73E}',
            Icon::Play => '\u{E768}',
            Icon::Pause => '\u{E769}',
            Icon::Sync => '\u{E895}',
            Icon::List => '\u{E8FD}',
            Icon::Grid => '\u{F0E2}',
            Icon::Disconnect => '\u{E8CD}',
            Icon::Clone => '\u{E8C8}',
            Icon::Break => '\u{E7BA}',
            Icon::ClearScreen => '\u{E75C}',
            Icon::CloseOthers => '\u{E8BB}',
            Icon::CloseEnded => '\u{E894}',
            Icon::CloseRight => '\u{E72A}',
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segoe_glyphs_are_private_use_code_points() {
        for icon in [Icon::Settings, Icon::Grid, Icon::CloseRight, Icon::Clear] {
            let c = icon.segoe() as u32;
            assert!((0xE000..=0xF8FF).contains(&c), "{icon:?}: {c:X}");
        }
        assert_eq!(Icon::Refresh.segoe(), '\u{E72C}');
    }
}
