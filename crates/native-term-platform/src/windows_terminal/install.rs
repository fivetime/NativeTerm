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

/// The oldest Windows Terminal NativeTerm works with.
///
/// A tab is opened with `--sessionId {guid}` and found again by it
/// (`WT_SESSION` in the tab). Terminal learned that flag with its buffer
/// restore (microsoft/terminal#16598, first released in 1.21); older
/// versions open the tab but NativeTerm cannot tell which one it is.
pub const OLDEST: (u16, u16) = (1, 21);

/// A version as Terminal states it, `major.minor.build.revision`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u16, pub u16, pub u16, pub u16);

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0, self.1, self.2, self.3)
    }
}

impl Version {
    /// The version in a package full name
    /// (`Microsoft.WindowsTerminal_1.21.3231.0_x64__8wekyb3d8bbwe`).
    #[must_use]
    pub fn from_package_name(full_name: &str) -> Option<Version> {
        let field = full_name.split('_').nth(1)?;
        Version::parse(field)
    }

    /// `1.21.3231.0`, as a file version or a package name states it.
    #[must_use]
    pub fn parse(text: &str) -> Option<Version> {
        let mut parts = text.split('.');
        let major = parts.next()?.parse::<u16>().ok()?;
        let mut next = || parts.next().and_then(|p| p.parse::<u16>().ok()).unwrap_or(0);
        Some(Version(major, next(), next(), next()))
    }

    /// Whether NativeTerm can find its tabs in this Terminal.
    #[must_use]
    pub fn old(&self) -> bool {
        (self.0, self.1) < OLDEST
    }
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
    /// What it says its version is (a package's full name, or the
    /// `WindowsTerminal.exe` file version); `None` when it could not be
    /// read.
    pub version: Option<Version>,
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
        let version = file_version(&dir.join("WindowsTerminal.exe"));
        Ok(Install { launcher: dir.join("wt.exe"), dir, kind, settings_dir, family: None, version })
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
            version: Version::from_package_name(&full_name),
        }))
    }

    /// Installed packages plus the given unpackaged folders that exist.
    pub fn discover(folders: &[PathBuf]) -> Vec<Install> {
        let mut found: Vec<Install> = FAMILIES.iter().filter_map(|f| Install::packaged(f).ok().flatten()).collect();
        found.extend(folders.iter().filter_map(|d| Install::from_dir(d).ok()));
        found
    }

    /// Too old for NativeTerm to find its tabs (see `OLDEST`). Unknown
    /// versions are given the benefit of the doubt.
    #[must_use]
    pub fn too_old(&self) -> bool {
        self.version.is_some_and(|v| v.old())
    }

    /// What starts Terminal now, if anything does.
    ///
    /// A packaged install is normally started through its app execution
    /// alias, which Windows lets the person turn off (Settings 鈫?Apps 鈫?    /// Advanced app settings 鈫?App execution aliases) and which some
    /// "debloat" scripts remove. The alias only points at `wt.exe` inside
    /// the package, and that `wt.exe` is a shim that runs the
    /// `WindowsTerminal.exe` next to it (terminal/src/cascadia/wt/shim.cpp),
    /// so the copy the package info leads to does the same thing. `wt.exe`
    /// on the PATH (scoop, a portable folder someone added) is the last
    /// resort.
    #[must_use]
    pub fn launcher_now(&self) -> Option<PathBuf> {
        if self.launcher.is_file() {
            return Some(self.launcher.clone());
        }
        let in_package = self.dir.join("wt.exe");
        if in_package.is_file() {
            return Some(in_package);
        }
        on_path("wt.exe")
    }

    /// Whether the alias a packaged install is normally started through is
    /// gone (`launcher_now` then finds it another way).
    #[must_use]
    pub fn alias_off(&self) -> bool {
        self.kind == Kind::Packaged && !self.launcher.is_file()
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

/// `name` in one of the PATH folders.
fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|p| p.is_file())
}

/// The file version of `path` (`WindowsTerminal.exe` states Terminal's).
fn file_version(path: &Path) -> Option<Version> {
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let wide = HSTRING::from(path.as_os_str());
    // SAFETY: the string lives through the calls; the buffer is as large
    // as the first call asks for, and the pointer VerQueryValue hands back
    // is read only while the buffer is alive.
    unsafe {
        let size = GetFileVersionInfoSizeW(&wide, None);
        if size == 0 {
            return None;
        }
        let mut buffer = vec![0u8; size as usize];
        GetFileVersionInfoW(&wide, None, size, buffer.as_mut_ptr().cast()).ok()?;
        let mut value = std::ptr::null_mut();
        let mut length = 0u32;
        let root = HSTRING::from("\\");
        if !VerQueryValueW(buffer.as_ptr().cast(), &root, &mut value, &mut length).as_bool() || length == 0 {
            return None;
        }
        let info = &*value.cast::<windows::Win32::Storage::FileSystem::VS_FIXEDFILEINFO>();
        Some(Version(
            (info.dwFileVersionMS >> 16) as u16,
            (info.dwFileVersionMS & 0xffff) as u16,
            (info.dwFileVersionLS >> 16) as u16,
            (info.dwFileVersionLS & 0xffff) as u16,
        ))
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
    fn versions_are_read_and_compared() {
        assert_eq!(Version::parse("1.21.3231.0"), Some(Version(1, 21, 3231, 0)));
        assert_eq!(Version::parse("1.22"), Some(Version(1, 22, 0, 0)), "a short version is still a version");
        assert_eq!(Version::parse(""), None);
        assert_eq!(Version::parse("preview"), None);
        assert_eq!(
            Version::from_package_name("Microsoft.WindowsTerminal_1.21.3231.0_x64__8wekyb3d8bbwe"),
            Some(Version(1, 21, 3231, 0))
        );
        assert_eq!(Version::from_package_name("Microsoft.WindowsTerminal"), None);
        assert!(Version(1, 20, 11781, 0).old(), "1.20 has no --sessionId");
        assert!(!Version(1, 21, 0, 0).old(), "the oldest we work with");
        assert!(!Version(1, 26, 2581, 0).old());
        assert!(Version(1, 9, 0, 0) < Version(1, 21, 0, 0), "minor versions are numbers, not text");
        assert_eq!(Version(1, 21, 3231, 0).to_string(), "1.21.3231.0");
    }

    #[test]
    fn a_missing_launcher_is_looked_for_in_the_package_folder() {
        // the app execution alias is off: the package's own wt.exe does the
        // same thing (it starts the WindowsTerminal.exe next to it)
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("package");
        std::fs::create_dir(&dir).unwrap();
        fake_install(&dir, false);
        let install = Install {
            dir: dir.clone(),
            kind: Kind::Packaged,
            launcher: tmp.path().join("WindowsApps").join("family").join("wt.exe"),
            settings_dir: tmp.path().join("LocalState"),
            family: Some("family".to_string()),
            version: Some(Version(1, 26, 0, 0)),
        };
        assert!(install.alias_off());
        assert_eq!(install.launcher_now(), Some(dir.join("wt.exe")));
        assert!(!install.too_old());
        // with the alias there, that is what is used
        std::fs::create_dir_all(install.launcher.parent().unwrap()).unwrap();
        std::fs::write(&install.launcher, "").unwrap();
        assert!(!install.alias_off());
        assert_eq!(install.launcher_now(), Some(install.launcher.clone()));
    }

    #[test]
    fn an_unknown_version_is_not_called_old() {
        let tmp = tempfile::tempdir().unwrap();
        fake_install(tmp.path(), true);
        let install = Install::from_dir(tmp.path()).unwrap();
        assert_eq!(install.version, None, "an empty file has no version resource");
        assert!(!install.too_old(), "what we cannot read, we do not complain about");
        assert!(!install.alias_off(), "only a packaged install has an alias");
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
        std::fs::write(install.state_json(false), r#"{"persistedWorkspaces": {"nativeterm": {"tabLayout": []}}}"#)
            .unwrap();
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
