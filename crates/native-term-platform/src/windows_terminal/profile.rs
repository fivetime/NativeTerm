//! The "NativeTerm SSH" profile, installed as a Windows Terminal JSON
//! fragment (see "Terminal tabs" in `docs/ARCHITECTURE.md`).

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{json, Value};

use super::command::{quote, PROFILE_NAME};

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

/// `%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments` (read by
/// packaged, unpackaged and portable installs alike).
pub fn user_fragments_root() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Microsoft").join("Windows Terminal").join("Fragments"))
}

pub fn fragment_path(root: &Path) -> PathBuf {
    root.join(SOURCE).join(FILE_NAME)
}

/// The shim path in an installed fragment, to notice a moved program folder.
pub fn installed_shim(root: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(fragment_path(root)).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
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
