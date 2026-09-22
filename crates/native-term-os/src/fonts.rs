//! Font files, mapped read-only for the rest of the process: the pages
//! are backed by the file (shared with the system's cache), not private
//! memory. Used for large fonts.

use std::path::PathBuf;

#[cfg(windows)]
pub use native_term_win::map_file_for_process as map_file;

/// Where the system keeps its fonts.
fn font_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        vec![windir.join("Fonts")]
    }
    #[cfg(target_os = "macos")]
    {
        vec![PathBuf::from("/System/Library/Fonts"), PathBuf::from("/Library/Fonts")]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")]
    }
}

/// The file named `name` under `dir`, up to `depth` levels down.
fn find_file(dir: &std::path::Path, name: &str, depth: usize) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).find_map(|d| find_file(&d, name, depth - 1))
}

/// The first of `names` (in that order of preference) found in any of the
/// font folders, up to `depth` levels down.
fn find_font(names: &[&str], depth: usize) -> Option<PathBuf> {
    let dirs = font_dirs();
    names.iter().find_map(|name| dirs.iter().find_map(|d| find_file(d, name, depth)))
}

/// A font with Chinese, Japanese and Korean glyphs, if the system has
/// one NativeTerm knows: Microsoft YaHei or SimSun, PingFang, Noto Sans
/// CJK, WenQuanYi.
#[must_use]
pub fn cjk_file() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["msyh.ttc", "simsun.ttc"]
    } else if cfg!(target_os = "macos") {
        &["PingFang.ttc", "Hiragino Sans GB.ttc", "STHeiti Light.ttc"]
    } else {
        &["NotoSansCJK-Regular.ttc", "NotoSansCJKsc-Regular.otf", "NotoSansSC-Regular.otf", "wqy-microhei.ttc"]
    };
    find_font(names, 3)
}

/// The system's icon font, where it has one NativeTerm draws with:
/// Segoe Fluent Icons (Segoe MDL2 Assets on Windows 10).
#[must_use]
pub fn icon_file() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    find_font(&["SegoeIcons.ttf", "segmdl2.ttf"], 0)
}

#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::io;
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    /// Map a file read-only for the rest of the process.
    pub fn map_file(path: &Path) -> io::Result<&'static [u8]> {
        let file = File::open(path)?;
        let len = usize::try_from(file.metadata()?.len()).map_err(io::Error::other)?;
        if len == 0 {
            return Ok(&[]);
        }
        // SAFETY: a fresh private read-only mapping of `len` bytes of an
        // open file; never unmapped, so the slice lives for the process.
        // The file may close: the mapping keeps its own reference.
        let view =
            unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_PRIVATE, file.as_raw_fd(), 0) };
        if view == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `view` points at `len` readable bytes for the rest of
        // the process (see above).
        Ok(unsafe { std::slice::from_raw_parts(view as *const u8, len) })
    }
}

#[cfg(unix)]
pub use unix::map_file;

#[cfg(test)]
mod tests {
    #[test]
    fn the_first_name_wins() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("second.ttf"), b"2").unwrap();
        std::fs::write(dir.path().join("third.ttf"), b"3").unwrap();
        assert_eq!(super::find_file(dir.path(), "second.ttf", 1), Some(sub.join("second.ttf")));
        assert_eq!(super::find_file(dir.path(), "second.ttf", 0), None);
        assert_eq!(super::find_file(dir.path(), "third.ttf", 0), Some(dir.path().join("third.ttf")));
    }

    #[test]
    fn maps_what_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("font.bin");
        std::fs::write(&path, b"glyphs").unwrap();
        assert_eq!(super::map_file(&path).unwrap(), b"glyphs");
        let empty = dir.path().join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        assert!(super::map_file(&empty).unwrap().is_empty());
    }
}
