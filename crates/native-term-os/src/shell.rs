//! The desktop's shell: opening a file with its program, the wastebasket,
//! the drives and the user's folders.

use std::path::Path;

#[cfg(windows)]
pub use native_term_win::shell::{disk_space, downloads_folder, drives, open_file, recycle, user_folders};

/// A program started and left to run: waited for off this thread, so it
/// isn't left a zombie once it ends (`xdg-open` ends at once).
pub fn reaped(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// What opens a file or folder with the desktop's program: GIO's `gio
/// open` where it is (it asks the desktop's own MIME settings), else
/// `xdg-open`. (`xdg-open` takes the desktop's name for it: Lingmo calls
/// itself KDE and has no `kfmclient`, so `xdg-open` opens nothing there.)
#[cfg(all(unix, not(target_os = "macos")))]
fn opener() -> std::process::Command {
    let gio =
        std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("gio").is_file()));
    if gio {
        let mut command = std::process::Command::new("gio");
        command.arg("open");
        command
    } else {
        std::process::Command::new("xdg-open")
    }
}

/// Show a folder in the desktop's file manager.
pub fn open_folder(dir: &Path) -> std::io::Result<()> {
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = opener();
    #[cfg(windows)]
    let mut command = std::process::Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    command.arg(dir).spawn().map(reaped)
}

/// Show a file in the desktop's file manager, selected where that is
/// possible (Windows, macOS), else its folder.
pub fn reveal(file: &Path) -> std::io::Result<()> {
    if cfg!(windows) {
        std::process::Command::new("explorer.exe").arg(format!("/select,{}", file.display())).spawn().map(reaped)
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg("-R").arg(file).spawn().map(reaped)
    } else {
        open_folder(file.parent().unwrap_or(file))
    }
}

#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};

    /// Opens a file with its program (`open` on macOS, `xdg-open` on
    /// desktops that have it).
    pub fn open_file(path: &Path) -> std::io::Result<()> {
        #[cfg(target_os = "macos")]
        let mut command = std::process::Command::new("open");
        #[cfg(not(target_os = "macos"))]
        let mut command = super::opener();
        let status = command.arg(path).status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("{:?} failed ({status})", command.get_program())))
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

    /// The file system `path` is on: its size and what is free to this
    /// user, in bytes.
    pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c` is a valid C string and `st` ours, both alive
        // through the call.
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        let unit = st.f_frsize as u64;
        Some((st.f_blocks as u64 * unit, st.f_bavail as u64 * unit))
    }
}

#[cfg(unix)]
pub use unix::{disk_space, downloads_folder, drives, open_file, recycle, user_folders};

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
