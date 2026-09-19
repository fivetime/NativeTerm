//! File and folder pickers for console helpers (the shim's rz / sz).
//!
//! The dialog is owned by a hidden, topmost window of this process only.
//! A dialog owned by a Terminal window disables that window while it is
//! open, and a helper killed meanwhile left the Terminal disabled for
//! good (why an earlier picker was removed from `shell`). An owner of our
//! own also keeps the dialog above the Terminal the user typed `rz` in:
//! a console child is rarely allowed to take the foreground.
//!
//! Blocking: run it on a thread that may wait for the user.

use std::path::{Path, PathBuf};

use windows::core::{w, HSTRING};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST,
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetCursorPos, SetForegroundWindow, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

/// COM on this thread while the value lives.
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
        // thread (`Com` is neither Send nor Sync); the COM objects made
        // meanwhile are dropped before it.
        unsafe { CoUninitialize() };
    }
}

/// A hidden topmost window of this process near the mouse, to own a
/// dialog; destroyed with the value.
struct Owner(HWND);

impl Owner {
    fn new() -> Option<Owner> {
        let mut at = windows::Win32::Foundation::POINT::default();
        // SAFETY: `at` is a local out-parameter; the class is the
        // system's "STATIC", the strings are static wide literals, and
        // the window is destroyed in Drop on this same thread.
        unsafe {
            let _ = GetCursorPos(&mut at);
            let module = GetModuleHandleW(None).ok()?;
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                w!("STATIC"),
                w!("NativeTerm"),
                WS_POPUP,
                at.x,
                at.y,
                0,
                0,
                None,
                None,
                Some(module.into()),
                None,
            )
            .ok()?;
            let _ = SetForegroundWindow(window);
            Some(Owner(window))
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        // SAFETY: our own window, made on this thread by `new`.
        unsafe {
            let _ = DestroyWindow(self.0);
        }
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

/// Files to send (several may be chosen), starting in `start`; `None` if
/// cancelled.
pub fn pick_files(title: &str, start: Option<&Path>) -> Option<Vec<PathBuf>> {
    pick(title, start, false)
}

/// A folder, starting in `start`; `None` if cancelled.
pub fn pick_folder(title: &str, start: Option<&Path>) -> Option<PathBuf> {
    pick(title, start, true).and_then(|mut v| v.pop())
}

fn pick(title: &str, start: Option<&Path>, folders: bool) -> Option<Vec<PathBuf>> {
    let _com = Com::init()?;
    let owner = Owner::new();
    // SAFETY: COM is initialized on this thread for the whole block (`_com`
    // outlives the objects made here, which drop at its end); the owner is
    // our own window or none; the HSTRINGs live through their calls.
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let base = dialog.GetOptions().ok()? | FOS_FORCEFILESYSTEM;
        let options = if folders { base | FOS_PICKFOLDERS } else { base | FOS_ALLOWMULTISELECT | FOS_FILEMUSTEXIST };
        dialog.SetOptions(options).ok()?;
        let _ = dialog.SetTitle(&HSTRING::from(title));
        if let Some(start) = start.filter(|p| p.is_dir()) {
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(start.as_os_str()), None) {
                let _ = dialog.SetFolder(&item);
            }
        }
        // cancelled: an error
        dialog.Show(owner.as_ref().map(|o| o.0)).ok()?;
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
