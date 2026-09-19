//! The Windows shell: opening a file with its program, the Recycle Bin,
//! drives and known folders.
//!
//! (A file picker once lived here, owned by the window in front: that was
//! sometimes a Terminal window, which Windows disables while a modal
//! dialog it owns is open; a picker killed with its process left the
//! Terminal disabled for good. A picker, if one comes back, is owned by
//! a NativeTerm window only.)

use std::path::PathBuf;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath, ShellExecuteW, KF_FLAG_DEFAULT};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Opens a file with its program; one without a program gets Windows'
/// "How do you want to open this file?".
pub fn open_file(path: &std::path::Path) -> std::io::Result<()> {
    let file = HSTRING::from(path.as_os_str());
    for verb in ["open", "openas"] {
        // SAFETY: the strings are HSTRINGs alive through the call; null
        // parameters and window are allowed.
        let result =
            unsafe { ShellExecuteW(None, &HSTRING::from(verb), &file, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
        // greater than 32: success; 31: no program for it
        if result.0 as isize > 32 {
            return Ok(());
        }
        if result.0 as isize != 31 {
            return Err(std::io::Error::other(format!("ShellExecute failed ({})", result.0 as isize)));
        }
    }
    Err(std::io::Error::other("no program to open it"))
}

/// Moves files and folders to the Recycle Bin (no questions from Windows:
/// NativeTerm asked already); an error if any stayed.
pub fn recycle(paths: &[PathBuf]) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW,
    };
    if paths.is_empty() {
        return Ok(());
    }
    // NUL-terminated paths, the list ended by one more NUL
    let mut from: Vec<u16> = Vec::new();
    for path in paths {
        from.extend(path.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    // SAFETY: `op` points at `from`, a double-NUL-terminated list alive
    // through the call; no window, no progress title.
    let result = unsafe { SHFileOperationW(&mut op) };
    if result != 0 || op.fAnyOperationsAborted.as_bool() {
        return Err(std::io::Error::other(format!("not moved to the Recycle Bin (error {result})")));
    }
    Ok(())
}

/// The drives (`C:\`, `D:\`, …).
pub fn drives() -> Vec<PathBuf> {
    use windows::Win32::Storage::FileSystem::GetLogicalDriveStringsW;
    let mut buf = vec![0u16; 512];
    // SAFETY: the buffer is ours; its length goes with it.
    let len = unsafe { GetLogicalDriveStringsW(Some(&mut buf)) } as usize;
    buf[..len.min(buf.len())]
        .split(|&c| c == 0)
        .filter(|d| !d.is_empty())
        .map(|d| PathBuf::from(String::from_utf16_lossy(d)))
        .collect()
}

/// The user's Downloads folder.
pub fn downloads_folder() -> Option<PathBuf> {
    known_folder(&FOLDERID_Downloads)
}

/// The user's Desktop, Documents and Downloads folders (the ones there).
pub fn user_folders() -> Vec<PathBuf> {
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Documents};
    [FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads].iter().filter_map(known_folder).collect()
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    // SAFETY: the path SHGetKnownFolderPath returns is a CoTaskMem string
    // owned by us, read once and freed once.
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        text.map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn drives_and_recycle_bin() {
        assert!(
            super::drives().iter().any(|d| d.to_string_lossy().eq_ignore_ascii_case("C:\\")),
            "{:?}",
            super::drives()
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("回收站测试.txt");
        std::fs::write(&file, b"x").unwrap();
        super::recycle(std::slice::from_ref(&file)).unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn downloads() {
        let d = super::downloads_folder().expect("a Downloads folder");
        assert!(d.is_absolute(), "{}", d.display());
    }
}
