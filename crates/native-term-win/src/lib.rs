//! Small Win32 helpers shared by NativeTerm's crates: the current user's
//! identity, security descriptors (file ACLs, pipe ACLs), and read-only
//! file mappings.
#![cfg(windows)]

pub mod desktop;
pub mod dock;
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

#[cfg(test)]
mod tests {
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
