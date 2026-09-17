//! Which colors and text size NativeTerm's tab menu should use next to a
//! Windows Terminal: the Terminal's own `theme` setting first, then
//! Windows' app theme, high contrast, and text scaling.

use std::path::Path;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{
    GetSysColor, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

use super::jsonc;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub background: COLORREF,
    pub hover: COLORREF,
    pub text: COLORREF,
    pub hover_text: COLORREF,
    /// Secondary text (header rows, disabled items).
    pub dim: COLORREF,
    pub separator: COLORREF,
    pub border: COLORREF,
}

#[derive(Clone, Debug)]
pub struct Look {
    pub palette: Palette,
    /// "dark", "light" or "high contrast", with where it came from.
    pub description: String,
    /// Windows "Text size" (Accessibility), 1.0–2.25.
    pub text_scale: f32,
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

fn read_dword(key: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    status.is_ok().then_some(data)
}

fn windows_apps_dark() -> bool {
    read_dword(w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), w!("AppsUseLightTheme")) == Some(0)
}

fn text_scale() -> f32 {
    read_dword(w!(r"Software\Microsoft\Accessibility"), w!("TextScaleFactor"))
        .map(|v| v.clamp(100, 225) as f32 / 100.0)
        .unwrap_or(1.0)
}

fn high_contrast() -> bool {
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some(&mut hc as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && hc.dwFlags.contains(HCF_HIGHCONTRASTON)
}

/// The Terminal theme's name and its application theme ("light", "dark"
/// or "system"), as Windows Terminal resolves `theme`.
pub fn terminal_application_theme(settings: &Path, windows_dark: bool) -> Option<(String, String)> {
    let text = std::fs::read_to_string(settings).ok()?;
    let json = jsonc::parse(&text).ok()?;
    // defaults.json says "dark"
    let theme = json.get("theme").cloned().unwrap_or_else(|| "dark".into());
    // a name, or a {"dark": name, "light": name} pair picked by the OS theme
    let name = match &theme {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(pair) => pair.get(if windows_dark { "dark" } else { "light" })?.as_str()?.to_string(),
        _ => return None,
    };
    let user_theme = json
        .get("themes")
        .and_then(|t| t.as_array())
        .and_then(|list| list.iter().find(|t| t.get("name").and_then(|n| n.as_str()) == Some(name.as_str())))
        .and_then(|t| t.pointer("/window/applicationTheme"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let app = user_theme.unwrap_or_else(|| {
        match name.as_str() {
            "light" | "legacyLight" => "light",
            "dark" | "legacyDark" => "dark",
            _ => "system",
        }
        .to_string()
    });
    Some((name, app))
}

/// Read on every menu, so theme changes apply to the next one.
pub fn look(settings: &Path) -> Look {
    let text_scale = text_scale();
    if high_contrast() {
        let sys = |i| COLORREF(unsafe { GetSysColor(i) });
        return Look {
            palette: Palette {
                background: sys(COLOR_WINDOW),
                hover: sys(COLOR_HIGHLIGHT),
                text: sys(COLOR_WINDOWTEXT),
                hover_text: sys(COLOR_HIGHLIGHTTEXT),
                dim: sys(COLOR_GRAYTEXT),
                separator: sys(COLOR_WINDOWTEXT),
                border: sys(COLOR_WINDOWTEXT),
            },
            description: "high contrast (system colors)".into(),
            text_scale,
        };
    }
    let windows_dark = windows_apps_dark();
    let (dark, description) = match terminal_application_theme(settings, windows_dark) {
        Some((name, app)) if app == "light" => (false, format!("light (Terminal theme {name:?})")),
        Some((name, app)) if app == "dark" => (true, format!("dark (Terminal theme {name:?})")),
        Some((name, _)) => {
            (windows_dark, format!("{} (Terminal theme {name:?} follows Windows)", if windows_dark { "dark" } else { "light" }))
        }
        None => (windows_dark, format!("{} (Windows; Terminal settings unreadable)", if windows_dark { "dark" } else { "light" })),
    };
    // WinUI flyout colors (acrylic approximated by its solid fallback)
    let palette = if dark {
        Palette {
            background: rgb(0x2c, 0x2c, 0x2c),
            hover: rgb(0x3a, 0x3a, 0x3a),
            text: rgb(0xff, 0xff, 0xff),
            hover_text: rgb(0xff, 0xff, 0xff),
            dim: rgb(0x9e, 0x9e, 0x9e),
            separator: rgb(0x40, 0x40, 0x40),
            border: rgb(0x45, 0x45, 0x45),
        }
    } else {
        Palette {
            background: rgb(0xf9, 0xf9, 0xf9),
            hover: rgb(0xea, 0xea, 0xea),
            text: rgb(0x1b, 0x1b, 0x1b),
            hover_text: rgb(0x1b, 0x1b, 0x1b),
            dim: rgb(0x6e, 0x6e, 0x6e),
            separator: rgb(0xe5, 0xe5, 0xe5),
            border: rgb(0xe0, 0xe0, 0xe0),
        }
    };
    Look { palette, description, text_scale }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("settings.json");
        let check = |text: &str, windows_dark: bool| {
            std::fs::write(&file, text).unwrap();
            terminal_application_theme(&file, windows_dark).map(|(_, app)| app)
        };
        assert_eq!(check("{}", false).as_deref(), Some("dark"), "defaults.json");
        assert_eq!(check(r#"{"theme": "light"}"#, true).as_deref(), Some("light"));
        assert_eq!(check(r#"{"theme": "system"}"#, true).as_deref(), Some("system"));
        let pair = r#"{"theme": {"dark": "dark", "light": "mine"},
            // a custom theme
            "themes": [{"name": "mine", "window": {"applicationTheme": "light"}},]}"#;
        assert_eq!(check(pair, false).as_deref(), Some("light"));
        assert_eq!(check(pair, true).as_deref(), Some("dark"));
        assert_eq!(check("not json", true), None);
    }
}
