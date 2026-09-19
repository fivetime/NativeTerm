//! Small Win32 helpers shared by NativeTerm's crates: the current user's
//! identity, security descriptors (file ACLs, pipe ACLs), and read-only
//! file mappings.
#![cfg(windows)]

pub mod cloud;
pub mod credentials;
pub mod desktop;
pub mod dock;
pub mod registry;
pub mod service;
pub mod shell;
pub mod watch;

use std::io;
use std::fs::File;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    GetTokenInformation, SetFileSecurityW, TokenElevation, TokenStatistics, TokenUser, DACL_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_ELEVATION, TOKEN_INFORMATION_CLASS,
    TOKEN_QUERY, TOKEN_STATISTICS, TOKEN_USER,
};
use windows::Win32::System::Memory::{CreateFileMappingW, MapViewOfFile, FILE_MAP_READ, PAGE_READONLY};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

fn token_information(class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u8>> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, class, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let result = GetTokenInformation(token, class, Some(buf.as_mut_ptr().cast()), len, &mut len);
        let _ = CloseHandle(token);
        result?;
        Ok(buf)
    }
}

/// The current user's SID as a string (`S-1-5-21-…`).
/// When a running process started (FILETIME ticks), or `None` if no
/// process with that id is running. Together with the id it names one
/// process: ids are reused, start times are not.
pub fn process_started(pid: u32) -> Option<u64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    const STILL_ACTIVE: u32 = 259;
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut code = 0u32;
    let (mut created, mut exited, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    let result = unsafe {
        GetExitCodeProcess(handle, &mut code)
            .and_then(|()| GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user))
    };
    unsafe {
        let _ = CloseHandle(handle);
    }
    result.ok()?;
    (code == STILL_ACTIVE).then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// The process that started `pid` (its parent id as Windows recorded it;
/// the parent may have exited since).
pub fn parent_pid(pid: u32) -> Option<u32> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = None;
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            if entry.th32ProcessID == pid {
                found = Some(entry.th32ParentProcessID);
                break;
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

/// `YYYY-MM-DD HH:MM` in local time for seconds since the Unix epoch.
pub fn local_date_time(unix: u64) -> String {
    match local_time(unix) {
        Some(t) => format!("{:04}-{:02}-{:02} {:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute),
        None => String::new(),
    }
}

/// `HH:MM` in local time for seconds since the Unix epoch.
pub fn local_time_of_day(unix: u64) -> String {
    match local_time(unix) {
        Some(t) => format!("{:02}:{:02}", t.wHour, t.wMinute),
        None => String::new(),
    }
}

fn local_time(unix: u64) -> Option<windows::Win32::Foundation::SYSTEMTIME> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    // FILETIME: 100 ns steps since 1601
    let ticks = (unix + 11_644_473_600) * 10_000_000;
    let file = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    let ok = unsafe { FileTimeToSystemTime(&file, &mut utc).is_ok() && SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_ok() };
    ok.then_some(local)
}

pub fn user_sid() -> io::Result<String> {
    let buf = token_information(TokenUser)?;
    unsafe {
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut sid = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid)?;
        let text = sid.to_string().map_err(io::Error::other);
        LocalFree(Some(HLOCAL(sid.0.cast())));
        text
    }
}

/// The logon session (authentication id) of the current token, as hex.
/// Two sign-ins of the same user get different values.
pub fn logon_session_id() -> io::Result<String> {
    let buf = token_information(TokenStatistics)?;
    let stats = unsafe { &*(buf.as_ptr() as *const TOKEN_STATISTICS) };
    let luid = stats.AuthenticationId;
    Ok(format!("{:x}{:08x}", luid.HighPart, luid.LowPart))
}

/// Whether this process runs elevated (as administrator with UAC).
pub fn is_elevated() -> bool {
    token_information(TokenElevation)
        .map(|buf| unsafe { (*(buf.as_ptr() as *const TOKEN_ELEVATION)).TokenIsElevated != 0 })
        .unwrap_or(false)
}

/// A self-relative security descriptor parsed from SDDL, freed on drop.
pub struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    pub fn from_sddl(sddl: &str) -> io::Result<SecurityDescriptor> {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(&HSTRING::from(sddl), SDDL_REVISION_1, &mut sd, None)?;
        }
        Ok(SecurityDescriptor(sd))
    }

    pub fn as_ptr(&self) -> *mut core::ffi::c_void {
        self.0 .0
    }
}

// SAFETY: the descriptor is a heap block owned exclusively by this value
// and never mutated after creation; freeing it from any thread is fine.
unsafe impl Send for SecurityDescriptor {}
unsafe impl Sync for SecurityDescriptor {}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(Some(HLOCAL(self.0 .0)));
        }
    }
}

/// Protected DACL: full control for the user, Administrators, and SYSTEM.
pub fn owner_only_sddl() -> io::Result<String> {
    Ok(format!("D:P(A;;FA;;;{})(A;;FA;;;BA)(A;;FA;;;SY)", user_sid()?))
}

pub fn set_file_dacl(path: &Path, sddl: &str) -> io::Result<()> {
    let sd = SecurityDescriptor::from_sddl(sddl)?;
    let ok = unsafe {
        SetFileSecurityW(
            &HSTRING::from(path),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            PSECURITY_DESCRIPTOR(sd.as_ptr()),
        )
    };
    if ok.as_bool() {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The file's DACL in SDDL form, e.g. `D:P(A;;FA;;;S-1-5-…)…`.
pub fn file_dacl_sddl(path: &Path) -> io::Result<String> {
    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        GetNamedSecurityInfoW(&HSTRING::from(path), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, None, None, None, None, &mut sd)
            .ok()?;
        let sd = SecurityDescriptor(sd);
        let mut text = PWSTR::null();
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            PSECURITY_DESCRIPTOR(sd.as_ptr()),
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            None,
        )?;
        let out = text.to_string().map_err(io::Error::other);
        LocalFree(Some(HLOCAL(text.0.cast())));
        out
    }
}

/// Map a file read-only for the rest of the process. The pages are backed
/// by the file (shared with the system file cache), not private memory;
/// used for large fonts. The file stays open for reading.
pub fn map_file_for_process(path: &Path) -> io::Result<&'static [u8]> {
    let file = File::open(path)?;
    let len = usize::try_from(file.metadata()?.len()).map_err(io::Error::other)?;
    if len == 0 {
        return Ok(&[]);
    }
    unsafe {
        let mapping =
            CreateFileMappingW(HANDLE(file.as_raw_handle() as _), None, PAGE_READONLY, 0, 0, None)?;
        let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
        // the view keeps the mapping and the file alive
        let _ = CloseHandle(mapping);
        if view.Value.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: never unmapped; the file is opened without write sharing
        // by us and system fonts aren't rewritten in place
        Ok(std::slice::from_raw_parts(view.Value as *const u8, len))
    }
}

/// The system's ANSI code page (936 on Chinese Windows).
pub fn ansi_code_page() -> u32 {
    // SAFETY: no parameters, reads a system value.
    unsafe { windows::Win32::Globalization::GetACP() }
}

/// A message Windows' OpenSSH wrote to stderr, as text. ssh escapes bytes
/// it won't print as `\ooo` (octal), and the system's own messages (e.g.
/// "no such host" from `getaddrinfo`) are in the ANSI code page, so
/// `\262\273…` is GBK on a Chinese system: undo the escapes, then decode as
/// UTF-8 or, if that isn't valid, the ANSI code page.
pub fn ssh_message(bytes: &[u8]) -> String {
    let mut raw = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes.get(i + 1..i + 4).filter(|d| bytes[i] == b'\\' && d.iter().all(|c| (b'0'..=b'7').contains(c)));
        match octal {
            Some(d) => {
                let v = d.iter().fold(0u32, |v, c| v * 8 + (c - b'0') as u32);
                raw.push(v as u8);
                i += 4;
            }
            None => {
                raw.push(bytes[i]);
                i += 1;
            }
        }
    }
    match String::from_utf8(raw) {
        Ok(text) => text,
        Err(e) => registry::from_ansi(e.as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ssh_messages() {
        assert_eq!(super::ssh_message(b"ssh: connect to host x port 22: Connection refused"), "ssh: connect to host x port 22: Connection refused");
        assert_eq!(super::ssh_message("已经是 UTF-8".as_bytes()), "已经是 UTF-8");
        // "不知道这样的主机。" in GBK, escaped by ssh; only on a Chinese system
        let gbk = super::ssh_message(
            br"ssh: Could not resolve hostname h: \262\273\326\252\265\300\325\342\321\371\265\304\326\367\273\372\241\243",
        );
        assert!(!gbk.contains(r"\262"), "{gbk}");
        if unsafe { windows::Win32::Globalization::GetACP() } == 936 {
            assert!(gbk.ends_with("不知道这样的主机。"), "{gbk}");
        }
    }

    #[test]
    fn process_start_times() {
        let me = super::process_started(std::process::id()).expect("this process runs");
        assert_eq!(super::process_started(std::process::id()), Some(me), "stable");
        let mut child = std::process::Command::new("cmd.exe").args(["/c", "exit"]).spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        // exited: gone, or (if the id was reused already) a later start
        assert!(super::process_started(pid).is_none_or(|t| t > me));
    }

    use super::*;

    #[test]
    fn identity() {
        assert!(user_sid().unwrap().starts_with("S-1-"));
        let logon = logon_session_id().unwrap();
        assert!(!logon.is_empty() && logon.chars().all(|c| c.is_ascii_hexdigit()), "{logon}");
        let _ = is_elevated();
    }

    #[test]
    fn owner_only_file_acl() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config");
        std::fs::write(&file, "x").unwrap();
        set_file_dacl(&file, &owner_only_sddl().unwrap()).unwrap();
        let sddl = file_dacl_sddl(&file).unwrap();
        assert!(sddl.starts_with("D:P"), "{sddl}");
        assert!(sddl.contains(&user_sid().unwrap()), "{sddl}");
        assert_eq!(sddl.matches("(A;").count(), 3, "{sddl}");
    }

    #[test]
    fn mapped_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data");
        std::fs::write(&file, b"font bytes").unwrap();
        assert_eq!(map_file_for_process(&file).unwrap(), b"font bytes");
        std::fs::write(dir.path().join("empty"), b"").unwrap();
        assert!(map_file_for_process(&dir.path().join("empty")).unwrap().is_empty());
        assert!(map_file_for_process(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn invalid_sddl_is_an_error() {
        assert!(SecurityDescriptor::from_sddl("not sddl").is_err());
    }
}
