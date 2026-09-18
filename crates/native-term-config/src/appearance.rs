//! A session tab's look, per host or as a folder default:
//! `NativeTermTabColor` (a color, or `none` to have none where the folder
//! has one), passed to `wt new-tab` as `--tabColor`, and
//! `NativeTermColorScheme` (one of Windows Terminal's built-in schemes, or
//! `none`), which the shim applies in the tab itself with OSC 4 / 10 / 11 /
//! 12: `wt new-tab --colorScheme` had no effect in Terminal 1.26 (tried on
//! a plain `cmd` tab too). A Terminal profile per host isn't offered:
//! NativeTerm's own profile keeps the tab title fixed, which is how its
//! tabs are found.

use crate::tree::{Folder, HostEntry};

pub const TAB_COLOR: &str = "tabcolor";
pub const COLOR_SCHEME: &str = "colorscheme";

/// Named colors offered in the UI (the name is stored as given; `wt` gets
/// the hex value).
pub const PRESETS: [(&str, &str); 6] = [
    ("red", "#C0392B"),
    ("orange", "#D35400"),
    ("yellow", "#B7950B"),
    ("green", "#1E8449"),
    ("blue", "#2E86C1"),
    ("purple", "#7D3C98"),
];

/// A color scheme: what the shim sets in the tab.
#[derive(Debug, PartialEq, Eq)]
pub struct Scheme {
    pub name: &'static str,
    pub foreground: &'static str,
    pub background: &'static str,
    pub cursor: &'static str,
    /// black, red, green, yellow, blue, purple, cyan, white, then the
    /// bright ones (the console's 16 colors, OSC 4 indexes 0–15).
    pub palette: [&'static str; 16],
}

/// Windows Terminal's built-in schemes, as its `defaults.json` (1.26)
/// defines them.
pub const SCHEMES: [Scheme; 16] = [
    Scheme {
        name: "Dimidium",
        foreground: "#BAB7B6",
        background: "#141414",
        cursor: "#37E57B",
        palette: ["#000000", "#CF494C", "#60B442", "#DB9C11", "#0575D8", "#AF5ED2", "#1DB6BB", "#BAB7B6", "#817E7E", "#FF643B", "#37E57B", "#FCCD1A", "#688DFD", "#ED6FE9", "#32E0FB", "#DEE3E4"],
    },
    Scheme {
        name: "Ottosson",
        foreground: "#bebebe",
        background: "#000000",
        cursor: "#ffffff",
        palette: ["#000000", "#be2c21", "#3fae3a", "#be9a4a", "#204dbe", "#bb54be", "#00a7b2", "#bebebe", "#808080", "#ff3e30", "#58ea51", "#ffc944", "#2f6aff", "#fc74ff", "#00e1f0", "#ffffff"],
    },
    Scheme {
        name: "Campbell",
        foreground: "#CCCCCC",
        background: "#0C0C0C",
        cursor: "#FFFFFF",
        palette: ["#0C0C0C", "#C50F1F", "#13A10E", "#C19C00", "#0037DA", "#881798", "#3A96DD", "#CCCCCC", "#767676", "#E74856", "#16C60C", "#F9F1A5", "#3B78FF", "#B4009E", "#61D6D6", "#F2F2F2"],
    },
    Scheme {
        name: "Campbell Powershell",
        foreground: "#CCCCCC",
        background: "#012456",
        cursor: "#FFFFFF",
        palette: ["#0C0C0C", "#C50F1F", "#13A10E", "#C19C00", "#0037DA", "#881798", "#3A96DD", "#CCCCCC", "#767676", "#E74856", "#16C60C", "#F9F1A5", "#3B78FF", "#B4009E", "#61D6D6", "#F2F2F2"],
    },
    Scheme {
        name: "Vintage",
        foreground: "#C0C0C0",
        background: "#000000",
        cursor: "#FFFFFF",
        palette: ["#000000", "#800000", "#008000", "#808000", "#000080", "#800080", "#008080", "#C0C0C0", "#808080", "#FF0000", "#00FF00", "#FFFF00", "#0000FF", "#FF00FF", "#00FFFF", "#FFFFFF"],
    },
    Scheme {
        name: "One Half Dark",
        foreground: "#DCDFE4",
        background: "#282C34",
        cursor: "#FFFFFF",
        palette: ["#282C34", "#E06C75", "#98C379", "#E5C07B", "#61AFEF", "#C678DD", "#56B6C2", "#DCDFE4", "#5A6374", "#E06C75", "#98C379", "#E5C07B", "#61AFEF", "#C678DD", "#56B6C2", "#DCDFE4"],
    },
    Scheme {
        name: "One Half Light",
        foreground: "#383A42",
        background: "#FAFAFA",
        cursor: "#4F525D",
        palette: ["#383A42", "#E45649", "#50A14F", "#C18301", "#0184BC", "#A626A4", "#0997B3", "#FAFAFA", "#4F525D", "#DF6C75", "#98C379", "#E4C07A", "#61AFEF", "#C577DD", "#56B5C1", "#FFFFFF"],
    },
    Scheme {
        name: "Solarized Dark",
        foreground: "#839496",
        background: "#002B36",
        cursor: "#FFFFFF",
        palette: ["#002B36", "#DC322F", "#859900", "#B58900", "#268BD2", "#D33682", "#2AA198", "#EEE8D5", "#073642", "#CB4B16", "#586E75", "#657B83", "#839496", "#6C71C4", "#93A1A1", "#FDF6E3"],
    },
    Scheme {
        name: "Solarized Light",
        foreground: "#657B83",
        background: "#FDF6E3",
        cursor: "#002B36",
        palette: ["#002B36", "#DC322F", "#859900", "#B58900", "#268BD2", "#D33682", "#2AA198", "#EEE8D5", "#073642", "#CB4B16", "#586E75", "#657B83", "#839496", "#6C71C4", "#93A1A1", "#FDF6E3"],
    },
    Scheme {
        name: "Tango Dark",
        foreground: "#D3D7CF",
        background: "#000000",
        cursor: "#FFFFFF",
        palette: ["#000000", "#CC0000", "#4E9A06", "#C4A000", "#3465A4", "#75507B", "#06989A", "#D3D7CF", "#555753", "#EF2929", "#8AE234", "#FCE94F", "#729FCF", "#AD7FA8", "#34E2E2", "#EEEEEC"],
    },
    Scheme {
        name: "Tango Light",
        foreground: "#555753",
        background: "#FFFFFF",
        cursor: "#000000",
        palette: ["#000000", "#CC0000", "#4E9A06", "#C4A000", "#3465A4", "#75507B", "#06989A", "#D3D7CF", "#555753", "#EF2929", "#8AE234", "#FCE94F", "#729FCF", "#AD7FA8", "#34E2E2", "#EEEEEC"],
    },
    Scheme {
        name: "Dark+",
        foreground: "#cccccc",
        background: "#1e1e1e",
        cursor: "#808080",
        palette: ["#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5", "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#e5e5e5"],
    },
    Scheme {
        name: "VSCode Dark Modern",
        foreground: "#CCCCCC",
        background: "#1F1F1F",
        cursor: "#FFFFFF",
        palette: ["#000000", "#CD3131", "#0DBC79", "#E5E510", "#2472C8", "#BC3FBC", "#11A8CD", "#E5E5E5", "#666666", "#F14C4C", "#23D18B", "#F5F543", "#3B8EEA", "#D670D6", "#29B8DB", "#E5E5E5"],
    },
    Scheme {
        name: "VSCode Light Modern",
        foreground: "#3B3B3B",
        background: "#FFFFFF",
        cursor: "#000000",
        palette: ["#000000", "#CD3131", "#00BC00", "#949800", "#0451A5", "#BC05BC", "#0598BC", "#555555", "#666666", "#CD3131", "#14CE14", "#B5BA00", "#0451A5", "#BC05BC", "#0598BC", "#A5A5A5"],
    },
    Scheme {
        name: "CGA",
        foreground: "#AAAAAA",
        background: "#000000",
        cursor: "#00AA00",
        palette: ["#000000", "#AA0000", "#00AA00", "#AA5500", "#0000AA", "#AA00AA", "#00AAAA", "#AAAAAA", "#555555", "#FF5555", "#55FF55", "#FFFF55", "#5555FF", "#FF55FF", "#55FFFF", "#FFFFFF"],
    },
    Scheme {
        name: "IBM 5153",
        foreground: "#AAAAAA",
        background: "#000000",
        cursor: "#00AA00",
        palette: ["#000000", "#AA0000", "#00AA00", "#C47E00", "#0000AA", "#AA00AA", "#00AAAA", "#AAAAAA", "#555555", "#FF5555", "#55FF55", "#FFFF55", "#5555FF", "#FF55FF", "#55FFFF", "#FFFFFF"],
    },
];

/// A built-in scheme by name (any case).
pub fn scheme(name: &str) -> Option<&'static Scheme> {
    SCHEMES.iter().find(|s| s.name.eq_ignore_ascii_case(name.trim()))
}

/// The escape sequences that give the tab `scheme`'s colors (OSC 4 for the
/// 16 colors, 10 foreground, 11 background, 12 cursor), or put the
/// Terminal's own back (OSC 104, 110, 111, 112) for `None`.
pub fn osc(scheme: Option<&Scheme>) -> String {
    const ESC: char = '\u{1b}';
    const BEL: char = '\u{7}';
    match scheme {
        Some(s) => {
            let mut out: String = s.palette.iter().enumerate().map(|(i, c)| format!("{ESC}]4;{i};{c}{BEL}")).collect();
            out.push_str(&format!("{ESC}]10;{}{BEL}{ESC}]11;{}{BEL}{ESC}]12;{}{BEL}", s.foreground, s.background, s.cursor));
            out
        }
        None => format!("{ESC}]104{BEL}{ESC}]110{BEL}{ESC}]111{BEL}{ESC}]112{BEL}"),
    }
}

/// A tab color value as `#RRGGBB`: a preset name, `#RGB` or `#RRGGBB`.
/// `None` for `none`, empty or anything else.
pub fn tab_color(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some((_, hex)) = PRESETS.iter().find(|(name, _)| name.eq_ignore_ascii_case(value)) {
        return Some(hex.to_string());
    }
    let hex = value.strip_prefix('#')?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        6 => Some(format!("#{}", hex.to_ascii_uppercase())),
        3 => Some(format!("#{}", hex.chars().flat_map(|c| [c, c]).collect::<String>().to_ascii_uppercase())),
        _ => None,
    }
}

/// A color scheme value: a built-in scheme's name, else `None` (`none`,
/// or a scheme NativeTerm has no colors for).
pub fn color_scheme(value: &str) -> Option<String> {
    scheme(value).map(|s| s.name.to_string())
}

/// What a host's tabs look like.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    /// `#RRGGBB`.
    pub tab_color: Option<String>,
    pub color_scheme: Option<String>,
}

/// The host's own values, else its folder's.
pub fn for_host(folder: &Folder, host: &HostEntry) -> Appearance {
    Appearance {
        tab_color: folder.nt(host, TAB_COLOR).and_then(tab_color),
        color_scheme: folder.nt(host, COLOR_SCHEME).and_then(color_scheme),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(tab_color("red").as_deref(), Some("#C0392B"));
        assert_eq!(tab_color("#c0392b").as_deref(), Some("#C0392B"));
        assert_eq!(tab_color("#0a0").as_deref(), Some("#00AA00"));
        assert_eq!(tab_color("none"), None);
        assert_eq!(tab_color("#12345"), None);
        assert_eq!(tab_color("#xyzxyz"), None);
    }

    #[test]
    fn schemes() {
        assert_eq!(color_scheme(" one half dark ").as_deref(), Some("One Half Dark"));
        assert_eq!(color_scheme("none"), None);
        assert_eq!(color_scheme("My Own"), None, "no colors for it");
        let light = scheme("One Half Light").unwrap();
        assert_eq!(light.background, "#FAFAFA");
        let osc = osc(Some(light));
        assert!(osc.starts_with("\u{1b}]4;0;"), "{osc:?}");
        assert!(osc.contains("\u{1b}]11;#FAFAFA\u{7}"), "{osc:?}");
        assert_eq!(osc.matches("\u{1b}]4;").count(), 16);
        assert!(super::osc(None).contains("\u{1b}]104\u{7}"));
    }
}
