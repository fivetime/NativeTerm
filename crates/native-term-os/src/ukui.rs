//! UKUI's look, read where UKUI's own programs take it from. Chrome goes
//! to Qt there and takes Qt 5 first (`PreferQt6` is KDE 6 only), for which
//! UKUI ships no theme: its colours on UKUI are Qt's built-in Fusion
//! style's (measured on Ubuntu Kylin 26.04: window `#efefef`, a title bar
//! of `#308eca`), not the desktop's. So this is not Chrome's way but
//! UKUI's own Qt 6 style's (`qt6-ukui-platformtheme`,
//! `ukui-styles/readconfig.cpp`, `GlobalDTConfigPrivate`):
//!
//! - gsettings `org.ukui.style`: `style-name` (`ukui-dark`, `ukui-black`:
//!   dark; anything else light), `widget-theme-name` (`default`,
//!   `classical`, `fashion`, …), `theme-color` (a name or a colour),
//!   `icon-theme-name`, `system-font` and `system-font-size`;
//! - the design tokens of that theme,
//!   `/usr/share/config/themeconfig/token/k<widget theme>-<light|dark>.css`
//!   (`k<widget theme>.css` before it where there is one): one
//!   `--name: value;` a line, a value being `rgba(r, g, b, a)`,
//!   `var(--Other-Name)` (looked up in lower case) or layers to be laid
//!   over a colour (`linear-gradient(…), rgba(…)`);
//! - the highlight is the theme colour whatever the tokens say, unless
//!   `theme-color` is `default`.
//!
//! The tokens are read as UKUI reads them, by line and not as CSS: a CSS
//! parser (cssparser, lightningcss) would give the declarations but none
//! of UKUI's rules for them (names in lower case, layers mixed).
//!
//! The tab strip is a UKUI tab bar: the window's colour with the active
//! tab in the base colour (seen in a Qt 6 window there: `#f6f6f6` and
//! `#ffffff`; the window manager's title bar is the base colour too, which
//! would hide the active tab).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::appearance::{DesktopLook as Look, UiFont};
use crate::titlebar::{Rgb, Titlebar};

/// Whether the desktop is UKUI, as Chromium tells
/// (`base::nix::GetDesktopEnvironment`): by `XDG_CURRENT_DESKTOP`, else
/// by `DESKTOP_SESSION`.
pub fn is_desktop(xdg_current_desktop: &str, desktop_session: &str) -> bool {
    xdg_current_desktop.split(':').any(|d| d.trim() == "UKUI") || desktop_session == "ukui"
}

/// Where UKUI's themes keep their design tokens.
pub const TOKENS: &str = "/usr/share/config/themeconfig/token";

/// What `org.ukui.style` says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub dark: bool,
    /// `default`, `classical`, `fashion`, `dawnlight`.
    pub widget_theme: String,
    /// `daybreakBlue` and the like, a colour, or `default`.
    pub theme_color: String,
    pub icon_theme: Option<String>,
    /// `system-font` at `system-font-size` (points, `10` or `10.5`).
    pub font: Option<UiFont>,
}

impl Style {
    /// From what `gsettings list-recursively org.ukui.style` printed
    /// (`org.ukui.style KEY VALUE` a line); `None` when that is not it.
    pub fn parse(listing: &str) -> Option<Style> {
        let value = |key: &str| {
            listing.lines().find_map(|line| {
                let rest = line.trim().strip_prefix("org.ukui.style ")?.strip_prefix(key)?;
                // (a key that only starts like this one is another key)
                rest.starts_with(' ').then(|| rest.trim().trim_matches('\'').to_string())
            })
        };
        let style = value("style-name")?;
        let font = value("system-font").filter(|family| !family.is_empty()).and_then(|family| {
            let points: f32 = value("system-font-size")?.parse().ok()?;
            (points > 0. && points < 1000.).then(|| UiFont { family, tenths: (points * 10.).round() as u32 })
        });
        Some(Style {
            dark: matches!(style.as_str(), "ukui-dark" | "ukui-black"),
            // (what UKUI's style starts with where the key is not there)
            widget_theme: value("widget-theme-name").filter(|v| !v.is_empty()).unwrap_or_else(|| "default".into()),
            theme_color: value("theme-color").unwrap_or_default(),
            icon_theme: value("icon-theme-name").filter(|v| !v.is_empty()),
            font,
        })
    }

    /// The token files of this style, the one UKUI takes first.
    pub fn token_files(&self, dir: &Path) -> [PathBuf; 2] {
        let mode = if self.dark { "dark" } else { "light" };
        [dir.join(format!("k{}.css", self.widget_theme)), dir.join(format!("k{}-{mode}.css", self.widget_theme))]
    }

    /// The highlight: the theme colour by its name (the table is in
    /// UKUI's code, `initUKUIGlobalThemeParameters`) or as a colour; `None`
    /// for `default`, which leaves it to the tokens.
    pub fn accent(&self) -> Option<Rgb> {
        match self.theme_color.as_str() {
            "" | "default" => None,
            "daybreakBlue" => Some((55, 144, 250)),
            "jamPurple" => Some((120, 115, 245)),
            "magenta" => Some((235, 48, 150)),
            "sunRed" => Some((243, 34, 45)),
            "sunsetOrange" => Some((246, 140, 39)),
            "dustGold" => Some((249, 197, 61)),
            "polarGreen" => Some((82, 196, 41)),
            color => Color::parse(color).map(Color::opaque),
        }
    }
}

/// UKUI's look (what the settings say, and the palette where the theme's
/// tokens were found) from the settings' listing (see [`Style::parse`]) and the
/// token files under `dir`.
pub fn look(listing: &str, dir: &Path) -> Option<Look> {
    let style = Style::parse(listing)?;
    let tokens = style.token_files(dir).iter().find_map(|file| std::fs::read_to_string(file).ok());
    Some(look_of(&style, tokens.as_deref()))
}

/// The look of a style with its tokens (the file's text).
pub fn look_of(style: &Style, tokens: Option<&str>) -> Look {
    let tokens = tokens.map(Tokens::parse);
    let accent = style.accent().or_else(|| Some(tokens.as_ref()?.color("highlight-active")?.opaque()));
    Look {
        dark: style.dark,
        accent,
        icon_theme: style.icon_theme.clone(),
        font: style.font.clone(),
        palette: tokens.as_ref().and_then(palette),
    }
}

/// The tab strip's colours from a theme's tokens: Qt's palette roles as
/// UKUI's style fills them (`window-active` is QPalette's Window in an
/// active window, and so on). Text is what is seen of it on its
/// background (UKUI's text colours are black or white, in part).
fn palette(tokens: &Tokens) -> Option<Titlebar> {
    let frame = tokens.color("window-active")?.opaque();
    let frame_inactive = tokens.color("window-inactive").map_or(frame, Color::opaque);
    let window = tokens.color("base-active")?.opaque();
    let title = tokens.color("windowtext-active")?;
    let text = tokens.color("text-active").unwrap_or(title);
    let title_inactive = tokens.color("windowtext-inactive").unwrap_or(title);
    Some(Titlebar {
        frame,
        frame_inactive,
        window,
        text: text.on(window),
        title: title.on(frame),
        title_inactive: title_inactive.on(frame_inactive),
        chrome_frame: true,
        ..Titlebar::default()
    })
}

/// A theme's design tokens by name.
struct Tokens<'a>(HashMap<String, &'a str>);

impl<'a> Tokens<'a> {
    fn parse(css: &'a str) -> Self {
        let mut tokens = HashMap::new();
        for line in css.lines() {
            let Some((name, value)) = line.trim().strip_prefix("--").and_then(|l| l.split_once(':')) else { continue };
            // (the first of a name is the one UKUI finds)
            tokens.entry(name.trim().to_string()).or_insert(value.trim().trim_end_matches(';').trim());
        }
        Tokens(tokens)
    }

    fn color(&self, name: &str) -> Option<Color> {
        self.resolve(name, 0)
    }

    /// UKUI's `getColorValue`. A gradient from one colour to another
    /// (`180deg`: a button under the pointer) is here the colour half way.
    fn resolve(&self, name: &str, depth: usize) -> Option<Color> {
        // (a theme whose names go round in a circle has no colour)
        if depth > 16 {
            return None;
        }
        let value = *self.0.get(name)?;
        if let Some(other) = value.strip_prefix("var(--") {
            return self.resolve(&other.split(')').next()?.trim().to_ascii_lowercase(), depth + 1);
        }
        if value.starts_with("rgba(") {
            return Color::parse(value);
        }
        let layers = value.strip_prefix("linear-gradient(")?;
        let colors: Vec<Color> = layers.split("rgba").skip(1).filter_map(Color::parse).collect();
        let (bottom, over) = colors.split_last()?;
        if layers.starts_with("180deg") {
            let (first, second) = over.split_at(over.len().div_ceil(2));
            let (top, below) = (mix(first, *bottom), mix(second, *bottom));
            let half = |a: u8, b: u8| ((u16::from(a) + u16::from(b)) / 2) as u8;
            return Some(Color {
                r: half(top.r, below.r),
                g: half(top.g, below.g),
                b: half(top.b, below.b),
                a: ((u32::from(top.a) + u32::from(below.a)) / 2) as u16,
            });
        }
        Some(mix(over, *bottom))
    }
}

/// The layers `over` laid on `bottom`, the first of them first as UKUI's
/// `mixColor` does, each colour once.
fn mix(over: &[Color], bottom: Color) -> Color {
    let mut seen: Vec<Color> = Vec::new();
    let mut color = bottom;
    for layer in over {
        if !seen.contains(layer) {
            seen.push(*layer);
            color = layer.over(color);
        }
    }
    color
}

/// A colour with its opacity, as Qt keeps one (QColor): the opacity in
/// 0..=65535.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Color {
    r: u8,
    g: u8,
    b: u8,
    a: u16,
}

impl Color {
    /// UKUI's `stringToColor`: what is between the brackets, or all of it
    /// without any (`rgba(55, 144, 250, 1)`, `(125,125,125)`,
    /// `55,144,250,1`), `r, g, b` and an alpha in 0..=1 or up to 255; or
    /// `#rrggbb`.
    fn parse(text: &str) -> Option<Color> {
        let text = text.trim();
        let inner = match text.split_once('(') {
            Some((_, rest)) => rest.split(')').next()?,
            None => text,
        };
        if let Some(hex) = inner.trim().strip_prefix('#') {
            let v = u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6)?;
            return Some(Color { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8, a: u16::MAX });
        }
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        let channel = |i: usize| parts.get(i)?.parse::<u8>().ok();
        let (r, g, b) = (channel(0)?, channel(1)?, channel(2)?);
        let a = match parts.get(3) {
            None => 1.,
            Some(alpha) => {
                let a: f32 = alpha.parse().ok()?;
                if a <= 1. {
                    a.max(0.)
                } else {
                    (a / 255.).min(1.)
                }
            }
        };
        (parts.len() <= 4).then_some(Color { r, g, b, a: Self::alpha_from(a) })
    }

    /// QColor's `setAlphaF` and `alphaF`.
    fn alpha_from(a: f32) -> u16 {
        (a * 65535.).round() as u16
    }

    fn alpha(self) -> f64 {
        f64::from(f32::from(self.a) / 65535.)
    }

    /// This colour over `back`: UKUI's `composeColor`, in Qt's numbers
    /// (an opacity is a `float` there, the sum a `double`, a channel the
    /// whole number below it: black at 0.2 over 245 is 195, as the style
    /// has it, not 196).
    fn over(self, back: Color) -> Color {
        let (fore_a, back_a) = (self.alpha(), back.alpha());
        let a = fore_a + back_a * (1. - fore_a);
        if a <= 0. {
            return Color { a: 0, ..back };
        }
        let channel =
            |fore: u8, back: u8| ((f64::from(fore) * fore_a + f64::from(back) * back_a * (1. - fore_a)) / a) as u8;
        Color {
            r: channel(self.r, back.r),
            g: channel(self.g, back.g),
            b: channel(self.b, back.b),
            a: Self::alpha_from(a as f32),
        }
    }

    /// As painted on an opaque background (the opacity in 0..=255 there).
    fn on(self, (r, g, b): Rgb) -> Rgb {
        // (QColor's `alpha`: a division by 257, to the nearest)
        let a = u32::from(self.a);
        let a = (a - (a >> 8) + 0x80) >> 8;
        let channel = |fore: u8, back: u8| ((u32::from(fore) * a + u32::from(back) * (255 - a) + 127) / 255) as u8;
        (channel(self.r, r), channel(self.g, g), channel(self.b, b))
    }

    /// Without its opacity, as Qt names a colour.
    fn opaque(self) -> Rgb {
        (self.r, self.g, self.b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ubuntu Kylin 26.04 as installed.
    const SETTINGS: &str = "org.ukui.style custom-highlight-color '#3D6BE5'\norg.ukui.style icon-theme-name 'ukui-icon-theme-default'\norg.ukui.style style-name 'ukui-light'\norg.ukui.style system-font 'Noto Sans CJK SC'\norg.ukui.style system-font-size '10'\norg.ukui.style theme-color 'daybreakBlue'\norg.ukui.style use-custom-highlight-color false\norg.ukui.style widget-theme-name 'default'\n";

    /// Lines of ukui-themes' `kdefault-light.css`.
    const LIGHT: &str = "body {\n\t--base-active: var(--kgray-0);\n\t--highlight-active: var(--kbrand-normal);\n\t--kbrand1: rgba(55, 144, 250, 1);\n\t--kbrand-click: linear-gradient(rgba(0, 0, 0, 0.2)), rgba(55, 144, 250, 1);\n\t--kbrand-normal: var(--kbrand1);\n\t--kfont-primary: rgba(0, 0, 0, 0.85);\n\t--kfont-secondary-disable: rgba(0, 0, 0, 0.3);\n\t--kgray-0: rgba(255, 255, 255, 1);\n\t--kgray-2: rgba(246, 246, 246, 1);\n\t--text-active: var(--kfont-primary);\n\t--window-active: var(--kgray-2);\n\t--window-inactive: var(--kgray-2);\n\t--windowtext-active: var(--kfont-primary);\n\t--windowtext-inactive: var(--kfont-secondary-disable);\n}\n";

    /// Lines of `kclassical-dark.css`: names in capitals where they are
    /// used, layers over the highlight.
    const CLASSICAL_DARK: &str = "body {\n    --windowtext-active: rgba(255, 255, 255, 0.9);\n    --base-inactive: var(--KGray-1);\n    --kgray-3: rgba(54, 54, 54, 1);\n    --highlight-active: linear-gradient(0deg, rgba(0, 0, 0, 0.4) 0%, rgba(0, 0, 0, 0.4) 100%), rgba(55, 144, 250, 1);\n    --kgray-0: rgba(30, 30, 30, 1);\n    --windowtext-inactive: rgba(255, 255, 255, 0.65);\n    --kgray-2: rgba(46, 46, 46, 1);\n    --text-active: rgba(255, 255, 255, 0.9);\n    --window-inactive: var(--KGray-3);\n    --base-active: var(--KGray-0);\n    --window-active: var(--KGray-2);\n}\n";

    #[test]
    fn the_settings() {
        let style = Style::parse(SETTINGS).unwrap();
        assert!(!style.dark);
        assert_eq!(style.widget_theme, "default");
        assert_eq!(style.theme_color, "daybreakBlue");
        assert_eq!(style.icon_theme.as_deref(), Some("ukui-icon-theme-default"));
        assert_eq!(style.font, Some(UiFont { family: "Noto Sans CJK SC".into(), tenths: 100 }));
        let half = "org.ukui.style style-name 'ukui-light'\norg.ukui.style system-font 'Sans'\norg.ukui.style system-font-size '10.5'\n";
        assert_eq!(Style::parse(half).unwrap().font.map(|f| f.tenths), Some(105));
        assert_eq!(
            style.token_files(Path::new(TOKENS)),
            [Path::new(TOKENS).join("kdefault.css"), Path::new(TOKENS).join("kdefault-light.css")]
        );
        let dark = Style::parse("org.ukui.style style-name 'ukui-dark'\n").unwrap();
        assert!(dark.dark);
        assert_eq!(dark.widget_theme, "default", "UKUI's own default");
        assert_eq!(dark.font, None);
        assert_eq!(dark.token_files(Path::new("/t"))[1], Path::new("/t").join("kdefault-dark.css"));
        assert!(Style::parse("org.ukui.style style-name 'ukui-black'\n").unwrap().dark);
        assert!(!Style::parse("org.ukui.style style-name 'ukui-default'\n").unwrap().dark);
        assert_eq!(Style::parse("No such schema \"org.ukui.style\"\n"), None);
        assert_eq!(Style::parse(""), None);
    }

    #[test]
    fn the_desktop() {
        assert!(is_desktop("UKUI", "ukui"));
        assert!(is_desktop("", "ukui"));
        assert!(is_desktop("x:UKUI", ""));
        assert!(!is_desktop("KDE", "plasma"));
        assert!(!is_desktop("", ""));
    }

    #[test]
    fn the_theme_colour() {
        let with = |color: &str| Style { theme_color: color.into(), ..Style::default() }.accent();
        assert_eq!(with("daybreakBlue"), Some((55, 144, 250)));
        assert_eq!(with("polarGreen"), Some((82, 196, 41)));
        assert_eq!(with("55,144,250,1"), Some((55, 144, 250)), "the schema's default");
        assert_eq!(with("(125,125,125)"), Some((125, 125, 125)));
        assert_eq!(with("#3790FA"), Some((0x37, 0x90, 0xfa)));
        assert_eq!(with("default"), None);
        assert_eq!(with("skyBlue"), None, "a name UKUI has not");
    }

    #[test]
    fn the_light_theme_as_measured() {
        // Qt 6 on that desktop: Window #f6f6f6, Base #ffffff, Highlight
        // #3790fa; a tab's text was seen as #262626 on the white tab
        let look = look_of(&Style::parse(SETTINGS).unwrap(), Some(LIGHT));
        assert!(!look.dark);
        assert_eq!(look.accent, Some((0x37, 0x90, 0xfa)));
        let bar = look.palette.unwrap();
        assert_eq!(bar.frame, (0xf6, 0xf6, 0xf6));
        assert_eq!(bar.frame_inactive, (0xf6, 0xf6, 0xf6));
        assert_eq!(bar.window, (0xff, 0xff, 0xff));
        assert_eq!(bar.text, (0x26, 0x26, 0x26));
        assert_eq!(bar.title, (0x25, 0x25, 0x25));
        assert_eq!(bar.title_inactive, (0xac, 0xac, 0xac));
        assert!(bar.chrome_frame && bar.buttons.is_empty());
    }

    #[test]
    fn a_theme_with_layers_and_capitals() {
        let style =
            Style { dark: true, widget_theme: "classical".into(), theme_color: "default".into(), ..Style::default() };
        let look = look_of(&style, Some(CLASSICAL_DARK));
        // black at 0.4, once, over the blue
        assert_eq!(look.accent, Some((32, 86, 149)));
        let bar = look.palette.unwrap();
        assert_eq!(bar.frame, (46, 46, 46));
        assert_eq!(bar.frame_inactive, (54, 54, 54));
        assert_eq!(bar.window, (30, 30, 30));
        assert_eq!(bar.text, (233, 233, 233));
        // the theme colour, where one is set, over the tokens' highlight
        let named = Style { theme_color: "sunRed".into(), ..style };
        assert_eq!(look_of(&named, Some(CLASSICAL_DARK)).accent, Some((243, 34, 45)));
    }

    #[test]
    fn tokens_that_are_not_there() {
        let style = Style::parse(SETTINGS).unwrap();
        let look = look_of(&style, None);
        assert_eq!(look.palette, None);
        assert_eq!(look.accent, Some((55, 144, 250)), "the settings still say");
        assert_eq!(look_of(&style, Some("body {\n\t--window-active: var(--kgray-2);\n}\n")).palette, None);
        let circle = "--window-active: var(--a);\n--a: var(--window-active);\n--base-active: rgba(1, 2, 3, 1);\n";
        assert_eq!(look_of(&style, Some(circle)).palette, None);
    }

    #[test]
    fn colours_and_layers() {
        assert_eq!(Color::parse("rgba(55, 144, 250, 1)"), Some(Color { r: 55, g: 144, b: 250, a: 65535 }));
        assert_eq!(Color::parse("rgba(0, 0, 0, 0.5) 0%"), Some(Color { r: 0, g: 0, b: 0, a: 32768 }));
        assert_eq!(Color::parse("rgba(0, 0, 0, 255)").map(|c| c.a), Some(65535));
        assert_eq!(Color::parse("rgba(0, 0)"), None);
        assert_eq!(Color::parse("rgba(0, 0, 300, 1)"), None);
        // what UKUI's style had of kdefault-light's layers (its
        // application properties, read in a Qt 6 program there)
        let tokens = Tokens::parse(
            "--kerror-click: linear-gradient(rgba(0, 0, 0, 0.2)), rgba(245, 63, 63, 1);\n--kerror-hover: linear-gradient(rgba(0, 0, 0, 0.05)), rgba(245, 63, 63, 1);\n--kwarning-click: linear-gradient(rgba(0, 0, 0, 0.2)), rgba(255, 125, 0, 1);\n--ksuccess-hover: linear-gradient(rgba(0, 0, 0, 0.05)), rgba(0, 180, 42, 1);\n",
        );
        assert_eq!(tokens.color("kerror-click").map(Color::opaque), Some((0xc3, 0x32, 0x32)));
        assert_eq!(tokens.color("kerror-hover").map(Color::opaque), Some((0xe8, 0x3b, 0x3b)));
        assert_eq!(tokens.color("kwarning-click").map(Color::opaque), Some((0xcb, 0x63, 0x00)));
        assert_eq!(tokens.color("ksuccess-hover").map(Color::opaque), Some((0x00, 0xaa, 0x27)));
        let gradient = Tokens::parse(
            "--hover: linear-gradient(180deg, rgba(255, 255, 255, 0.2) 0%, rgba(0, 0, 0, 0.2) 100%), rgba(100, 100, 100, 1);\n",
        );
        // 131 at the top, 79 at the bottom
        assert_eq!(gradient.color("hover").map(Color::opaque), Some((105, 105, 105)));
    }

    /// What this desktop gives: run in a UKUI session with
    /// `cargo test -p native-term-os -- --ignored --nocapture this_ukui`.
    #[test]
    #[ignore]
    fn this_ukui_desktop() {
        let listing = std::process::Command::new("gsettings").args(["list-recursively", "org.ukui.style"]).output();
        let listing = String::from_utf8_lossy(&listing.map(|o| o.stdout).unwrap_or_default()).into_owned();
        println!("{:#?}", look(&listing, Path::new(TOKENS)));
    }
}
