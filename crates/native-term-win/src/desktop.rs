//! Registry values and other programs' windows.

use std::io;
use std::path::{Path, PathBuf};

use windows::core::{BOOL, HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HWND, LPARAM};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible, MessageBoxW, PostMessageW,
    SetForegroundWindow, ShowWindow, GW_OWNER, MB_ICONERROR, MB_OK, SW_RESTORE, WM_CLOSE,
};

/// A string value under `HKEY_CURRENT_USER` (`REG_SZ`, or `REG_EXPAND_SZ`
/// expanded). `None` if the key or value doesn't exist.
pub fn user_registry_string(subkey: &str, value: &str) -> io::Result<Option<String>> {
    let (subkey, value) = (HSTRING::from(subkey), HSTRING::from(value));
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
    let mut size = 0u32;
    let status = unsafe { RegGetValueW(HKEY_CURRENT_USER, &subkey, &value, flags, None, None, Some(&mut size)) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status.0 as i32));
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, &subkey, &value, flags, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status.0 as i32));
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Ok(Some(String::from_utf16_lossy(&buf[..end])))
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

/// Visible, unowned top-level windows of other processes running `image`.
pub fn windows_of_other_instances(image: &Path) -> Vec<isize> {
    unsafe extern "system" fn each(window: HWND, found: LPARAM) -> BOOL {
        let visible = unsafe { IsWindowVisible(window) }.as_bool();
        let owned = unsafe { GetWindow(window, GW_OWNER) }.is_ok_and(|o| !o.is_invalid());
        if visible && !owned {
            unsafe { &mut *(found.0 as *mut Vec<HWND>) }.push(window);
        }
        BOOL(1)
    }
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut all as *mut _ as isize));
    }
    let me = std::process::id();
    let image = image.to_string_lossy().to_lowercase();
    all.into_iter()
        .filter(|&window| {
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
            pid != me && process_image(pid).is_some_and(|p| p.to_string_lossy().to_lowercase() == image)
        })
        .map(|w| w.0 as isize)
        .collect()
}

/// Restore and bring a window to the front (allowed while this process
/// was just started by the user or is in the foreground).
pub fn bring_to_front(handle: isize) -> bool {
    let window = HWND(handle as *mut _);
    unsafe {
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
        SetForegroundWindow(window).as_bool()
    }
}

/// Ask a window to close, as its close button would.
pub fn close_window(handle: isize) -> bool {
    unsafe { PostMessageW(Some(HWND(handle as *mut _)), WM_CLOSE, Default::default(), Default::default()) }.is_ok()
}

/// An error for a program without a window (or whose window failed).
pub fn message_box(title: &str, text: &str) {
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(title), MB_OK | MB_ICONERROR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_strings() {
        // present on every Windows installation
        let value = user_registry_string(r"Control Panel\International", "LocaleName").unwrap();
        assert!(value.is_some_and(|v| !v.is_empty()));
        assert_eq!(user_registry_string(r"Software\NativeTerm-NoSuchKey", "DataDir").unwrap(), None);
        assert_eq!(user_registry_string(r"Control Panel\International", "NoSuchValue").unwrap(), None);
    }

    #[test]
    fn no_other_instance_of_a_missing_program() {
        assert!(windows_of_other_instances(Path::new(r"C:\nowhere\nothing.exe")).is_empty());
        assert!(process_image(std::process::id()).is_some());
    }
}
