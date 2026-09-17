//! Terminal top-level windows through plain Win32, never UIA: a hung
//! Terminal blocks UIA without limit, so each window is probed with
//! `WM_NULL` first (see "A hung Terminal blocks UIA").

use std::path::PathBuf;
use std::time::Duration;

use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SendMessageTimeoutW, SetForegroundWindow, ShowWindow, SMTO_ABORTIFHUNG, SMTO_BLOCK, SW_RESTORE, WM_NULL,
};

use super::install::Install;

pub const WINDOW_CLASS: &str = "CASCADIA_HOSTING_WINDOW_CLASS";
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalWindow {
    pub handle: isize,
    pub pid: u32,
    pub responsive: bool,
}

pub fn hwnd(handle: isize) -> HWND {
    HWND(handle as *mut _)
}

pub fn process_image(pid: u32) -> Option<PathBuf> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        result.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

fn class_name(window: HWND) -> String {
    let mut class = [0u16; 64];
    let n = unsafe { GetClassNameW(window, &mut class) };
    String::from_utf16_lossy(&class[..n.max(0) as usize])
}

/// Visible Terminal windows of `install`, in Z order (topmost first).
pub fn terminal_windows(install: &Install) -> Vec<TerminalWindow> {
    unsafe extern "system" fn each(window: HWND, found: LPARAM) -> BOOL {
        if unsafe { IsWindowVisible(window) }.as_bool() && class_name(window) == WINDOW_CLASS {
            unsafe { &mut *(found.0 as *mut Vec<HWND>) }.push(window);
        }
        BOOL(1)
    }
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut all as *mut _ as isize));
    }
    let mut images: Vec<(u32, bool)> = Vec::new();
    all.into_iter()
        .filter_map(|window| {
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
            let ours = match images.iter().find(|(p, _)| *p == pid) {
                Some((_, ours)) => *ours,
                None => {
                    let ours = process_image(pid).is_some_and(|image| install.owns_image(&image));
                    images.push((pid, ours));
                    ours
                }
            };
            ours.then(|| TerminalWindow { handle: window.0 as isize, pid, responsive: responds(window) })
        })
        .collect()
}

pub fn responds(window: HWND) -> bool {
    let mut result = 0usize;
    let answered = unsafe {
        SendMessageTimeoutW(
            window,
            WM_NULL,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            PROBE_TIMEOUT.as_millis() as u32,
            Some(&mut result),
        )
    };
    answered.0 != 0
}

pub fn foreground() -> isize {
    unsafe { GetForegroundWindow() }.0 as isize
}

/// Bring a Terminal window to the front. Works while NativeTerm is the
/// foreground app (the user just clicked it), as Windows requires.
pub fn activate(handle: isize) -> bool {
    let window = hwnd(handle);
    unsafe {
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
        SetForegroundWindow(window).as_bool()
    }
}
