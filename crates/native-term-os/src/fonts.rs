//! Font files, mapped read-only for the rest of the process: the pages
//! are backed by the file (shared with the system's cache), not private
//! memory. Used for large fonts.

#[cfg(windows)]
pub use native_term_win::map_file_for_process as map_file;

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
