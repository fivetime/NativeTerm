//! The desktop's shell: opening a file with its program, the wastebasket,
//! the drives and the user's folders.

#[cfg(windows)]
pub use native_term_win::shell::{downloads_folder, drives, open_file, recycle, user_folders};

#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};

    /// Opens a file with its program (`open` on macOS, `xdg-open` on
    /// desktops that have it).
    pub fn open_file(path: &Path) -> std::io::Result<()> {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let status = std::process::Command::new(opener).arg(path).status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("{opener} failed ({status})")))
        }
    }

    /// The wastebasket: nothing yet on this system (a future `trash`).
    pub fn recycle(paths: &[PathBuf]) -> std::io::Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        Err(crate::unsupported("moving to the wastebasket"))
    }

    /// Where file systems start: the root, and where removable ones are
    /// mounted.
    pub fn drives() -> Vec<PathBuf> {
        let mut roots = vec![PathBuf::from("/")];
        for mounts in ["/Volumes", "/media", "/mnt", "/run/media"] {
            let dir = PathBuf::from(mounts);
            if dir.is_dir() {
                roots.push(dir);
            }
        }
        roots
    }

    fn home() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_dir())
    }

    /// A folder named in `~/.config/user-dirs.dirs` (`XDG_DOWNLOAD_DIR=
    /// "$HOME/Downloads"`), if the desktop keeps one.
    fn xdg_user_dir(key: &str) -> Option<PathBuf> {
        let home = home()?;
        let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        let text = std::fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
        let value = text.lines().find_map(|l| l.trim().strip_prefix(key)?.trim().strip_prefix('='))?;
        let value = value.trim().trim_matches('"');
        let path = match value.strip_prefix("$HOME/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(value),
        };
        path.is_dir().then_some(path)
    }

    fn user_folder(xdg: &str, name: &str) -> Option<PathBuf> {
        xdg_user_dir(xdg).or_else(|| home().map(|h| h.join(name)).filter(|d| d.is_dir()))
    }

    /// The user's Downloads folder.
    pub fn downloads_folder() -> Option<PathBuf> {
        user_folder("XDG_DOWNLOAD_DIR", "Downloads")
    }

    /// The user's Desktop, Documents and Downloads folders (the ones there).
    pub fn user_folders() -> Vec<PathBuf> {
        [("XDG_DESKTOP_DIR", "Desktop"), ("XDG_DOCUMENTS_DIR", "Documents"), ("XDG_DOWNLOAD_DIR", "Downloads")]
            .iter()
            .filter_map(|(xdg, name)| user_folder(xdg, name))
            .collect()
    }
}

#[cfg(unix)]
pub use unix::{downloads_folder, drives, open_file, recycle, user_folders};

#[cfg(test)]
mod tests {
    #[test]
    fn a_drive_to_start_from() {
        let drives = super::drives();
        assert!(!drives.is_empty());
        assert!(drives.iter().all(|d| d.is_absolute()), "{drives:?}");
    }

    #[test]
    fn user_folders_exist() {
        for d in super::user_folders() {
            assert!(d.is_dir(), "{}", d.display());
        }
    }
}
