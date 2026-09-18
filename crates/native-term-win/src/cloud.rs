//! Whether a file is kept by a cloud sync client (OneDrive "Files
//! On-Demand" and other cloud-files providers) and whether its content is
//! on this PC. Only attributes are read, never the content (reading a
//! cloud-only file would download it).

use std::path::Path;

use windows::core::HSTRING;
use windows::Win32::Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x40000;
const FILE_ATTRIBUTE_PINNED: u32 = 0x80000;
const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x400000;
/// `IO_REPARSE_TAG_CLOUD` and its variants (`0x9000?01A`).
const CLOUD_TAG: u32 = 0x9000_001A;
const CLOUD_TAG_MASK: u32 = 0xFFFF_0FFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudState {
    /// A normal local file.
    Local,
    /// Synced and marked "Always keep on this device".
    Kept,
    /// Synced, on this PC now, but may be freed up to the cloud.
    Available,
    /// Only in the cloud: reading it downloads it first (or fails offline).
    CloudOnly,
}

/// The state from a file's attributes and reparse tag.
pub fn classify(attributes: u32, reparse_tag: u32) -> CloudState {
    if attributes & (FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_OFFLINE) != 0 {
        return CloudState::CloudOnly;
    }
    let cloud = attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && reparse_tag & CLOUD_TAG_MASK == CLOUD_TAG;
    if attributes & FILE_ATTRIBUTE_PINNED != 0 {
        CloudState::Kept
    } else if cloud {
        CloudState::Available
    } else {
        CloudState::Local
    }
}

/// `None` if the file can't be looked at.
pub fn cloud_state(path: &Path) -> Option<CloudState> {
    let mut data = WIN32_FIND_DATAW::default();
    let handle = unsafe { FindFirstFileW(&HSTRING::from(path), &mut data) }.ok()?;
    unsafe {
        let _ = FindClose(handle);
    }
    // dwReserved0 holds the reparse tag for reparse points
    let tag = if data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 { data.dwReserved0 } else { 0 };
    Some(classify(data.dwFileAttributes, tag))
}

/// Folders a sync client keeps in step across computers, by name:
/// OneDrive (personal and work, from the variables its client sets) and
/// Dropbox (its default folder). Only ones that exist.
pub fn sync_roots() -> Vec<(String, std::path::PathBuf)> {
    use std::path::PathBuf;
    let mut roots: Vec<(String, PathBuf)> = Vec::new();
    for (var, name) in [("OneDriveConsumer", "OneDrive"), ("OneDriveCommercial", "OneDrive (work)"), ("OneDrive", "OneDrive")] {
        if let Some(dir) = std::env::var_os(var).map(PathBuf::from).filter(|d| d.is_dir()) {
            if !roots.iter().any(|(_, d)| d == &dir) {
                roots.push((name.to_string(), dir));
            }
        }
    }
    if let Some(profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        let dropbox = profile.join("Dropbox");
        if dropbox.is_dir() {
            roots.push(("Dropbox".to_string(), dropbox));
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states() {
        assert_eq!(classify(0x20, 0), CloudState::Local);
        // a symbolic link is a reparse point, but not a cloud file
        assert_eq!(classify(0x20 | 0x400, 0xA000_000C), CloudState::Local);
        assert_eq!(classify(0x20 | 0x400, 0x9000_001A), CloudState::Available);
        assert_eq!(classify(0x20 | 0x400, 0x9000_701A), CloudState::Available);
        assert_eq!(classify(0x20 | 0x400 | 0x80000, 0x9000_001A), CloudState::Kept);
        assert_eq!(classify(0x20 | 0x400 | 0x400000, 0x9000_001A), CloudState::CloudOnly);
        assert_eq!(classify(0x1000, 0), CloudState::CloudOnly);
    }

    #[test]
    fn local_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(cloud_state(&file), Some(CloudState::Local));
        assert_eq!(cloud_state(&dir.path().join("missing")), None);
    }
}
