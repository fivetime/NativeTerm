//! What ssh wrote to stderr, as text.

#[cfg(windows)]
pub use native_term_win::ssh_message as message;

/// A message ssh wrote to stderr, as text. ssh escapes bytes it won't
/// print as `\ooo` (octal): undo the escapes, then decode as UTF-8.
#[cfg(unix)]
pub fn message(bytes: &[u8]) -> String {
    let mut raw = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal =
            bytes.get(i + 1..i + 4).filter(|d| bytes[i] == b'\\' && d.iter().all(|c| (b'0'..=b'7').contains(c)));
        match octal {
            Some(d) => {
                let v = d.iter().fold(0u32, |v, c| v * 8 + u32::from(c - b'0'));
                raw.push(v as u8);
                i += 4;
            }
            None => {
                raw.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&raw).into_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn octal_escapes_are_undone() {
        // "中" in UTF-8, as ssh prints it
        assert_eq!(super::message(br"ssh: \344\270\255 x"), "ssh: 中 x");
        assert_eq!(super::message(b"plain\\"), "plain\\");
    }
}
