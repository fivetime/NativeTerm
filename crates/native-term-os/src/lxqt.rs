//! LXQt's look, read where LXQt's own programs take it from: its Qt
//! platform theme (`lxqt-qtplugin`, `lxqtplatformtheme.cpp`) gives every
//! Qt program the palette, icon theme and font of `lxqt/lxqt.conf`, what
//! the Appearance settings write:
//!
//! - `[Palette]`: `window_color` (`#efefef` without one), from which Qt
//!   makes a whole palette (`QPalette(QColor)`: a white base and black
//!   text for a window brighter than half, black and white otherwise),
//!   and over it what is there of `base_color`, `text_color`,
//!   `window_text_color`, `highlight_color` (else LXQt's `#3c8ce6`);
//! - `[General] icon_theme`, `[Qt] font` (`Ubuntu,11,-1,5,…`);
//! - a key the person's file has not is looked for in the system's
//!   (QSettings: `lxqt/lxqt.conf` and `lxqt.conf` in `XDG_CONFIG_HOME`,
//!   then in each of `XDG_CONFIG_DIRS`), where a distribution keeps its
//!   defaults (Lubuntu: `/etc/xdg/xdg-Lubuntu/lxqt/lxqt.conf`).
//!
//! Measured on Lubuntu 26.04: Qt's palette there is the file's (window
//! `#f5f6f7`, base `#ffffff`, highlight `#5294e2`), in Qt 5 and Qt 6.
//!
//! Chrome's tab strip there is another colour, `#d0d0d2`: it has the Qt
//! style draw a title bar, and the style (Kvantum) draws the one of a
//! window inside a window (`titlebar-focused` of its theme's SVG, a
//! gradient from `#d6d6d8` to `#ccccce`), which no window of the desktop
//! wears (Openbox's are `#2f343f` with the same theme). Having it would
//! take Kvantum's drawing (its themes' lookup and inheritance, an SVG
//! renderer) for a colour the desktop does not show; the strip is an LXQt
//! tab bar instead, the window's colour, as on UKUI (see `ukui`).

use std::path::PathBuf;

use crate::appearance::parse::{ini_value, is_dark, qt_font};
use crate::appearance::DesktopLook;
use crate::titlebar::{Rgb, Titlebar};

/// Whether the desktop is LXQt, as Chromium tells
/// (`base::nix::GetDesktopEnvironment`: by `XDG_CURRENT_DESKTOP` alone).
pub fn is_desktop(xdg_current_desktop: &str) -> bool {
    xdg_current_desktop.split(':').any(|d| d.trim() == "LXQt")
}

/// The settings files, the one whose keys count first: the person's,
/// then the system's (`config_home`: `XDG_CONFIG_HOME` or `~/.config`;
/// `config_dirs`: `XDG_CONFIG_DIRS`).
pub fn files(config_home: Option<PathBuf>, config_dirs: &str) -> Vec<PathBuf> {
    // (what the specification says of a variable not set)
    let dirs = if config_dirs.is_empty() { "/etc/xdg" } else { config_dirs };
    config_home
        .into_iter()
        .chain(dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from))
        .flat_map(|dir| [dir.join("lxqt").join("lxqt.conf"), dir.join("lxqt.conf")])
        .collect()
}

/// The look from the settings files' texts, in [`files`]' order; `None`
/// without any.
pub fn look(settings: &[String]) -> Option<DesktopLook> {
    if settings.is_empty() {
        return None;
    }
    let value = |section: &str, key: &str| {
        settings.iter().find_map(|text| ini_value(text, section, key)).map(|v| v.trim_matches('"'))
    };
    let color = |key: &str| value("Palette", key).and_then(qt_color);
    let window = color("window_color").unwrap_or((0xef, 0xef, 0xef));
    // Qt's palette of a colour: by its value (the brightest channel)
    let bright = window.0.max(window.1).max(window.2) > 128;
    let (base, text) = if bright { ((255, 255, 255), (0, 0, 0)) } else { ((0, 0, 0), (255, 255, 255)) };
    let title = color("window_text_color").unwrap_or(text);
    Some(DesktopLook {
        dark: is_dark(window),
        accent: Some(color("highlight_color").unwrap_or((60, 140, 230))),
        icon_theme: value("General", "icon_theme").filter(|v| !v.is_empty()).map(str::to_string),
        font: value("Qt", "font").and_then(qt_font),
        palette: Some(Titlebar {
            frame: window,
            frame_inactive: window,
            window: color("base_color").unwrap_or(base),
            text: color("text_color").unwrap_or(text),
            title,
            // (Qt's text in a window without the focus is the same)
            title_inactive: title,
            chrome_frame: true,
            ..Titlebar::default()
        }),
    })
}

/// A colour as the settings have it and `QColor::fromString` reads it:
/// `#rrggbb`, `#rgb`, `#aarrggbb` (the opacity is not the palette's to
/// show). A colour by its name is left to Qt's own (LXQt writes none).
fn qt_color(text: &str) -> Option<Rgb> {
    let hex = text.trim().strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    let nibble = |at: usize| u8::from_str_radix(&hex[at..at + 1], 16).ok().map(|n| n * 17);
    match hex.len() {
        3 => Some((nibble(0)?, nibble(1)?, nibble(2)?)),
        6 => Some((byte(0)?, byte(2)?, byte(4)?)),
        8 => Some((byte(2)?, byte(4)?, byte(6)?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::UiFont;

    /// Lubuntu 26.04, the person's file as the Appearance settings wrote
    /// it.
    const LUBUNTU: &str = "[General]\n__userfile__=true\nicon_follow_color_scheme=false\nicon_theme=Papirus\npalette_override=true\ntheme=Lubuntu Arc\n\n[Palette]\nbase_color=#ffffff\nhighlight_color=#5294e2\nhighlighted_text_color=#ffffff\ntext_color=#000000\nwindow_color=#f5f6f7\nwindow_text_color=#000000\n\n[Qt]\ndoubleClickInterval=400\nfont=\"Ubuntu,11,-1,5,50,0,0,0,0,0\"\nstyle=kvantum\n";

    #[test]
    fn the_desktop() {
        assert!(is_desktop("LXQt"));
        assert!(is_desktop("Lubuntu:LXQt"));
        assert!(!is_desktop("KDE"));
        assert!(!is_desktop("lxqt"), "as Chromium tells it");
        assert!(!is_desktop(""));
    }

    #[test]
    fn where_the_settings_are() {
        let found = files(Some("/home/u/.config".into()), "/etc/xdg/xdg-Lubuntu:/etc/xdg");
        let expected = [
            "/home/u/.config/lxqt/lxqt.conf",
            "/home/u/.config/lxqt.conf",
            "/etc/xdg/xdg-Lubuntu/lxqt/lxqt.conf",
            "/etc/xdg/xdg-Lubuntu/lxqt.conf",
            "/etc/xdg/lxqt/lxqt.conf",
            "/etc/xdg/lxqt.conf",
        ];
        assert_eq!(found, expected.map(|p| PathBuf::from(p).components().collect::<PathBuf>()));
        assert_eq!(files(None, "").first(), Some(&PathBuf::from("/etc/xdg").join("lxqt").join("lxqt.conf")));
    }

    #[test]
    fn lubuntu_as_measured() {
        // Qt's palette there: Window #f5f6f7, Base #ffffff, Text #000000,
        // Highlight #5294e2
        let look = look(&[LUBUNTU.to_string()]).unwrap();
        assert!(!look.dark);
        assert_eq!(look.accent, Some((0x52, 0x94, 0xe2)));
        assert_eq!(look.icon_theme.as_deref(), Some("Papirus"));
        assert_eq!(look.font, Some(UiFont { family: "Ubuntu".into(), tenths: 110 }));
        let bar = look.palette.unwrap();
        assert_eq!(bar.frame, (0xf5, 0xf6, 0xf7));
        assert_eq!(bar.frame_inactive, (0xf5, 0xf6, 0xf7));
        assert_eq!(bar.window, (0xff, 0xff, 0xff));
        assert_eq!(bar.text, (0, 0, 0));
        assert_eq!(bar.title, (0, 0, 0));
        assert!(bar.chrome_frame && bar.buttons.is_empty());
    }

    #[test]
    fn the_systems_file_for_what_the_persons_has_not() {
        let own = "[General]\nicon_theme=Papirus-Dark\n\n[Palette]\nwindow_color=#2b2b2b\n".to_string();
        let system = "[General]\nicon_theme=oxygen\n\n[Palette]\nwindow_color=#ffffff\nhighlight_color=#ff0000\n\n[Qt]\nfont=\"Sans,10\"\n".to_string();
        let look = look(&[own, system]).unwrap();
        assert!(look.dark);
        assert_eq!(look.icon_theme.as_deref(), Some("Papirus-Dark"));
        assert_eq!(look.accent, Some((255, 0, 0)));
        assert_eq!(look.font.map(|f| f.tenths), Some(100));
        let bar = look.palette.unwrap();
        // what Qt makes of a dark window's colour (measured: QPalette of
        // #2b2b2b has Base #000000 and Text #ffffff)
        assert_eq!(
            (bar.frame, bar.window, bar.text, bar.title),
            ((43, 43, 43), (0, 0, 0), (255, 255, 255), (255, 255, 255))
        );
    }

    #[test]
    fn settings_that_say_little() {
        assert_eq!(look(&[]), None);
        // LXQt's own defaults
        let look = look(&["[General]\ntheme=frost\n".to_string()]).unwrap();
        assert!(!look.dark);
        assert_eq!(look.accent, Some((60, 140, 230)));
        assert_eq!((look.icon_theme, look.font), (None, None));
        let bar = look.palette.unwrap();
        assert_eq!((bar.frame, bar.window, bar.text), ((0xef, 0xef, 0xef), (255, 255, 255), (0, 0, 0)));
    }

    #[test]
    fn colours_as_qt_reads_them() {
        // (measured with QColor::fromString)
        assert_eq!(qt_color("#fff"), Some((255, 255, 255)));
        assert_eq!(qt_color("#5294E2"), Some((0x52, 0x94, 0xe2)));
        assert_eq!(qt_color("#80102030"), Some((0x10, 0x20, 0x30)));
        assert_eq!(qt_color("rgb(1,2,3)"), None);
        assert_eq!(qt_color("1,2,3"), None);
        assert_eq!(qt_color("#12345"), None);
        assert_eq!(qt_color("#ggg"), None);
        assert_eq!(qt_color(""), None);
    }

    /// What this desktop gives: run in an LXQt session with
    /// `cargo test -p native-term-os -- --ignored --nocapture this_lxqt`.
    #[test]
    #[ignore]
    fn this_lxqt_desktop() {
        let home = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
        let found = files(home, &std::env::var("XDG_CONFIG_DIRS").unwrap_or_default());
        let texts: Vec<String> = found.iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect();
        println!("{:#?}", look(&texts));
    }
}
