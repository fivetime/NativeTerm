//! This computer and this user, by name.

/// The computer's name (`COMPUTERNAME`; the host name elsewhere), or
/// empty when it has none.
#[must_use]
pub fn name() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMPUTERNAME").unwrap_or_default()
    }
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer and its length go together; the name is
        // NUL-terminated within it (or truncated, still within it).
        if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
            return String::new();
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    }
}

/// The user's account name, or empty when it isn't known.
#[must_use]
pub fn user() -> String {
    let var = if cfg!(windows) { "USERNAME" } else { "USER" };
    std::env::var(var).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_computer_has_a_name() {
        let name = super::name();
        assert!(!name.is_empty());
        assert!(!name.contains('\0'));
    }
}
