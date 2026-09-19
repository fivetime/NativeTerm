//! The "NativeTerm SSH" profile, installed as a Windows Terminal JSON
//! fragment (see "Terminal tabs" in `docs/ARCHITECTURE.md`).

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{json, Value};

use super::command::{quote, PROFILE_NAME};
use super::install::Install;
use super::jsonc;

/// Fixed, so saved layouts keep pointing at the profile.
pub const PROFILE_GUID: &str = "{2b0f6c9e-7d1a-4c55-9a39-5d0c1f0e7a11}";
/// Scrollback lines per NativeTerm tab (Terminal's default is 9001).
pub const HISTORY_SIZE: u32 = 5000;
/// Folder name under `Fragments`, shown as the extension's source.
pub const SOURCE: &str = "NativeTerm";
const FILE_NAME: &str = "nativeterm.json";

pub fn profile(shim: &Path) -> Value {
    json!({
        "guid": PROFILE_GUID,
        "name": PROFILE_NAME,
        // restored and duplicated panes run this: the shim without a host
        "commandline": quote_always(&shim.to_string_lossy()),
        "suppressApplicationTitle": true,
        // exit 0 closes the tab, whatever the user's default
        "closeOnExit": "automatic",
        "historySize": HISTORY_SIZE,
    })
}

fn quote_always(path: &str) -> String {
    let quoted = quote(path);
    if quoted.starts_with('"') {
        quoted
    } else {
        format!("\"{quoted}\"")
    }
}

pub fn fragment(shim: &Path) -> String {
    let mut text = serde_json::to_string_pretty(&json!({ "profiles": [profile(shim)] })).expect("static JSON");
    text.push('\n');
    text
}

/// Overrides the fragments folder (tests).
pub const FRAGMENTS_ENV: &str = "NATIVETERM_FRAGMENTS_DIR";

/// Where NativeTerm's fragment goes: `FRAGMENTS_ENV`, else the user's.
pub fn fragments_root() -> Option<PathBuf> {
    std::env::var_os(FRAGMENTS_ENV).map(PathBuf::from).or_else(user_fragments_root)
}

/// Whether a Terminal will show the "NativeTerm SSH" profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The fragment is installed with this shim.
    Installed,
    /// Defined in the Terminal's own `settings.json` (by hand, or tests).
    InSettings,
    /// The fragment points at another shim (the program folder moved).
    Outdated {
        shim: PathBuf,
    },
    /// Installed, but turned off on Terminal's "Extensions" page.
    Disabled,
    Missing,
}

impl Status {
    pub fn usable(&self) -> bool {
        matches!(self, Status::Installed | Status::InSettings)
    }
}

pub fn status(install: &Install, root: Option<&Path>, shim: &Path) -> Status {
    let settings = fs::read_to_string(install.settings_json()).ok().and_then(|t| jsonc::parse(&t).ok());
    let disabled = settings
        .as_ref()
        .and_then(|s| s.get("disabledProfileSources"))
        .and_then(Value::as_array)
        .is_some_and(|list| list.iter().any(|v| v.as_str() == Some(SOURCE)));
    let fragment = root.and_then(installed_shim);
    match &fragment {
        Some(_) if disabled => return Status::Disabled,
        Some(installed) if same_path(installed, shim) => return Status::Installed,
        _ => {}
    }
    let own =
        settings.as_ref().and_then(|s| s.pointer("/profiles/list")).and_then(Value::as_array).is_some_and(|list| {
            list.iter().any(|p| {
                p.get("name").and_then(Value::as_str) == Some(PROFILE_NAME)
                    && p.get("source").is_none()
                    && p.get("hidden").and_then(Value::as_bool) != Some(true)
            })
        });
    match fragment {
        _ if own => Status::InSettings,
        Some(installed) => Status::Outdated { shim: installed },
        None => Status::Missing,
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// `%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments` (read by
/// packaged, unpackaged and portable installs alike).
pub fn user_fragments_root() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|d| PathBuf::from(d).join("Microsoft").join("Windows Terminal").join("Fragments"))
}

pub fn fragment_path(root: &Path) -> PathBuf {
    root.join(SOURCE).join(FILE_NAME)
}

/// The shim path in an installed fragment, to notice a moved program folder.
pub fn installed_shim(root: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(fragment_path(root)).ok()?;
    // edited by hand or other tools: a BOM or comments are possible
    let value = jsonc::parse(&text).ok()?;
    let line = value.get("profiles")?.get(0)?.get("commandline")?.as_str()?;
    Some(PathBuf::from(line.trim_matches('"')))
}

/// Write the fragment if it differs, then touch each `settings.json` so
/// the Terminals reload (they only watch that file). Returns whether
/// anything changed.
pub fn install(root: &Path, shim: &Path, settings_files: &[PathBuf]) -> io::Result<bool> {
    let path = fragment_path(root);
    let text = fragment(shim);
    if fs::read_to_string(&path).is_ok_and(|current| current == text) {
        return Ok(false);
    }
    fs::create_dir_all(path.parent().expect("fragment folder"))?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, &text)?;
    fs::rename(&temp, &path)?;
    touch(settings_files);
    Ok(true)
}

pub fn uninstall(root: &Path, settings_files: &[PathBuf]) -> io::Result<()> {
    let folder = root.join(SOURCE);
    if folder.exists() {
        fs::remove_dir_all(folder)?;
        touch(settings_files);
    }
    Ok(())
}

fn touch(files: &[PathBuf]) {
    for file in files {
        if let Ok(f) = File::options().write(true).open(file) {
            let _ = f.set_modified(SystemTime::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_content() {
        let shim = Path::new(r"C:\Program Files\NativeTerm\nativeterm-shim.exe");
        let value: Value = serde_json::from_str(&fragment(shim)).unwrap();
        let p = &value["profiles"][0];
        assert_eq!(p["name"], "NativeTerm SSH");
        assert_eq!(p["commandline"], r#""C:\Program Files\NativeTerm\nativeterm-shim.exe""#);
        assert_eq!(p["suppressApplicationTitle"], true);
        assert_eq!(p["closeOnExit"], "automatic");
        let plain = profile(Path::new(r"C:\NativeTerm\nativeterm-shim.exe"));
        assert_eq!(plain["commandline"], r#""C:\NativeTerm\nativeterm-shim.exe""#);
    }

    fn portable(dir: &Path, settings: &str) -> Install {
        fs::write(dir.join("WindowsTerminal.exe"), "").unwrap();
        fs::write(dir.join("wt.exe"), "").unwrap();
        fs::write(dir.join(".portable"), "").unwrap();
        fs::create_dir_all(dir.join("settings")).unwrap();
        fs::write(dir.join("settings").join("settings.json"), settings).unwrap();
        Install::from_dir(dir).unwrap()
    }

    #[test]
    fn status_of_fragment_and_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Fragments");
        let shim = Path::new(r"C:\NativeTerm\nativeterm-shim.exe");
        let term = tmp.path().join("wt");
        fs::create_dir(&term).unwrap();
        let commented = "{\n  // user settings\n  \"profiles\": {\"list\": [{\"name\": \"cmd\"},]},\n}";
        let install = portable(&term, commented);
        assert_eq!(status(&install, Some(&root), shim), Status::Missing);
        assert_eq!(status(&install, None, shim), Status::Missing);

        super::install(&root, shim, &[]).unwrap();
        assert_eq!(status(&install, Some(&root), shim), Status::Installed);
        assert!(status(&install, Some(&root), shim).usable());
        let moved = Path::new(r"D:\Tools\nativeterm-shim.exe");
        assert_eq!(status(&install, Some(&root), moved), Status::Outdated { shim: shim.to_path_buf() });

        let install = portable(&term, r#"{"disabledProfileSources": ["NativeTerm"], "profiles": {"list": []}}"#);
        assert_eq!(status(&install, Some(&root), shim), Status::Disabled);

        let own = r#"{"profiles": {"list": [{"name": "NativeTerm SSH", "commandline": "x"}]}}"#;
        let install = portable(&term, own);
        assert_eq!(status(&install, None, shim), Status::InSettings);
        assert_eq!(status(&install, Some(&root), moved), Status::InSettings);
        // the stub Terminal leaves for a removed fragment profile doesn't count
        let stub = r#"{"profiles": {"list": [{"name": "NativeTerm SSH", "source": "NativeTerm", "hidden": true}]}}"#;
        let install = portable(&term, stub);
        assert_eq!(status(&install, None, shim), Status::Missing);
    }

    #[test]
    fn hand_edited_fragment_with_bom() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(SOURCE)).unwrap();
        let text = "\u{feff}{\"profiles\": [{\"name\": \"NativeTerm SSH\", \"commandline\": \"\\\"D:\\\\Old\\\\nativeterm-shim.exe\\\"\"}]}";
        fs::write(fragment_path(tmp.path()), text).unwrap();
        assert_eq!(installed_shim(tmp.path()), Some(PathBuf::from(r"D:\Old\nativeterm-shim.exe")));
    }

    #[test]
    fn install_touches_settings_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Fragments");
        let settings = tmp.path().join("settings.json");
        fs::write(&settings, "{}").unwrap();
        let old = SystemTime::now() - std::time::Duration::from_secs(3600);
        File::options().write(true).open(&settings).unwrap().set_modified(old).unwrap();

        let shim = Path::new(r"D:\Tools\NativeTerm\nativeterm-shim.exe");
        assert!(install(&root, shim, std::slice::from_ref(&settings)).unwrap());
        assert_eq!(installed_shim(&root).as_deref(), Some(shim));
        let touched = fs::metadata(&settings).unwrap().modified().unwrap();
        assert!(touched > old + std::time::Duration::from_secs(60));
        assert_eq!(fs::read_to_string(&settings).unwrap(), "{}", "content untouched");
        assert!(!install(&root, shim, &[]).unwrap(), "unchanged");

        let moved = Path::new(r"E:\NativeTerm\nativeterm-shim.exe");
        assert!(install(&root, moved, &[]).unwrap());
        assert_eq!(installed_shim(&root).as_deref(), Some(moved));

        uninstall(&root, &[]).unwrap();
        assert!(!root.join(SOURCE).exists());
        assert_eq!(installed_shim(&root), None);
    }
}
