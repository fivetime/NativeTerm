//! File ACLs as Windows OpenSSH expects them: the config and every
//! included file must be owned by the user (or Administrators/SYSTEM) and
//! writable by nobody else, or ssh aborts with "Bad owner or permissions".

use std::io;
use std::path::Path;

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    GetTokenInformation, SetFileSecurityW, TokenUser, DACL_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// The current user's SID as a string (`S-1-5-21-…`).
pub fn current_user_sid() -> io::Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let result = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len);
        let _ = CloseHandle(token);
        result?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut sid = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid)?;
        let text = sid.to_string().map_err(io::Error::other);
        LocalFree(HLOCAL(sid.0.cast()));
        text
    }
}

/// Protected DACL: full control for the user, Administrators, and SYSTEM
/// only (no inherited entries).
pub fn restrict_to_owner(path: &Path) -> io::Result<()> {
    set_dacl(path, &format!("D:P(A;;FA;;;{})(A;;FA;;;BA)(A;;FA;;;SY)", current_user_sid()?))
}

pub fn set_dacl(path: &Path, sddl: &str) -> io::Result<()> {
    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(&HSTRING::from(sddl), SDDL_REVISION_1, &mut sd, None)?;
        let ok = SetFileSecurityW(
            &HSTRING::from(path),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            sd,
        );
        let error = io::Error::last_os_error();
        LocalFree(HLOCAL(sd.0));
        if ok.as_bool() {
            Ok(())
        } else {
            Err(error)
        }
    }
}

/// The file's DACL in SDDL form, e.g. `D:P(A;;FA;;;S-1-5-…)…`.
pub fn dacl_sddl(path: &Path) -> io::Result<String> {
    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        GetNamedSecurityInfoW(&HSTRING::from(path), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, None, None, None, None, &mut sd)
            .ok()?;
        let mut text = PWSTR::null();
        let result =
            ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut text, None);
        LocalFree(HLOCAL(sd.0));
        result?;
        let out = text.to_string().map_err(io::Error::other);
        LocalFree(HLOCAL(text.0.cast()));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricts_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config");
        std::fs::write(&file, "x").unwrap();
        restrict_to_owner(&file).unwrap();
        let sddl = dacl_sddl(&file).unwrap();
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-"), "{sid}");
        assert!(sddl.starts_with("D:P"), "{sddl}");
        assert!(sddl.contains(&sid), "{sddl}");
        assert_eq!(sddl.matches("(A;").count(), 3, "{sddl}");
    }
}
