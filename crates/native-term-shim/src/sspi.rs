//! Windows logins for a proxy: NTLM and Negotiate (Kerberos), through
//! SSPI.
//!
//! A corporate proxy that answers `Proxy-Authenticate: NTLM` or
//! `Negotiate` wants a handshake, not a password: two or three rounds of
//! tokens, each carried base64 in a `Proxy-Authorization` header, on one
//! connection (NTLM is per connection, which is why the proxy keeps it
//! open between the rounds).
//!
//! Windows makes the tokens. With no credentials of our own it uses the
//! ones the person is signed in with, which is the whole point: nothing
//! to type, nothing to store, and the password never passes through
//! NativeTerm. A proxy user name with a saved password is used instead
//! when there is one.
//!
//! No crate: `sspi` (the Rust one) carries its own NTLM and Kerberos and
//! a large dependency tree, for what is a handful of calls into
//! `secur32.dll` here.

use std::ffi::c_void;

use windows::core::PCWSTR;
use windows::Win32::Security::Authentication::Identity::{
    AcquireCredentialsHandleW, DeleteSecurityContext, FreeContextBuffer, FreeCredentialsHandle,
    InitializeSecurityContextW, SecBuffer, SecBufferDesc, ISC_REQ_ALLOCATE_MEMORY, ISC_REQ_CONNECTION, SECBUFFER_TOKEN,
    SECBUFFER_VERSION, SECPKG_CRED_OUTBOUND, SECURITY_NATIVE_DREP,
};
use windows::Win32::Security::Credentials::SecHandle;
use windows::Win32::System::Rpc::{SEC_WINNT_AUTH_IDENTITY_UNICODE, SEC_WINNT_AUTH_IDENTITY_W};

/// `SEC_I_CONTINUE_NEEDED`: the proxy's answer is needed for the next
/// token.
const CONTINUE_NEEDED: i32 = 0x0009_0312;
/// `SEC_E_NO_CREDENTIALS`: Windows has nothing to log in with. An account
/// signed in with a Microsoft account or a PIN has no password Windows
/// can offer a proxy, and neither has one running as a service account.
const NO_CREDENTIALS: i32 = -0x7FF6_FCF2; // 0x8009030E
/// `SEC_E_LOGON_DENIED`: the credentials are there and were refused.
const LOGON_DENIED: i32 = -0x7FF6_FCEE; // 0x80090312 is continue; denied is 0x8009030C
/// `SEC_E_INVALID_TOKEN`: the proxy's token made no sense.
const INVALID_TOKEN: i32 = -0x7FF6_FCF8; // 0x80090308

/// A status in words, so the tab says something a person can act on.
fn message(status: i32) -> String {
    match status {
        NO_CREDENTIALS => crate::t!("sspi-no-credentials"),
        LOGON_DENIED => crate::t!("sspi-denied"),
        INVALID_TOKEN => crate::t!("sspi-bad-token"),
        other => format!("0x{other:08x}"),
    }
}

/// The schemes this can answer, best first (the proxy's list is matched
/// against it without case).
pub const SCHEMES: [&str; 2] = ["Negotiate", "NTLM"];

/// Whether NativeTerm can answer one of the proxy's schemes.
#[must_use]
pub fn supported(schemes: &[String]) -> Option<&'static str> {
    SCHEMES.into_iter().find(|ours| schemes.iter().any(|theirs| theirs.eq_ignore_ascii_case(ours)))
}

/// A login in progress: the credentials and the security context, freed
/// when it is dropped.
pub struct Handshake {
    credentials: SecHandle,
    /// The context, once the first round made it: the same handle is
    /// given to every further round (as `phContext` and `phNewContext`),
    /// which is how SSPI carries the login along.
    context: SecHandle,
    started: bool,
    /// Windows has said its part: there is no further token to send.
    done: bool,
    /// `HTTP/<proxy host>`, what the ticket is asked for.
    target: Vec<u16>,
    /// Kept alive, and in one place, while the credentials refer to it:
    /// the identity holds pointers into its own strings, and SSPI may
    /// keep the address it was given, so it must not move with the
    /// handshake.
    _secret: Option<Box<Secret>>,
}

/// A user name and password handed to SSPI, zeroed when dropped.
struct Secret {
    user: Vec<u16>,
    domain: Vec<u16>,
    password: Vec<u16>,
    identity: SEC_WINNT_AUTH_IDENTITY_W,
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.password.fill(0);
        self.user.fill(0);
        self.domain.fill(0);
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `DOMAIN\user`, `user@domain` or a plain user name.
fn split_user(user: &str) -> (String, String) {
    if let Some((domain, name)) = user.split_once('\\') {
        return (name.to_string(), domain.to_string());
    }
    (user.to_string(), String::new())
}

impl Handshake {
    /// Start a login with `package` ("Negotiate" or "NTLM") against the
    /// proxy at `host`. `login`: a user name and password to use instead
    /// of the signed-in person's own.
    pub fn start(package: &str, host: &str, login: Option<(&str, &str)>) -> Result<Handshake, String> {
        let package_name = wide(package);
        let mut credentials = SecHandle::default();
        let secret = login.map(|(user, password)| {
            let (name, domain) = split_user(user);
            let mut secret = Box::new(Secret {
                user: wide(&name),
                domain: wide(&domain),
                password: wide(password),
                identity: SEC_WINNT_AUTH_IDENTITY_W::default(),
            });
            // the lengths exclude the terminating zero
            secret.identity.User = secret.user.as_mut_ptr();
            secret.identity.UserLength = (secret.user.len() - 1) as u32;
            secret.identity.Domain = secret.domain.as_mut_ptr();
            secret.identity.DomainLength = (secret.domain.len() - 1) as u32;
            secret.identity.Password = secret.password.as_mut_ptr();
            secret.identity.PasswordLength = (secret.password.len() - 1) as u32;
            secret.identity.Flags = SEC_WINNT_AUTH_IDENTITY_UNICODE;
            secret
        });
        let auth_data = secret.as_ref().map(|s| std::ptr::from_ref(&s.identity).cast::<c_void>());
        // SAFETY: the package name and the identity (whose strings this
        // keeps alive next to it) outlive the call; the handle is freed
        // on drop.
        let result = unsafe {
            AcquireCredentialsHandleW(
                PCWSTR::null(),
                PCWSTR(package_name.as_ptr()),
                SECPKG_CRED_OUTBOUND,
                None,
                auth_data,
                None,
                None,
                &mut credentials,
                None,
            )
        };
        if let Err(e) = result {
            return Err(format!("{package}: {}", message(e.code().0)));
        }
        Ok(Handshake {
            credentials,
            context: SecHandle::default(),
            started: false,
            done: false,
            target: wide(&format!("HTTP/{host}")),
            _secret: secret,
        })
    }

    /// Whether Windows has nothing more to send (the last token was the
    /// final one): a proxy still refusing after that has refused the
    /// person, not the handshake.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.done
    }

    /// The next token to send, from the proxy's last one (`None` on the
    /// first round). `None` back means the login is finished.
    pub fn next(&mut self, challenge: Option<&[u8]>) -> Result<Option<Vec<u8>>, String> {
        let mut input = challenge.map(|token| SecBuffer {
            cbBuffer: token.len() as u32,
            BufferType: SECBUFFER_TOKEN,
            pvBuffer: token.as_ptr() as *mut c_void,
        });
        let input_desc =
            input.as_mut().map(|buffer| SecBufferDesc { ulVersion: SECBUFFER_VERSION, cBuffers: 1, pBuffers: buffer });
        let mut output = SecBuffer { cbBuffer: 0, BufferType: SECBUFFER_TOKEN, pvBuffer: std::ptr::null_mut() };
        let mut output_desc = SecBufferDesc { ulVersion: SECBUFFER_VERSION, cBuffers: 1, pBuffers: &mut output };
        let previous = self.started.then(|| std::ptr::from_ref(&self.context));
        let mut attributes = 0u32;
        // SAFETY: both descriptors and the target name live through the
        // call; the token SSPI allocates is copied out and freed below.
        let status = unsafe {
            InitializeSecurityContextW(
                Some(&self.credentials),
                previous,
                Some(self.target.as_ptr()),
                // only the token is wanted: asking for confidentiality
                // makes Windows refuse (SEC_E_UNSUPPORTED_FUNCTION) when
                // the other side didn't offer sealing, and nothing here
                // is encrypted by SSPI anyway
                ISC_REQ_CONNECTION | ISC_REQ_ALLOCATE_MEMORY,
                0,
                SECURITY_NATIVE_DREP,
                input_desc.as_ref().map(std::ptr::from_ref),
                0,
                Some(&mut self.context),
                Some(&mut output_desc),
                &mut attributes,
                None,
            )
        };
        self.started = true;
        self.done = status.0 == 0;
        let token = match output.pvBuffer.is_null() || output.cbBuffer == 0 {
            true => None,
            // SAFETY: SSPI says how long its buffer is; it is freed right after.
            false => unsafe {
                let token = std::slice::from_raw_parts(output.pvBuffer.cast::<u8>(), output.cbBuffer as usize).to_vec();
                let _ = FreeContextBuffer(output.pvBuffer);
                Some(token)
            },
        };
        match status.0 {
            0 | CONTINUE_NEEDED => Ok(token),
            other => Err(message(other)),
        }
    }
}

impl Drop for Handshake {
    fn drop(&mut self) {
        // SAFETY: handles this made, each freed once.
        unsafe {
            if self.started {
                let _ = DeleteSecurityContext(std::ptr::from_ref(&self.context));
            }
            let _ = FreeCredentialsHandle(std::ptr::from_ref(&self.credentials));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_best_scheme_we_can_answer_is_picked() {
        let offered = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(supported(&offered(&["Basic", "NTLM"])), Some("NTLM"));
        assert_eq!(supported(&offered(&["ntlm", "negotiate"])), Some("Negotiate"), "Kerberos first");
        assert_eq!(supported(&offered(&["Basic", "Digest"])), None);
        assert_eq!(supported(&[]), None);
    }

    #[test]
    fn a_user_name_is_split_the_way_windows_writes_it() {
        assert_eq!(split_user(r"CORP\alice"), ("alice".to_string(), "CORP".to_string()));
        assert_eq!(split_user("alice"), ("alice".to_string(), String::new()));
        assert_eq!(split_user("alice@corp.example"), ("alice@corp.example".to_string(), String::new()));
    }

    /// A type 2 message as a server sends one: a target name, a
    /// challenge, and a target info block (NTLMv2 needs it).
    fn fake_challenge() -> Vec<u8> {
        let target: Vec<u8> = "GW".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let info: Vec<u8> = vec![0, 0, 0, 0]; // MsvAvEOL
        let target_at = 56u32;
        let info_at = target_at + target.len() as u32;
        let mut m = Vec::new();
        m.extend(b"NTLMSSP\0");
        m.extend(2u32.to_le_bytes());
        m.extend((target.len() as u16).to_le_bytes());
        m.extend((target.len() as u16).to_le_bytes());
        m.extend(target_at.to_le_bytes());
        // unicode | request target | NTLM | always sign | target type server
        // | extended session security | target info
        m.extend(0x0088_8205u32.to_le_bytes());
        m.extend([1, 2, 3, 4, 5, 6, 7, 8]); // the challenge
        m.extend([0u8; 8]); // reserved
        m.extend((info.len() as u16).to_le_bytes());
        m.extend((info.len() as u16).to_le_bytes());
        m.extend(info_at.to_le_bytes());
        m.extend([6, 1, 0, 0, 0, 0, 0, 15]); // version
        m.extend(target);
        m.extend(info);
        m
    }

    #[test]
    fn a_challenge_is_answered_with_a_typed_login() {
        let mut h = Handshake::start("NTLM", "gw.corp", Some((r"CORPlice", "secret"))).unwrap();
        assert!(h.next(None).unwrap().is_some());
        let answer = h.next(Some(&fake_challenge())).expect("second leg").expect("a type 3");
        assert_eq!(answer[8..12], 3u32.to_le_bytes());
    }

    /// The same with the signed-in person's own credentials. Windows
    /// has none to offer on an account signed in with a Microsoft
    /// account or a PIN (this machine is one), and then it must say so
    /// in words rather than a number.
    #[test]
    fn a_challenge_is_answered_with_the_signed_in_person() {
        let mut h = Handshake::start("NTLM", "gw.corp", None).unwrap();
        let first = h.next(None).unwrap().unwrap();
        assert!(first.starts_with(b"NTLMSSP\0"));
        match h.next(Some(&fake_challenge())) {
            Ok(Some(answer)) => assert_eq!(answer[8..12], 3u32.to_le_bytes()),
            Ok(None) => panic!("no token and no error"),
            Err(e) => assert_eq!(e, crate::t!("sspi-no-credentials"), "the only failure that is expected here"),
        }
    }

    /// Windows makes a first token for the signed-in user without asking
    /// anyone anything (no proxy involved).
    #[test]
    fn windows_makes_a_first_token() {
        let mut handshake = Handshake::start("NTLM", "proxy.example", None).expect("NTLM is always there");
        let token = handshake.next(None).expect("a token").expect("not empty");
        assert!(token.starts_with(b"NTLMSSP\0"), "{:?}", &token[..token.len().min(8)]);
    }
}
