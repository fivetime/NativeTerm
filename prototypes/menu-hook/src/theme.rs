//! Which colors and text size a menu next to a given Windows Terminal window
//! should use: the Terminal's own `theme` setting first, then Windows'
//! app theme, high contrast, and text scaling.

use std::path::{Path, PathBuf};

use windows::core::{w, PWSTR};
use windows::Win32::Foundation::{CloseHandle, COLORREF, HWND};
use windows::Win32::Graphics::Gdi::{
    GetSysColor, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowThreadProcessId, SystemParametersInfoW, SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

pub struct Palette {
    pub background: COLORREF,
    pub hover: COLORREF,
    pub text: COLORREF,
    pub hover_text: COLORREF,
    /// secondary text (header rows)
    pub dim: COLORREF,
    pub separator: COLORREF,
    pub border: COLORREF,
}

pub struct Look {
    pub palette: Palette,
    /// "dark", "light", or "high contrast", with where it came from
    pub description: String,
    /// Windows "Text size" (Accessibility), 1.0–2.25
    pub text_scale: f32,
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

fn read_dword(key: windows::core::PCWSTR, value: windows::core::PCWSTR) -> Option<u32> {
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

pub fn process_image(window: HWND) -> Option<PathBuf> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, Some(&mut pid));
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        result.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// settings.json of the Terminal install that owns `window`
/// (packaged, portable, or unpackaged — see FileUtils.cpp in the Terminal source).
pub fn settings_path(window: HWND) -> Option<PathBuf> {
    let exe = process_image(window)?;
    let dir = exe.parent()?;
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let dir_name = dir.file_name()?.to_string_lossy().to_string();
    if dir.parent().and_then(Path::file_name).is_some_and(|n| n.eq_ignore_ascii_case("WindowsApps")) {
        // Microsoft.WindowsTerminal_1.24.11911.0_x64__8wekyb3d8bbwe -> Microsoft.WindowsTerminal_8wekyb3d8bbwe
        let (name_part, publisher) = dir_name.split_once("__")?;
        let name = name_part.split('_').next()?;
        return Some(local.join("Packages").join(format!("{name}_{publisher}")).join(r"LocalState\settings.json"));
    }
    if dir.join(".portable").exists() {
        return Some(dir.join(r"settings\settings.json"));
    }
    Some(local.join(r"Microsoft\Windows Terminal\settings.json"))
}

/// settings.json allows comments (JSONC); strip them outside strings.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// "light", "dark", or "system" as Windows Terminal resolves its `theme`.
fn terminal_application_theme(settings: &Path, windows_dark: bool) -> Option<(String, String)> {
    let text = std::fs::read_to_string(settings).ok()?;
    let json: serde_json::Value = serde_json::from_str(&strip_comments(&text)).ok()?;
    // default from defaults.json
    let theme = json.get("theme").cloned().unwrap_or_else(|| "dark".into());
    // a string, or a {"dark": name, "light": name} pair picked by the OS theme
    let name = match &theme {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(pair) => {
            let key = if windows_dark { "dark" } else { "light" };
            pair.get(key)?.as_str()?.to_string()
        }
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

pub fn look_for(window: HWND) -> Look {
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
    let path = settings_path(window);
    let resolved = path.as_deref().and_then(|p| terminal_application_theme(p, windows_dark));
    let (dark, description) = match &resolved {
        Some((name, app)) if app == "light" => (false, format!("light (Terminal theme {name:?})")),
        Some((name, app)) if app == "dark" => (true, format!("dark (Terminal theme {name:?})")),
        Some((name, _)) => (windows_dark, format!("{} (Terminal theme {name:?} follows Windows)", if windows_dark { "dark" } else { "light" })),
        None => (windows_dark, format!("{} (Windows; Terminal settings unreadable: {path:?})", if windows_dark { "dark" } else { "light" })),
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
