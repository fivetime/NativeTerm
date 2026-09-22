//! Files kept by a cloud sync client (OneDrive's "Files On-Demand" and
//! the like): whether a file's content is on this computer, and which
//! folders a client keeps in step across computers.

#[cfg(windows)]
pub use native_term_win::cloud::{cloud_state, sync_roots, CloudState};

#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum CloudState {
        /// A normal local file.
        Local,
        /// Synced and marked to be kept on this computer.
        Kept,
        /// Synced, here now, but may be freed up to the cloud.
        Available,
        /// Only in the cloud: reading it downloads it first (or fails offline).
        CloudOnly,
    }

    /// No sync client's placeholders are recognised here yet: `None`.
    pub fn cloud_state(_path: &Path) -> Option<CloudState> {
        None
    }

    /// Folders a sync client keeps in step across computers, by name:
    /// the default folders of Dropbox and OneDrive, where they exist.
    pub fn sync_roots() -> Vec<(String, PathBuf)> {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
        [("Dropbox", "Dropbox"), ("OneDrive", "OneDrive")]
            .iter()
            .map(|(name, dir)| (name.to_string(), home.join(dir)))
            .filter(|(_, dir)| dir.is_dir())
            .collect()
    }
}

#[cfg(unix)]
pub use unix::{cloud_state, sync_roots, CloudState};
