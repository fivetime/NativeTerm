//! The desktop's own window-button icons, found the way GTK finds them
//! for its apps (Chrome's title bar included): the icon theme the
//! desktop names, then the themes it inherits, then `hicolor` and
//! `Adwaita`; the freedesktop names (`window-close-symbolic`, …), so
//! every desktop that follows the icon theme specification gets its own
//! icons with no code of ours for it. Linux only; elsewhere nothing.
//!
//! The theme's name comes from XSETTINGS (`Net/IconThemeName`, what GTK
//! reads under X11, and what every desktop's settings daemon publishes
//! for XWayland too), then KDE's `kdeglobals`, GNOME's gsettings, GTK's
//! `settings.ini`.

use std::path::PathBuf;

/// The icon files for the window buttons (SVG), each `None` when the
/// themes have none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowIcons {
    pub close: Option<PathBuf>,
    pub minimize: Option<PathBuf>,
    pub maximize: Option<PathBuf>,
    pub restore: Option<PathBuf>,
}

impl WindowIcons {
    /// The icons found, keyed as WezTerm's `integrated_title_button_icons`.
    pub fn named(&self) -> Vec<(&'static str, PathBuf)> {
        [("close", &self.close), ("minimize", &self.minimize), ("maximize", &self.maximize), ("restore", &self.restore)]
            .into_iter()
            .filter_map(|(name, path)| Some((name, path.clone()?)))
            .collect()
    }
}

/// The window-button icons of the theme named `theme` (see
/// `appearance::Appearance::icon_theme`).
#[cfg(all(unix, not(target_os = "macos")))]
pub fn window_icons(theme: Option<&str>) -> WindowIcons {
    let bases = search_bases();
    let chain = theme_chain(theme.unwrap_or("hicolor"), &bases);
    let find = |name: &str| lookup(name, &chain, &bases);
    WindowIcons {
        close: find("window-close"),
        minimize: find("window-minimize"),
        maximize: find("window-maximize"),
        restore: find("window-restore"),
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
pub fn window_icons(_theme: Option<&str>) -> WindowIcons {
    WindowIcons::default()
}

/// Where icon themes live, in the specification's order: the user's
/// `~/.local/share/icons` and `~/.icons`, then `$XDG_DATA_DIRS/icons`.
#[cfg(all(unix, not(target_os = "macos")))]
fn search_bases() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home =
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let mut bases: Vec<PathBuf> = data_home.map(|d| d.join("icons")).into_iter().collect();
    bases.extend(home.map(|h| h.join(".icons")));
    bases.extend(data_dirs.split(':').filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("icons")));
    bases
}

/// A theme's `index.theme`, from the first base that has one.
#[cfg(all(unix, not(target_os = "macos")))]
fn index_of(theme: &str, bases: &[PathBuf]) -> Option<parse::Index> {
    bases
        .iter()
        .find_map(|base| std::fs::read_to_string(base.join(theme).join("index.theme")).ok())
        .map(|t| parse::index(&t))
}

/// The theme and every theme it inherits, depth first, then `hicolor`
/// (every theme's last resort) and `Adwaita` (GTK's own), each once.
#[cfg(all(unix, not(target_os = "macos")))]
fn theme_chain(theme: &str, bases: &[PathBuf]) -> Vec<(String, parse::Index)> {
    fn walk(theme: &str, bases: &[PathBuf], out: &mut Vec<(String, parse::Index)>) {
        if out.iter().any(|(name, _)| name == theme) {
            return;
        }
        let Some(index) = index_of(theme, bases) else { return };
        let inherits = index.inherits.clone();
        out.push((theme.to_string(), index));
        for parent in inherits {
            walk(&parent, bases, out);
        }
    }
    let mut chain = Vec::new();
    for theme in [theme, "hicolor", "Adwaita"] {
        walk(theme, bases, &mut chain);
    }
    chain
}

/// `name`'s icon as an SVG file: the symbolic one through the whole
/// chain first (the look GTK asks for in title bars), then the plain
/// one; in each theme the directory nearest 16 pixels.
#[cfg(all(unix, not(target_os = "macos")))]
fn lookup(name: &str, chain: &[(String, parse::Index)], bases: &[PathBuf]) -> Option<PathBuf> {
    for file in [format!("{name}-symbolic.svg"), format!("{name}.svg")] {
        for (theme, index) in chain {
            for dir in index.by_nearness(16) {
                if let Some(path) =
                    bases.iter().map(|base| base.join(theme).join(&dir.path).join(&file)).find(|p| p.is_file())
                {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// String settings from XSETTINGS (`Net/IconThemeName`,
/// `Net/ThemeName`, …), each `None` when unset or empty: the manager's
/// selection owner holds them in `_XSETTINGS_SETTINGS`.
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn xsettings(names: &[&str]) -> Vec<Option<String>> {
    let data = xsettings_data();
    names
        .iter()
        .map(|name| data.as_deref().and_then(|d| parse::xsettings_string(d, name)).filter(|v| !v.is_empty()))
        .collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn xsettings_data() -> Option<Vec<u8>> {
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    let (conn, screen) = x11rb::connect(None).ok()?;
    let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom);
    let selection = atom(&format!("_XSETTINGS_S{screen}"))?;
    let settings = atom("_XSETTINGS_SETTINGS")?;
    let owner = conn.get_selection_owner(selection).ok()?.reply().ok()?.owner;
    if owner == x11rb::NONE {
        return None;
    }
    let reply = conn.get_property(false, owner, settings, AtomEnum::ANY, 0, u32::MAX / 4).ok()?.reply().ok()?;
    Some(reply.value)
}

#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
pub(crate) mod parse {
    /// One directory of an icon theme, as `index.theme` describes it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Dir {
        pub path: String,
        pub size: u32,
        pub scale: u32,
        pub kind: String,
        pub min: u32,
        pub max: u32,
        pub threshold: u32,
    }

    /// An icon theme's `index.theme`: its directories and the themes it
    /// inherits.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Index {
        pub dirs: Vec<Dir>,
        pub inherits: Vec<String>,
    }

    impl Dir {
        /// How far the directory is from `size` pixels, the
        /// specification's DirectorySizeDistance (0 when it fits).
        fn distance(&self, size: u32) -> u32 {
            let size = size * self.scale.max(1);
            let (low, high) = match self.kind.as_str() {
                "Fixed" => (self.size, self.size),
                "Scalable" => (self.min, self.max),
                _ => (self.size.saturating_sub(self.threshold), self.size + self.threshold),
            };
            let (low, high) = (low * self.scale.max(1), high * self.scale.max(1));
            if size < low {
                low - size
            } else {
                size.saturating_sub(high)
            }
        }
    }

    impl Index {
        /// The directories nearest `size` pixels first (scale 1 before
        /// larger scales); among those that fit alike (scalable ones all
        /// fit), the one drawn for the nearest size — elementary's
        /// `actions/24` stretches down to 8 but was drawn for 24.
        pub fn by_nearness(&self, size: u32) -> Vec<&Dir> {
            let mut dirs: Vec<&Dir> = self.dirs.iter().collect();
            dirs.sort_by_key(|d| (d.distance(size), d.scale, d.size.abs_diff(size)));
            dirs
        }
    }

    /// `index.theme`, a desktop-entry-style ini: `[Icon Theme]` lists the
    /// `Directories` (and `ScaledDirectories`) and `Inherits`; each
    /// directory has its own section.
    pub fn index(text: &str) -> Index {
        let mut sections: Vec<(String, Vec<(String, String)>)> = Vec::new();
        for line in text.lines().map(str::trim) {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                sections.push((name.to_string(), Vec::new()));
            } else if let (Some((key, value)), Some((_, entries))) = (line.split_once('='), sections.last_mut()) {
                entries.push((key.trim().to_string(), value.trim().to_string()));
            }
        }
        let value = |section: &str, key: &str| {
            sections
                .iter()
                .find(|(name, _)| name == section)
                .and_then(|(_, entries)| entries.iter().find(|(k, _)| k == key))
                .map(|(_, v)| v.as_str())
        };
        let list = |v: Option<&str>| -> Vec<String> {
            v.unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
        };
        let mut names = list(value("Icon Theme", "Directories"));
        names.extend(list(value("Icon Theme", "ScaledDirectories")));
        let number = |dir: &str, key: &str| value(dir, key).and_then(|v| v.parse::<u32>().ok());
        let mut dirs = Vec::new();
        for name in names {
            if dirs.iter().any(|d: &Dir| d.path == name) {
                continue;
            }
            let Some(size) = number(&name, "Size") else { continue };
            dirs.push(Dir {
                size,
                scale: number(&name, "Scale").unwrap_or(1),
                kind: value(&name, "Type").unwrap_or("Threshold").to_string(),
                min: number(&name, "MinSize").unwrap_or(size),
                max: number(&name, "MaxSize").unwrap_or(size),
                threshold: number(&name, "Threshold").unwrap_or(2),
                path: name,
            });
        }
        Index { dirs, inherits: list(value("Icon Theme", "Inherits")) }
    }

    /// A string setting out of `_XSETTINGS_SETTINGS`: a byte-order byte,
    /// three pad bytes, a serial and a count, then per setting its type
    /// (0 integer, 1 string, 2 colour), a pad byte, the name's length and
    /// the name padded to 4, the last-change serial, and the value (an
    /// integer; a length and the string padded to 4; four u16s).
    pub fn xsettings_string(data: &[u8], wanted: &str) -> Option<String> {
        let big = *data.first()? == 1;
        let u16_at = |at: usize| -> Option<u16> {
            let b: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
            Some(if big { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) })
        };
        let u32_at = |at: usize| -> Option<u32> {
            let b: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
            Some(if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
        };
        let pad = |n: usize| n.div_ceil(4) * 4;
        let count = u32_at(8)?;
        let mut at = 12;
        for _ in 0..count {
            let kind = *data.get(at)?;
            let name_len = usize::from(u16_at(at + 2)?);
            let name = data.get(at + 4..at + 4 + name_len)?;
            at += 4 + pad(name_len) + 4;
            match kind {
                0 => at += 4,
                1 => {
                    let len = u32_at(at)? as usize;
                    let value = data.get(at + 4..at + 4 + len)?;
                    if name == wanted.as_bytes() {
                        return String::from_utf8(value.to_vec()).ok();
                    }
                    at += 4 + pad(len);
                }
                2 => at += 8,
                _ => return None,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::parse::*;

    /// What this desktop gives: run in its session's environment with
    /// `cargo test -p native-term-os -- --ignored --nocapture this_desktop`.
    #[test]
    #[ignore]
    fn this_desktop_icons() {
        let look = crate::appearance::read();
        let theme = look.icon_theme;
        println!("icon theme: {theme:?}");
        println!("ui font: {:?}", look.ui_font);
        for (name, path) in super::window_icons(theme.as_deref()).named() {
            println!("{name}: {}", path.display());
        }
    }

    const ELEMENTARY: &str = "[Icon Theme]\nName=elementary\nInherits=hicolor\n\
        Directories=actions/16,actions/24,actions/symbolic,actions/16@2x\n\
        ScaledDirectories=actions/16@2x\n\n\
        [actions/16]\nSize=16\nContext=Actions\nType=Fixed\n\n\
        [actions/24]\nSize=24\nMinSize=8\nMaxSize=31\nType=Scalable\n\n\
        # comment\n[actions/symbolic]\nSize=16\nMinSize=8\nMaxSize=512\nType=Scalable\n\n\
        [actions/16@2x]\nSize=16\nScale=2\nType=Fixed\n";

    #[test]
    fn an_index_theme() {
        let elementary = index(ELEMENTARY);
        assert_eq!(elementary.inherits, ["hicolor"]);
        let paths: Vec<&str> = elementary.dirs.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, ["actions/16", "actions/24", "actions/symbolic", "actions/16@2x"], "each once");
        let near: Vec<&str> = elementary.by_nearness(16).iter().map(|d| d.path.as_str()).collect();
        assert_eq!(near, ["actions/16", "actions/symbolic", "actions/24", "actions/16@2x"], "drawn for 16 first");
        let breeze = "[Icon Theme]\nInherits=hicolor, Adwaita\nDirectories=actions/22,actions/16\n\
            [actions/22]\nSize=22\n[actions/16]\nSize=16\n";
        let breeze = index(breeze);
        assert_eq!(breeze.inherits, ["hicolor", "Adwaita"]);
        assert_eq!(breeze.by_nearness(16)[0].path, "actions/16", "Threshold (2) around each size");
        assert_eq!(index("").dirs, []);
    }

    fn setting(out: &mut Vec<u8>, kind: u8, name: &str, value: &[u8]) {
        out.extend([kind, 0]);
        out.extend((name.len() as u16).to_le_bytes());
        out.extend(name.as_bytes());
        out.resize(out.len().div_ceil(4) * 4, 0);
        out.extend(0u32.to_le_bytes());
        out.extend(value);
    }

    #[test]
    fn xsettings() {
        let mut data = vec![0, 0, 0, 0];
        data.extend(7u32.to_le_bytes());
        data.extend(3u32.to_le_bytes());
        setting(&mut data, 0, "Net/DoubleClickTime", &400u32.to_le_bytes());
        setting(&mut data, 2, "Gtk/Colour", &[1, 0, 2, 0, 3, 0, 4, 0]);
        let mut theme = 10u32.to_le_bytes().to_vec();
        theme.extend(b"elementary\0\0");
        setting(&mut data, 1, "Net/IconThemeName", &theme);
        assert_eq!(xsettings_string(&data, "Net/IconThemeName").as_deref(), Some("elementary"));
        assert_eq!(xsettings_string(&data, "Net/ThemeName"), None);
        assert_eq!(xsettings_string(&data[..20], "Net/IconThemeName"), None, "cut short");
        assert_eq!(xsettings_string(&[], "Net/IconThemeName"), None);
    }
}
