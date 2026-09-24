//! What the desktop looks like, for windows that should look like they
//! belong: light or dark, the accent colour, the monospace font.
//!
//! There is no one place every system keeps this. Windows has it in the
//! registry and macOS in `defaults`; on Linux only the light-or-dark
//! preference has a standard home (the desktop portal's
//! `org.freedesktop.appearance` settings), and not every desktop fills
//! it in — one that ships an old portal answers "no preference" whatever
//! the setting is — so after the portal come the files the toolkits
//! themselves read (GTK's `settings.ini`, KDE's `kdeglobals`) and
//! GNOME's `gsettings`. Nothing here is per desktop: those are the
//! toolkit-level sources every desktop writes for its GTK and Qt
//! programs, and a desktop none of them cover simply reads as unknown,
//! which the program shows as light with the theme setting there to
//! override it. The monospace font is fontconfig's answer, if that is
//! an actual monospace font (some systems match `monospace` to a
//! proportional CJK font).
//!
//! `read()` asks the system (a few short subprocesses on Linux, some
//! tens of milliseconds); `refresh()` keeps the last answer for
//! `cached()`, which is what the desktop module's `accent()` returns on
//! Unix.

use std::sync::Mutex;

/// The desktop's look as far as it could be read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    /// Dark, light, or not known.
    pub dark: Option<bool>,
    /// The accent (selection, highlight) colour, if the desktop names one.
    pub accent: Option<(u8, u8, u8)>,
    /// The monospace font family the desktop prefers, when fontconfig
    /// names an actual monospace font.
    pub monospace: Option<String>,
    /// The desktop's window buttons, in GNOME's `button-layout` form
    /// (`close:maximize` — left of the colon at the left end of the title
    /// bar, the rest at the right), only close, minimize and maximize
    /// kept: GNOME's `button-layout`, KWin's `ButtonsOnLeft`/`ButtonsOnRight`
    /// (Linux only; `None` when the desktop does not say).
    pub button_layout: Option<String>,
    /// The icon theme the desktop uses (`elementary`, `breeze-dark`,
    /// `bloom`, …; see `icons`), Linux only.
    pub icon_theme: Option<String>,
    /// The GTK theme the desktop uses (`io.elementary.stylesheet.blueberry`,
    /// `Adwaita`, …), Linux only; with `icon_theme` it tells when the
    /// title bar GTK draws has changed (see `titlebar`).
    pub gtk_theme: Option<String>,
    /// The desktop's interface font (what its apps' tabs and labels are
    /// set in, and Chrome's): GTK's `gtk-font-name` (XSETTINGS
    /// `Gtk/FontName`, gsettings `font-name`, `settings.ini`) or KDE's
    /// `[General] font`, Linux only.
    pub ui_font: Option<UiFont>,
    /// A Qt desktop's palette as the tab strip takes it (see
    /// `titlebar::qt_colors`), so a change of colour scheme is a change.
    pub palette: Option<crate::titlebar::Titlebar>,
    /// Where `dark` came from (`portal`, `gtk`, `kdeglobals`,
    /// `gsettings`, `registry`, `defaults`), or empty.
    pub source: &'static str,
}

/// A font family and its size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiFont {
    pub family: String,
    /// The size in tenths of a point (11 pt: 110).
    pub tenths: u32,
}

impl UiFont {
    pub fn points(&self) -> f32 {
        self.tenths as f32 / 10.
    }
}

static LAST: Mutex<Option<Appearance>> = Mutex::new(None);

/// Read it again; whether it changed since the last `refresh`.
pub fn refresh() -> bool {
    let now = read();
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let changed = last.as_ref() != Some(&now);
    *last = Some(now);
    changed
}

/// The last `refresh`'s answer (unknown before the first).
#[must_use]
pub fn cached() -> Appearance {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// Ask the system now.
#[must_use]
pub fn read() -> Appearance {
    imp::read()
}

/// The readings, apart from the systems they come from: what the
/// portal, the toolkits' files, `gsettings` and fontconfig say. (Used
/// on Linux; tested everywhere.)
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
pub(crate) mod parse {
    /// A colour written the way the desktop files write it: `r,g,b`
    /// (kdeglobals), `#rrggbb` or `rgb(r,g,b)`.
    pub(super) fn parse_rgb(text: &str) -> Option<(u8, u8, u8)> {
        let text = text.trim();
        if let Some(hex) = text.strip_prefix('#') {
            if hex.len() == 6 {
                let v = u32::from_str_radix(hex, 16).ok()?;
                return Some(((v >> 16) as u8, (v >> 8) as u8, v as u8));
            }
            return None;
        }
        let inner = text.strip_prefix("rgb(").and_then(|t| t.strip_suffix(')')).unwrap_or(text);
        let mut parts = inner.split(',').map(|p| p.trim().parse::<u8>());
        let (r, g, b) = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
        parts.next().is_none().then_some((r, g, b))
    }

    /// Whether a colour is dark (the perceived brightness below half).
    pub(super) fn is_dark((r, g, b): (u8, u8, u8)) -> bool {
        (u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000 < 128
    }

    /// One key's value in an ini-style file, in `section` (case-sensitive
    /// names, `key=value`, spaces around `=` allowed).
    pub(crate) fn ini_value<'a>(text: &'a str, section: &str, key: &str) -> Option<&'a str> {
        let mut in_section = false;
        for line in text.lines() {
            let line = line.trim();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                in_section = name.trim() == section;
            } else if in_section {
                if let Some((k, v)) = line.split_once('=') {
                    if k.trim() == key {
                        return Some(v.trim());
                    }
                }
            }
        }
        None
    }

    /// GTK's `settings.ini`: `gtk-application-prefer-dark-theme`, else a
    /// theme whose name says dark (`Lingmo-dark`, `Adwaita-dark`); a theme
    /// name that doesn't means light.
    pub(super) fn gtk_dark(ini: &str) -> Option<bool> {
        if let Some("true" | "1" | "True" | "TRUE") = ini_value(ini, "Settings", "gtk-application-prefer-dark-theme") {
            return Some(true);
        }
        let theme = ini_value(ini, "Settings", "gtk-theme-name")?;
        Some(theme.to_ascii_lowercase().contains("dark"))
    }

    /// KDE's `kdeglobals`: dark when the window background is; the accent
    /// is `[General] AccentColor` (Plasma 5.23+), else the selection colour.
    pub(super) fn kde(globals: &str) -> (Option<bool>, Option<(u8, u8, u8)>) {
        let dark = ini_value(globals, "Colors:Window", "BackgroundNormal").and_then(parse_rgb).map(is_dark);
        let accent = ini_value(globals, "General", "AccentColor")
            .and_then(parse_rgb)
            .or_else(|| ini_value(globals, "Colors:Selection", "BackgroundNormal").and_then(parse_rgb));
        (dark, accent)
    }

    /// The portal's `color-scheme` (0 no preference, 1 dark, 2 light) from
    /// the last `uint32 N` in what `dbus-send --print-reply` printed, or the
    /// last number in what `busctl` printed (`v v u N`).
    pub(super) fn portal_scheme(reply: &str) -> Option<bool> {
        let n: u32 = reply.split_whitespace().rev().find_map(|w| w.parse().ok())?;
        match n {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        }
    }

    /// The portal's `accent-color`: three doubles in 0..=1 (`(ddd)`), as
    /// `dbus-send` (`double 0.1`) or `busctl` (`v v (ddd) 0.1 0.2 0.3`)
    /// print them. Out of range means the desktop has none.
    pub(super) fn portal_accent(reply: &str) -> Option<(u8, u8, u8)> {
        // the trailing run of numbers (dbus-send labels each `double` and
        // closes the struct with `}`; busctl writes them after `(ddd)`)
        let mut values: Vec<f64> = Vec::new();
        for word in reply.split_whitespace().rev() {
            match word.parse::<f64>() {
                Ok(v) => values.push(v),
                Err(_) if word == "}" || word == "double" => {}
                Err(_) => break,
            }
        }
        values.reverse();
        let [r, g, b] = values.as_slice() else { return None };
        if [r, g, b].iter().any(|v| !(0.0..=1.0).contains(*v)) {
            return None;
        }
        let byte = |v: f64| (v * 255.0).round() as u8;
        Some((byte(*r), byte(*g), byte(*b)))
    }

    /// KWin's `kwinrc` buttons (`X` close, `I` minimize, `A` maximize;
    /// the rest are menus and toggles) as a GNOME-style layout, KWin's
    /// defaults (`M` / `IAX`) for a key left out; `None` without either key
    /// unless `kde` (a KDE session runs on the defaults).
    pub(super) fn kwin_layout(kwinrc: &str, kde: bool) -> Option<String> {
        let left = ini_value(kwinrc, "org.kde.kdecoration2", "ButtonsOnLeft");
        let right = ini_value(kwinrc, "org.kde.kdecoration2", "ButtonsOnRight");
        if left.is_none() && right.is_none() && !kde {
            return None;
        }
        let names = |letters: &str| {
            letters
                .chars()
                .filter_map(|c| match c {
                    'X' => Some("close"),
                    'I' => Some("minimize"),
                    'A' => Some("maximize"),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(",")
        };
        Some(format!("{}:{}", names(left.unwrap_or("M")), names(right.unwrap_or("IAX"))))
    }

    /// `gsettings get org.gnome.desktop.wm.preferences button-layout`
    /// (`'appmenu:minimize,maximize,close'`) with close, minimize and
    /// maximize kept: `:minimize,maximize,close`.
    pub(super) fn gnome_layout(output: &str) -> Option<String> {
        let layout = output.trim().trim_matches('\'');
        let (left, right) = layout.split_once(':')?;
        let keep = |side: &str| {
            side.split(',')
                .map(str::trim)
                .filter(|b| matches!(*b, "close" | "minimize" | "maximize"))
                .collect::<Vec<_>>()
                .join(",")
        };
        Some(format!("{}:{}", keep(left), keep(right)))
    }

    /// KDE's icon theme in `kdeglobals` (`[Icons] Theme`), when set; KDE
    /// itself runs on `breeze` without it.
    pub(super) fn kde_icon_theme(kdeglobals: &str) -> Option<String> {
        ini_value(kdeglobals, "Icons", "Theme").map(str::to_string)
    }

    /// A Pango font description, GTK's form (`Cantarell 11`,
    /// `Noto Sans Bold 10.5`): the family without its style words, and the
    /// size.
    pub(super) fn pango_font(desc: &str) -> Option<super::UiFont> {
        const STYLES: &[&str] = &[
            "bold",
            "italic",
            "oblique",
            "light",
            "medium",
            "regular",
            "semi-bold",
            "semibold",
            "ultra-light",
            "extra-light",
            "heavy",
            "condensed",
            "book",
            "black",
            "thin",
            "normal",
            "demi-bold",
        ];
        let desc = desc.trim().trim_matches('\'').trim();
        let (rest, size) = desc.rsplit_once(char::is_whitespace)?;
        let size: f32 = size.trim().parse().ok()?;
        let mut words: Vec<&str> = rest.split_whitespace().collect();
        while words.len() > 1 && words.last().is_some_and(|w| STYLES.contains(&w.to_ascii_lowercase().as_str())) {
            words.pop();
        }
        let family = words.join(" ").trim_end_matches(',').to_string();
        (!family.is_empty() && size > 0.).then(|| super::UiFont { family, tenths: (size * 10.).round() as u32 })
    }

    /// Qt's font string, KDE's `[General] font=` (`Noto Sans,10,-1,5,50,…`):
    /// the family and its point size (none when given in pixels, -1).
    pub(super) fn qt_font(value: &str) -> Option<super::UiFont> {
        let mut parts = value.split(',');
        let family = parts.next()?.trim().to_string();
        let size: f32 = parts.next()?.trim().parse().ok()?;
        (!family.is_empty() && size > 0.).then(|| super::UiFont { family, tenths: (size * 10.).round() as u32 })
    }

    /// GTK's `settings.ini`: `gtk-theme-name`.
    pub(super) fn gtk_theme(settings: &str) -> Option<String> {
        ini_value(settings, "Settings", "gtk-theme-name").map(|v| v.trim_matches('"').to_string())
    }

    /// GTK's `settings.ini`: `gtk-icon-theme-name`.
    pub(super) fn gtk_icon_theme(settings: &str) -> Option<String> {
        ini_value(settings, "Settings", "gtk-icon-theme-name").map(|v| v.trim_matches('"').to_string())
    }

    /// `gsettings get org.gnome.desktop.interface color-scheme`.
    pub(super) fn gsettings_scheme(output: &str) -> Option<bool> {
        match output.trim().trim_matches('\'') {
            "prefer-dark" => Some(true),
            "prefer-light" => Some(false),
            _ => None,
        }
    }

    /// `fc-match -f '%{family}\t%{spacing}' monospace`: the first family
    /// name, when fontconfig's spacing says monospace (100) or dual (90).
    pub(super) fn fc_monospace(output: &str) -> Option<String> {
        let (family, spacing) = output.trim().split_once('\t')?;
        matches!(spacing.trim(), "100" | "90").then(|| family.split(',').next().unwrap_or(family).trim().to_string())
    }
}

#[cfg(windows)]
mod imp {
    use super::Appearance;

    pub fn read() -> Appearance {
        let light = native_term_win::desktop::user_registry_dword(
            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "AppsUseLightTheme",
        );
        Appearance {
            dark: light.map(|v| v == 0),
            accent: native_term_win::desktop::accent(),
            monospace: None,
            button_layout: None,
            icon_theme: None,
            gtk_theme: None,
            ui_font: None,
            palette: None,
            source: if light.is_some() { "registry" } else { "" },
        }
    }
}

#[cfg(unix)]
mod imp {
    #[cfg(not(target_os = "macos"))]
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    use super::Appearance;

    /// What `program args` printed, when it ran and succeeded.
    fn output(program: &str, args: &[&str]) -> Option<String> {
        ran(program, args)?.ok()
    }

    /// `None` when `program` could not be run at all (not installed);
    /// else what it printed, or `Err` when it failed.
    fn ran(program: &str, args: &[&str]) -> Option<Result<String, ()>> {
        let out = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        Some(if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).into_owned()) } else { Err(()) })
    }

    #[cfg(not(target_os = "macos"))]
    /// One `org.freedesktop.appearance` key through the portal's `Read`
    /// (the method every portal version has), by `dbus-send` or `busctl`.
    fn portal(key: &str) -> Option<String> {
        const DEST: &str = "org.freedesktop.portal.Desktop";
        const PATH: &str = "/org/freedesktop/portal/desktop";
        const NAMESPACE: &str = "org.freedesktop.appearance";
        let dbus_send = || {
            ran(
                "dbus-send",
                &[
                    "--session",
                    "--print-reply",
                    "--reply-timeout=1000",
                    &format!("--dest={DEST}"),
                    PATH,
                    "org.freedesktop.portal.Settings.Read",
                    &format!("string:{NAMESPACE}"),
                    &format!("string:{key}"),
                ],
            )
        };
        let busctl = || {
            output(
                "busctl",
                &[
                    "--user",
                    "--timeout=1",
                    "call",
                    DEST,
                    PATH,
                    "org.freedesktop.portal.Settings",
                    "Read",
                    "ss",
                    NAMESPACE,
                    key,
                ],
            )
        };
        // busctl only where dbus-send is not installed: a portal that does
        // not answer (a session without one) must not cost both timeouts
        match dbus_send() {
            Some(reply) => reply.ok(),
            None => busctl(),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn config_home() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }

    #[cfg(not(target_os = "macos"))]
    fn config_file(name: &str) -> Option<String> {
        std::fs::read_to_string(config_home()?.join(name)).ok()
    }

    fn monospace() -> Option<String> {
        super::parse::fc_monospace(&output("fc-match", &["-f", "%{family}\t%{spacing}", "monospace"])?)
    }

    #[cfg(target_os = "macos")]
    pub fn read() -> Appearance {
        // `AppleInterfaceStyle` exists only when dark; the accent is an
        // index (graphite -1, red 0 … pink 6; absent means blue)
        let dark = output("defaults", &["read", "-g", "AppleInterfaceStyle"]).is_some_and(|s| s.trim() == "Dark");
        let index = output("defaults", &["read", "-g", "AppleAccentColor"]).and_then(|s| s.trim().parse::<i32>().ok());
        let accent = match index {
            Some(-1) => (0x8c, 0x8c, 0x8c),
            Some(0) => (0xff, 0x5f, 0x57),
            Some(1) => (0xf7, 0x82, 0x1b),
            Some(2) => (0xff, 0xc6, 0x00),
            Some(3) => (0x62, 0xba, 0x46),
            Some(5) => (0xa5, 0x50, 0xa7),
            Some(6) => (0xf7, 0x4f, 0x9e),
            _ => (0x00, 0x7a, 0xff),
        };
        // the traffic lights sit on the left anyway, where WezTerm puts them
        Appearance {
            dark: Some(dark),
            accent: Some(accent),
            monospace: monospace(),
            button_layout: None,
            icon_theme: None,
            gtk_theme: None,
            ui_font: None,
            palette: None,
            source: "defaults",
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn read() -> Appearance {
        let mut dark = None;
        let mut source = "";
        let mut accent = None;
        if let Some(reply) = portal("color-scheme") {
            dark = super::parse::portal_scheme(&reply);
            if dark.is_some() {
                source = "portal";
            }
            accent = portal("accent-color").as_deref().and_then(super::parse::portal_accent);
        }
        if dark.is_none() {
            let gtk = config_file("gtk-3.0/settings.ini").or_else(|| config_file("gtk-4.0/settings.ini"));
            dark = gtk.as_deref().and_then(super::parse::gtk_dark);
            if dark.is_some() {
                source = "gtk";
            }
        }
        if let Some((kde_dark, kde_accent)) = config_file("kdeglobals").map(|g| super::parse::kde(&g)) {
            if dark.is_none() && kde_dark.is_some() {
                dark = kde_dark;
                source = "kdeglobals";
            }
            accent = accent.or(kde_accent);
        }
        if dark.is_none() {
            dark = output("gsettings", &["get", "org.gnome.desktop.interface", "color-scheme"])
                .as_deref()
                .and_then(super::parse::gsettings_scheme);
            if dark.is_some() {
                source = "gsettings";
            }
        }
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        let kde = desktop.split(':').any(|d| d.eq_ignore_ascii_case("KDE"));
        let button_layout = super::parse::kwin_layout(&config_file("kwinrc").unwrap_or_default(), kde).or_else(|| {
            // gsettings answers with the desktop's own override (Pantheon's
            // `close:maximize`) when XDG_CURRENT_DESKTOP names it
            output("gsettings", &["get", "org.gnome.desktop.wm.preferences", "button-layout"])
                .as_deref()
                .and_then(super::parse::gnome_layout)
        });
        // what GTK apps (Chrome too) use: XSETTINGS, which every
        // desktop's settings daemon publishes; then each desktop's own
        let gtk = || config_file("gtk-3.0/settings.ini").or_else(|| config_file("gtk-4.0/settings.ini"));
        // (Lingmo calls itself KDE but keeps its theme in settings.ini;
        // gsettings answers a bare `Adwaita` where nothing ever set it)
        let [x_icons, x_gtk, x_font]: [Option<String>; 3] =
            crate::icons::xsettings(&["Net/IconThemeName", "Net/ThemeName", "Gtk/FontName"])
                .try_into()
                .unwrap_or_default();
        let kdeglobals = config_file("kdeglobals").unwrap_or_default();
        let icon_theme = x_icons
            .or_else(|| super::parse::kde_icon_theme(&config_file("kdeglobals").unwrap_or_default()))
            .or_else(|| gtk().as_deref().and_then(super::parse::gtk_icon_theme))
            .or_else(|| {
                output("gsettings", &["get", "org.gnome.desktop.interface", "icon-theme"])
                    .map(|t| t.trim().trim_matches('\'').to_string())
                    .filter(|t| !t.is_empty())
            })
            .or_else(|| kde.then(|| "breeze".to_string()));
        let gtk_theme = x_gtk
            .or_else(|| {
                output("gsettings", &["get", "org.gnome.desktop.interface", "gtk-theme"])
                    .map(|t| t.trim().trim_matches('\'').to_string())
                    .filter(|t| !t.is_empty())
            })
            .or_else(|| gtk().as_deref().and_then(super::parse::gtk_theme));
        // the interface font as the toolkit Chrome would take it from
        let session = std::env::var("DESKTOP_SESSION").unwrap_or_default();
        let qt = crate::titlebar::toolkit(&desktop, &session) == crate::titlebar::Toolkit::Qt;
        let kde_font = || super::parse::ini_value(&kdeglobals, "General", "font").and_then(super::parse::qt_font);
        let gtk_font = || {
            x_font
                .as_deref()
                .and_then(super::parse::pango_font)
                .or_else(|| {
                    output("gsettings", &["get", "org.gnome.desktop.interface", "font-name"])
                        .as_deref()
                        .and_then(super::parse::pango_font)
                })
                .or_else(|| {
                    gtk().and_then(|ini| {
                        super::parse::ini_value(&ini, "Settings", "gtk-font-name")
                            .and_then(|v| super::parse::pango_font(v.trim_matches('"')))
                    })
                })
        };
        let ui_font = if qt { kde_font().or_else(gtk_font) } else { gtk_font().or_else(kde_font) };
        let palette = if qt { crate::titlebar::qt_colors(&kdeglobals) } else { None };
        Appearance {
            dark,
            accent,
            monospace: monospace(),
            button_layout,
            icon_theme,
            gtk_theme,
            ui_font,
            palette,
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse::*;
    use super::*;

    #[test]
    fn colours_in_every_spelling() {
        assert_eq!(parse_rgb("65,98,148"), Some((65, 98, 148)));
        assert_eq!(parse_rgb(" 250, 250 ,250 "), Some((250, 250, 250)));
        assert_eq!(parse_rgb("#1F6EE7"), Some((0x1f, 0x6e, 0xe7)));
        assert_eq!(parse_rgb("rgb(1,2,3)"), Some((1, 2, 3)));
        assert_eq!(parse_rgb("#1F6E"), None);
        assert_eq!(parse_rgb("1,2"), None);
        assert_eq!(parse_rgb("1,2,3,4"), None);
        assert!(is_dark((0x1c, 0x1c, 0x1c)));
        assert!(!is_dark((250, 250, 250)));
    }

    #[test]
    fn gtk_settings_ini_as_lingmo_writes_it() {
        let light = "[Settings]\ngtk-application-prefer-dark-theme=false\ngtk-icon-theme-name=Crule\ngtk-theme-name=Lingmo-light\n";
        let dark = "[Settings]\ngtk-application-prefer-dark-theme=true\ngtk-icon-theme-name=Crule-dark\ngtk-theme-name=Lingmo-dark\n";
        assert_eq!(gtk_dark(light), Some(false));
        assert_eq!(gtk_dark(dark), Some(true));
        assert_eq!(gtk_dark("[Settings]\ngtk-theme-name = Adwaita-dark\n"), Some(true), "the name says so");
        assert_eq!(gtk_dark("[Settings]\ngtk-font-name=Sans 10\n"), None, "nothing about the theme");
        assert_eq!(gtk_dark("[Other]\ngtk-theme-name=Adwaita-dark\n"), None, "wrong section");
    }

    #[test]
    fn kdeglobals_colours() {
        let globals = "[ColorEffects:Disabled]\nColor=210,205,218\n\n[Colors:Selection]\nBackgroundAlternate=171,188,248\nBackgroundNormal=65,98,148\n\n[Colors:Window]\nBackgroundNormal=239,240,241\nForegroundNormal=35,38,41\n\n[General]\nColorSchemeHash=7ec1\n";
        assert_eq!(kde(globals), (Some(false), Some((65, 98, 148))));
        let plasma = "[General]\nAccentColor=61,174,233\n\n[Colors:Window]\nBackgroundNormal=42,46,50\n";
        assert_eq!(kde(plasma), (Some(true), Some((61, 174, 233))));
        assert_eq!(kde("[General]\nfoo=1\n"), (None, None));
    }

    #[test]
    fn portal_replies_from_both_tools() {
        let dbus_send = "method return time=1790069033.329237 sender=:1.19 -> destination=:1.547 serial=360 reply_serial=2\n   variant       variant          uint32 2\n";
        assert_eq!(portal_scheme(dbus_send), Some(false));
        assert_eq!(portal_scheme("   variant       variant          uint32 1\n"), Some(true));
        assert_eq!(portal_scheme("   variant       variant          uint32 0\n"), None, "no preference");
        assert_eq!(portal_scheme("v v u 2\n"), Some(false));
        assert_eq!(portal_scheme("v v u 0\n"), None);
        assert_eq!(portal_scheme(""), None);
        let accent = "method return time=1 sender=:1.19 -> destination=:1.5 serial=3 reply_serial=2\n   variant       variant          struct {\n         double 0.2\n         double 0.4\n         double 1\n      }\n";
        assert_eq!(portal_accent(accent), Some((51, 102, 255)));
        assert_eq!(portal_accent("v v (ddd) 0.2 0.4 1\n"), Some((51, 102, 255)));
        assert_eq!(portal_accent("v v (ddd) -1 -1 -1\n"), None, "out of range: no accent");
        assert_eq!(portal_accent("Call failed: no such setting\n"), None);
        assert_eq!(portal_accent("   variant       variant          uint32 2\n"), None, "one number is no colour");
    }

    #[test]
    fn where_the_window_buttons_are() {
        let kwin = "[Windows]\nBorderlessMaximizedWindows=false\n\n[org.kde.kdecoration2]\nButtonsOnLeft=XIA\nButtonsOnRight=M\n";
        assert_eq!(kwin_layout(kwin, true).as_deref(), Some("close,minimize,maximize:"));
        let kwin = "[org.kde.kdecoration2]\nButtonsOnLeft=MS\nButtonsOnRight=HIAX\n";
        assert_eq!(kwin_layout(kwin, false).as_deref(), Some(":minimize,maximize,close"));
        assert_eq!(kwin_layout("[org.kde.kdecoration2]\nButtonsOnRight=X\n", false).as_deref(), Some(":close"));
        assert_eq!(kwin_layout("[Other]\nButtonsOnLeft=X\n", false), None, "not KWin's buttons");
        assert_eq!(kwin_layout("", true).as_deref(), Some(":minimize,maximize,close"), "KDE's defaults");
        assert_eq!(gnome_layout("'appmenu:minimize,maximize,close'\n").as_deref(), Some(":minimize,maximize,close"));
        assert_eq!(gnome_layout("'close,minimize,maximize:appmenu'").as_deref(), Some("close,minimize,maximize:"));
        assert_eq!(gnome_layout("'close:maximize'").as_deref(), Some("close:maximize"), "elementary");
        assert_eq!(gnome_layout("'appmenu:close'").as_deref(), Some(":close"));
        assert_eq!(gnome_layout(""), None);
        assert_eq!(kde_icon_theme("[Icons]\nTheme=breeze-dark\n").as_deref(), Some("breeze-dark"));
        assert_eq!(kde_icon_theme("[General]\nName=x\n"), None, "not set: the next source");
        assert_eq!(gtk_icon_theme("[Settings]\ngtk-icon-theme-name=Crule\n").as_deref(), Some("Crule"));
        assert_eq!(gtk_icon_theme("[Settings]\ngtk-icon-theme-name=\"Papirus\"\n").as_deref(), Some("Papirus"));
        assert_eq!(gtk_icon_theme("[Settings]\n"), None);
        assert_eq!(gtk_theme("[Settings]\ngtk-theme-name=Adwaita-dark\n").as_deref(), Some("Adwaita-dark"));
        let font = |f: &str, t: u32| Some(super::UiFont { family: f.to_string(), tenths: t });
        assert_eq!(pango_font("Cantarell 11"), font("Cantarell", 110));
        assert_eq!(pango_font("'Noto Sans CJK SC Bold 10.5'\n"), font("Noto Sans CJK SC", 105), "gsettings' quotes");
        assert_eq!(pango_font("Inter Variable 11"), font("Inter Variable", 110));
        assert_eq!(pango_font("Cantarell"), None, "no size");
        assert_eq!(qt_font("Noto Sans,10,-1,5,50,0,0,0,0,0"), font("Noto Sans", 100));
        assert_eq!(qt_font("Noto Sans,-1,13,5,400,0,0,0,0,0,0,0,0,0,0,1"), None, "in pixels");
    }

    #[test]
    fn gsettings_and_fontconfig_answers() {
        assert_eq!(gsettings_scheme("'prefer-dark'\n"), Some(true));
        assert_eq!(gsettings_scheme("'prefer-light'\n"), Some(false));
        assert_eq!(gsettings_scheme("'default'\n"), None);
        assert_eq!(fc_monospace("Noto Mono\t100\n"), Some("Noto Mono".to_string()));
        assert_eq!(fc_monospace("DejaVu Sans Mono,DejaVu Sans Mono Book\t100\n"), Some("DejaVu Sans Mono".to_string()));
        assert_eq!(fc_monospace("Noto Sans CJK SC\t\n"), None, "a proportional match is no monospace font");
        assert_eq!(fc_monospace("Noto Sans CJK SC\t0\n"), None);
    }

    #[test]
    fn a_refresh_reports_change() {
        // whatever the system says, saying it twice is no change
        refresh();
        assert!(!refresh());
        assert_eq!(cached(), read());
    }
}
