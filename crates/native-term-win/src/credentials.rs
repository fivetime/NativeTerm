//! Saved passwords in Windows Credential Manager (generic credentials of
//! this user, kept on this machine: not roaming). Windows encrypts them in
//! the user's profile; NativeTerm has no password store of its own. See
//! ARCHITECTURE.md, "Optional: saved passwords via Windows Credential
//! Manager".

use std::io;

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

/// A saved password and its note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub user: String,
    pub secret: String,
    /// NativeTerm's note on it (e.g. that a server refused it).
    pub comment: String,
}

/// The credential named `target`, if there is one.
pub fn read(target: &str) -> io::Result<Option<Saved>> {
    let mut found: *mut CREDENTIALW = std::ptr::null_mut();
    let name = HSTRING::from(target);
    if let Err(e) = unsafe { CredReadW(&name, CRED_TYPE_GENERIC, None, &mut found) } {
        // ERROR_NOT_FOUND
        return if e.code() == windows::Win32::Foundation::ERROR_NOT_FOUND.to_hresult() {
            Ok(None)
        } else {
            Err(e.into())
        };
    }
    let saved = unsafe {
        let c = &*found;
        let blob = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let text = |p: PWSTR| if p.is_null() { String::new() } else { p.to_string().unwrap_or_default() };
        let saved = Saved { user: text(c.UserName), secret: blob_text(blob), comment: text(c.Comment) };
        CredFree(found as *const _);
        saved
    };
    Ok(Some(saved))
}

/// A credential's secret as text. NativeTerm writes UTF-8; `cmdkey`,
/// Windows' own credential dialogs and most other tools write UTF-16LE.
/// UTF-8 text has no NUL bytes (a password can't contain NUL), UTF-16 of
/// ASCII has one in every other byte, and UTF-16 of other text is almost
/// never valid UTF-8: so valid UTF-8 without NUL is UTF-8, anything else
/// of even length is UTF-16.
fn blob_text(blob: &[u8]) -> String {
    match std::str::from_utf8(blob) {
        Ok(text) if !text.contains('\0') => text.to_string(),
        _ if blob.len().is_multiple_of(2) => {
            let wide: Vec<u16> = blob.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            String::from_utf16_lossy(&wide).trim_end_matches('\0').to_string()
        }
        _ => String::from_utf8_lossy(blob).into_owned(),
    }
}

/// Create or replace the credential named `target`.
pub fn write(target: &str, saved: &Saved) -> io::Result<()> {
    let mut name: Vec<u16> = target.encode_utf16().chain([0]).collect();
    let mut user: Vec<u16> = saved.user.encode_utf16().chain([0]).collect();
    let mut comment: Vec<u16> = saved.comment.encode_utf16().chain([0]).collect();
    let mut blob = saved.secret.as_bytes().to_vec();
    let credential = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(name.as_mut_ptr()),
        Comment: PWSTR(comment.as_mut_ptr()),
        LastWritten: FILETIME::default(),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: PWSTR::null(),
        UserName: PWSTR(user.as_mut_ptr()),
    };
    let result = unsafe { CredWriteW(&credential, 0) };
    blob.fill(0);
    result.map_err(io::Error::from)
}

/// Remove the credential named `target`; `Ok(false)` if there was none.
pub fn delete(target: &str) -> io::Result<bool> {
    match unsafe { CredDeleteW(&HSTRING::from(target), CRED_TYPE_GENERIC, None) } {
        Ok(()) => Ok(true),
        Err(e) if e.code() == windows::Win32::Foundation::ERROR_NOT_FOUND.to_hresult() => Ok(false),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Written, read back, noted, removed; a test name only.
    #[test]
    fn round_trip() {
        let target = format!("NativeTerm-Tests-{}:tester@host.invalid:22", std::process::id());
        assert_eq!(read(&target).unwrap(), None);
        let saved = Saved { user: "tester".into(), secret: "pässwörd 密码".into(), comment: String::new() };
        write(&target, &saved).unwrap();
        assert_eq!(read(&target).unwrap().as_ref(), Some(&saved));
        let refused = Saved { comment: "refused".into(), ..saved.clone() };
        write(&target, &refused).unwrap();
        assert_eq!(read(&target).unwrap().unwrap().comment, "refused");
        assert!(delete(&target).unwrap());
        assert!(!delete(&target).unwrap());
        assert_eq!(read(&target).unwrap(), None);
    }

    /// Secrets written by other tools (UTF-16LE) read like NativeTerm's own.
    #[test]
    fn secrets_in_utf16() {
        let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
        assert_eq!(blob_text(b"plain"), "plain");
        assert_eq!(blob_text("密码".as_bytes()), "密码");
        assert_eq!(blob_text(&utf16("plain")), "plain");
        assert_eq!(blob_text(&utf16("密码 pässwörd")), "密码 pässwörd");
    }
}
