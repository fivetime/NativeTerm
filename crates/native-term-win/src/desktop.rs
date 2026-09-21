//! Registry values and other programs' windows.

use std::io;
use std::path::{Path, PathBuf};

use windows::core::{BOOL, HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HWND, LPARAM};
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible, MessageBoxW, PostMessageW,
    SetForegroundWindow, ShowWindow, SystemParametersInfoW, GW_OWNER, MB_ICONERROR, MB_OK, SPI_GETCLIENTAREAANIMATION,
    SW_RESTORE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WM_CLOSE,
};

/// The Windows accent colour (Settings -> Personalisation -> Colours),
/// as red, green and blue. DWM keeps it as 0xAABBGGRR under
/// `HKCU\Software\Microsoft\Windows\DWM\AccentColor`. `None` when it
/// isn't there (a very old build, or a policy).
#[must_use]
pub fn accent() -> Option<(u8, u8, u8)> {
    let abgr = user_registry_dword(r"Software\Microsoft\Windows\DWM", "AccentColor")?;
    Some((abgr as u8, (abgr >> 8) as u8, (abgr >> 16) as u8))
}

/// A `DWORD` value under `HKEY_CURRENT_USER`.
#[must_use]
pub fn user_registry_dword(subkey: &str, value: &str) -> Option<u32> {
    let (subkey, value) = (HSTRING::from(subkey), HSTRING::from(value));
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: the buffer and its size are this `u32`; the strings live
    // for the call.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &subkey,
            &value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(data)
}

/// Whether Windows' animation effects are on (Settings -> Accessibility
/// -> Visual effects -> Animation effects). Something that moves or
/// fades asks this first, so a person who turned animations off doesn't
/// get ours. Asked at most once a second; the answer only changes when
/// they change the setting.
#[must_use]
pub fn animations() -> bool {
    static LAST: std::sync::Mutex<Option<(std::time::Instant, bool)>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((when, on)) = *last {
        if when.elapsed() < std::time::Duration::from_secs(1) {
            return on;
        }
    }
    let mut on = BOOL(1);
    // SAFETY: asks for one BOOL and is given one.
    let asked = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut BOOL as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    // if Windows doesn't say, animate (that is its own default)
    let on = asked.is_err() || on.as_bool();
    *last = Some((std::time::Instant::now(), on));
    on
}

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
