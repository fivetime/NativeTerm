//! A Windows Terminal installation: where it lives, how to launch it, and
//! where its settings are. Each install is its own single-instance app
//! (see "Multiple Windows Terminal installs"), so everything is per install.

use std::io;
use std::path::{Path, PathBuf};

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
use windows::Win32::Storage::Packaging::Appx::{GetPackagePathByFullName, GetPackagesByPackageFamily};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Store / MSIX package.
    Packaged,
    /// Unpackaged ZIP with a `.portable` marker: settings next to it.
    Portable,
    /// Unpackaged without the marker (e.g. scoop).
    Unpackaged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    /// Folder of `WindowsTerminal.exe`.
    pub dir: PathBuf,
    pub kind: Kind,
    /// What NativeTerm runs: `wt.exe` in the folder, or a package's app
    /// execution alias (packaged executables can't be started by path).
    pub launcher: PathBuf,
    pub settings_dir: PathBuf,
    /// Package family name, for packaged installs.
    pub family: Option<String>,
}

/// Package families of Windows Terminal (release, Preview, Canary).
pub const FAMILIES: &[&str] = &[
    "Microsoft.WindowsTerminal_8wekyb3d8bbwe",
    "Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe",
    "Microsoft.WindowsTerminalCanary_8wekyb3d8bbwe",
];

fn local_app_data() -> io::Result<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not set"))
}

impl Install {
    /// An unpackaged install in `dir`.
    pub fn from_dir(dir: &Path) -> io::Result<Install> {
        let dir = std::path::absolute(dir)?;
        for exe in ["WindowsTerminal.exe", "wt.exe"] {
            if !dir.join(exe).is_file() {
                return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} has no {exe}", dir.display())));
            }
        }
        let (kind, settings_dir) = if dir.join(".portable").exists() {
            (Kind::Portable, dir.join("settings"))
        } else {
            (Kind::Unpackaged, local_app_data()?.join("Microsoft").join("Windows Terminal"))
        };
        Ok(Install { launcher: dir.join("wt.exe"), dir, kind, settings_dir, family: None })
    }

    /// The installed package of `family`, if any.
    pub fn packaged(family: &str) -> io::Result<Option<Install>> {
        let Some(full_name) = package_full_name(family)? else { return Ok(None) };
        let dir = package_path(&full_name)?;
        let local = local_app_data()?;
        Ok(Some(Install {
            dir,
            kind: Kind::Packaged,
            launcher: local.join("Microsoft").join("WindowsApps").join(family).join("wt.exe"),
            settings_dir: local.join("Packages").join(family).join("LocalState"),
            family: Some(family.to_string()),
        }))
    }

    /// Installed packages plus the given unpackaged folders that exist.
    pub fn discover(folders: &[PathBuf]) -> Vec<Install> {
        let mut found: Vec<Install> = FAMILIES.iter().filter_map(|f| Install::packaged(f).ok().flatten()).collect();
        found.extend(folders.iter().filter_map(|d| Install::from_dir(d).ok()));
        found
    }

    pub fn settings_json(&self) -> PathBuf {
        self.settings_dir.join("settings.json")
    }

    /// `state.json`, or `elevated-state.json` for the elevated instance.
    pub fn state_json(&self, elevated: bool) -> PathBuf {
        self.settings_dir.join(if elevated { "elevated-state.json" } else { "state.json" })
    }

    /// Whether a process image belongs to this install.
    pub fn owns_image(&self, image: &Path) -> bool {
        let dir = self.dir.to_string_lossy().to_lowercase();
        let image = image.to_string_lossy().to_lowercase();
        image.strip_prefix(dir.trim_end_matches('\\')).is_some_and(|rest| rest.starts_with('\\'))
    }

    /// Whether Terminal has a saved workspace under this window name, which
    /// a `wt -w <name>` would restore instead of running the command.
    pub fn has_saved_workspace(&self, name: &str, elevated: bool) -> bool {
        let Ok(text) = std::fs::read_to_string(self.state_json(elevated)) else { return false };
        let Ok(state) = serde_json::from_str::<serde_json::Value>(&text) else { return false };
        state
            .get("persistedWorkspaces")
            .and_then(|w| w.as_object())
            .is_some_and(|w| w.keys().any(|k| k.eq_ignore_ascii_case(name)))
    }
}

fn package_full_name(family: &str) -> io::Result<Option<String>> {
    let family = HSTRING::from(family);
    let (mut count, mut length) = (0u32, 0u32);
    let status = unsafe { GetPackagesByPackageFamily(&family, &mut count, None, &mut length, None) };
    if count == 0 {
        return Ok(None);
    }
    if status != ERROR_INSUFFICIENT_BUFFER && status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status.0 as i32));
    }
    let mut names = vec![PWSTR::null(); count as usize];
    let mut buffer = vec![0u16; length as usize];
    let status = unsafe {
        GetPackagesByPackageFamily(
            &family,
            &mut count,
            Some(names.as_mut_ptr()),
            &mut length,
            Some(PWSTR(buffer.as_mut_ptr())),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status.0 as i32));
    }
    Ok(names.first().and_then(|n| unsafe { n.to_string() }.ok()))
}

fn package_path(full_name: &str) -> io::Result<PathBuf> {
    let name = HSTRING::from(full_name);
    let mut length = 0u32;
    let _ = unsafe { GetPackagePathByFullName(&name, &mut length, None) };
    let mut buffer = vec![0u16; length as usize];
    let status = unsafe { GetPackagePathByFullName(&name, &mut length, Some(PWSTR(buffer.as_mut_ptr()))) };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status.0 as i32));
    }
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..end])))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_install(dir: &Path, portable: bool) {
        std::fs::write(dir.join("WindowsTerminal.exe"), "").unwrap();
        std::fs::write(dir.join("wt.exe"), "").unwrap();
        if portable {
            std::fs::write(dir.join(".portable"), "").unwrap();
        }
    }

    #[test]
    fn portable_install_and_image_ownership() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("terminal-1.26");
        std::fs::create_dir(&dir).unwrap();
        fake_install(&dir, true);
        let install = Install::from_dir(&dir).unwrap();
        assert_eq!(install.kind, Kind::Portable);
        assert_eq!(install.settings_json(), dir.join("settings").join("settings.json"));
        assert_eq!(install.launcher, dir.join("wt.exe"));
        let upper = PathBuf::from(dir.to_string_lossy().to_uppercase()).join("WINDOWSTERMINAL.EXE");
        assert!(install.owns_image(&upper));
        let sibling = PathBuf::from(format!("{}-other", dir.display())).join("WindowsTerminal.exe");
        assert!(!install.owns_image(&sibling), "a folder with the same prefix is another install");
    }

    #[test]
    fn unpackaged_without_marker_and_missing_exe() {
        let tmp = tempfile::tempdir().unwrap();
        fake_install(tmp.path(), false);
        assert_eq!(Install::from_dir(tmp.path()).unwrap().kind, Kind::Unpackaged);
        let empty = tempfile::tempdir().unwrap();
        assert!(Install::from_dir(empty.path()).is_err());
    }

    #[test]
    fn saved_workspaces() {
        let tmp = tempfile::tempdir().unwrap();
        fake_install(tmp.path(), true);
        let install = Install::from_dir(tmp.path()).unwrap();
        assert!(!install.has_saved_workspace("NativeTerm", false), "no state file");
        std::fs::create_dir(&install.settings_dir).unwrap();
        std::fs::write(install.state_json(false), r#"{"persistedWorkspaces": {"nativeterm": {"tabLayout": []}}}"#).unwrap();
        assert!(install.has_saved_workspace("NativeTerm", false));
        assert!(!install.has_saved_workspace("other", false));
        assert!(!install.has_saved_workspace("NativeTerm", true), "elevated state is separate");
    }

    #[test]
    fn packaged_lookup_does_not_fail() {
        // read-only: whatever is installed on this machine
        for family in FAMILIES {
            if let Some(install) = Install::packaged(family).unwrap() {
                assert!(install.dir.join("WindowsTerminal.exe").exists(), "{}", install.dir.display());
                assert_eq!(install.kind, Kind::Packaged);
            }
        }
        assert!(Install::packaged("NoSuch.Package_0000000000000").unwrap().is_none());
    }
}
