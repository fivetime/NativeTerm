//! Saved passwords in the system's own store (Windows Credential
//! Manager). Nowhere else yet: `supported()` says so, and the program
//! leaves the saving out.

#[cfg(windows)]
pub use native_term_win::credentials::{delete, list, read, update, write, Saved};

/// Whether this system has a store to keep passwords in.
#[must_use]
pub fn supported() -> bool {
    cfg!(windows)
}

#[cfg(unix)]
mod unix {
    use std::io;

    /// A saved password and its note.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Saved {
        pub user: String,
        pub secret: String,
        /// NativeTerm's note on it (e.g. that a server refused it).
        pub comment: String,
    }

    /// The credential named `target`, if there is one.
    pub fn read(_target: &str) -> io::Result<Option<Saved>> {
        Ok(None)
    }

    pub fn write(_target: &str, _saved: &Saved) -> io::Result<()> {
        Err(crate::unsupported("saving passwords"))
    }

    /// The names of the credentials starting with `prefix`.
    pub fn list(_prefix: &str) -> io::Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// Whether there was one to delete.
    pub fn delete(_target: &str) -> io::Result<bool> {
        Ok(false)
    }

    /// Whether there was one to change.
    pub fn update(_target: &str, _change: impl FnOnce(&mut Saved)) -> io::Result<bool> {
        Ok(false)
    }
}

#[cfg(unix)]
pub use unix::{delete, list, read, update, write, Saved};

#[cfg(test)]
mod tests {
    #[test]
    fn nothing_saved_under_a_name_nobody_uses() {
        let name = format!("NativeTerm-Tests-os-{}", std::process::id());
        assert_eq!(super::read(&name).unwrap(), None);
        assert!(!super::delete(&name).unwrap());
        assert_eq!(super::supported(), cfg!(windows));
    }
}
