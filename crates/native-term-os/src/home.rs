//! The user's folders: home, `.ssh`, and where a program keeps its data.

use std::path::PathBuf;

/// The user's home folder (`USERPROFILE`; `HOME` elsewhere).
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// `~/.ssh`.
#[must_use]
pub fn ssh_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".ssh"))
}

/// What `app_data` reads, as the settings show where the data folder
/// came from.
pub const APP_DATA_SOURCE: &str = if cfg!(windows) { "%APPDATA%" } else { "$XDG_DATA_HOME" };

/// Where programs keep a user's data: `%APPDATA%`, or `$XDG_DATA_HOME`
/// (`~/.local/share`, and `~/Library/Application Support` on macOS).
#[must_use]
pub fn app_data() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").filter(|v| !v.is_empty()).map(PathBuf::from)
    }
    #[cfg(unix)]
    {
        if let Some(dir) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(dir));
        }
        let home = home_dir()?;
        Some(if cfg!(target_os = "macos") {
            home.join("Library").join("Application Support")
        } else {
            home.join(".local").join("share")
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn folders_are_absolute() {
        let home = super::home_dir().expect("a home");
        assert!(home.is_absolute());
        assert_eq!(super::ssh_dir().unwrap(), home.join(".ssh"));
        assert!(super::app_data().expect("an app data folder").is_absolute());
    }
}
