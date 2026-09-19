//! The Windows shell: file and folder pickers, opening a file with its
//! program, known folders.
//!
//! The pickers block until the user is done, so callers run them on a
//! thread of their own (COM is set up there); the owner window is
//! disabled meanwhile, as with any modal dialog.

use std::path::PathBuf;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, IShellItem, SHGetKnownFolderPath, ShellExecuteW, FOLDERID_Downloads,
    FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, KF_FLAG_DEFAULT, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// COM for this thread while the value lives.
struct Com;

impl Com {
    fn init() -> Option<Com> {
        // SAFETY: no pointers; a failure (another apartment on this
        // thread) is reported and nothing is uninitialized for it.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }.is_ok().then_some(Com)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: pairs the successful CoInitializeEx of `init` on this
        // thread (`Com` is neither Send nor Sync: it stays on the thread);
        // the COM objects made meanwhile are dropped before it.
        unsafe { CoUninitialize() };
    }
}

fn item_path(item: &IShellItem) -> Option<PathBuf> {
    // SAFETY: `item` is a live COM object; the name GetDisplayName returns
    // is a CoTaskMem string owned by us, read once and freed once.
    unsafe {
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path.map(PathBuf::from)
    }
}

/// Files to open (several may be chosen); `None` if cancelled.
pub fn pick_files(owner: isize, title: &str) -> Option<Vec<PathBuf>> {
    pick(owner, title, false)
}

/// A folder; `None` if cancelled.
pub fn pick_folder(owner: isize, title: &str) -> Option<PathBuf> {
    pick(owner, title, true).and_then(|mut v| v.pop())
}

fn pick(owner: isize, title: &str, folders: bool) -> Option<Vec<PathBuf>> {
    let _com = Com::init()?;
    // SAFETY: COM is initialized on this thread for the whole block (`_com`
    // outlives the objects made here, which drop at its end); `owner` is a
    // window handle or 0 (no owner), and a stale handle only makes Show
    // fail; the title's HSTRING lives through the call.
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let base = dialog.GetOptions().ok()? | FOS_FORCEFILESYSTEM;
        let options = if folders { base | FOS_PICKFOLDERS } else { base | FOS_ALLOWMULTISELECT | FOS_FILEMUSTEXIST };
        dialog.SetOptions(options).ok()?;
        let _ = dialog.SetTitle(&HSTRING::from(title));
        // cancelled: an error
        dialog.Show(Some(HWND(owner as *mut _))).ok()?;
        let items = dialog.GetResults().ok()?;
        let mut paths = Vec::new();
        for i in 0..items.GetCount().ok()? {
            if let Some(path) = items.GetItemAt(i).ok().as_ref().and_then(item_path) {
                paths.push(path);
            }
        }
        (!paths.is_empty()).then_some(paths)
    }
}

/// Opens a file with its program; one without a program gets Windows'
/// "How do you want to open this file?".
pub fn open_file(path: &std::path::Path) -> std::io::Result<()> {
    let file = HSTRING::from(path.as_os_str());
    for verb in ["open", "openas"] {
        // SAFETY: the strings are HSTRINGs alive through the call; null
        // parameters and window are allowed.
        let result = unsafe { ShellExecuteW(None, &HSTRING::from(verb), &file, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
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
    buf[..len.min(buf.len())].split(|&c| c == 0).filter(|d| !d.is_empty()).map(|d| PathBuf::from(String::from_utf16_lossy(d))).collect()
}

/// The window in front (the one the user just clicked in): the owner for
/// a picker.
pub fn foreground_window() -> isize {
    // SAFETY: no parameters; the handle is only passed on as a number.
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as isize
}

/// The user's Downloads folder.
pub fn downloads_folder() -> Option<PathBuf> {
    // SAFETY: the path SHGetKnownFolderPath returns is a CoTaskMem string
    // owned by us, read once and freed once.
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        text.map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn drives_and_recycle_bin() {
        assert!(super::drives().iter().any(|d| d.to_string_lossy().eq_ignore_ascii_case("C:\\")), "{:?}", super::drives());
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
