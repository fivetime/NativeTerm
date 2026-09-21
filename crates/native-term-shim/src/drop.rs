//! Files dropped into a tab.
//!
//! Windows Terminal answers a drop by pasting the file names into the tab.
//! The client (our ssh, or ntplink) spots a paste that is nothing but paths
//! of this machine, holds it back and starts this helper with the names;
//! here they are handed to NativeTerm, which asks what to do with them:
//! upload them to the session's folder, or send the text after all (it
//! types it into the tab, as "send text" does).
//!
//! The text is rebuilt here the way Terminal writes it, so that sending it
//! after all gives the session exactly what it would have had.

use std::time::Duration;

use native_term_session::pipe;
use native_term_session::protocol::{Role, ShimMessage};

/// The names as Terminal pastes them: separated by spaces, a name with a
/// space in quotes.
pub fn as_text(paths: &[String]) -> String {
    let mut text = String::new();
    for path in paths {
        if !text.is_empty() {
            text.push(' ');
        }
        if path.contains(' ') {
            text.push('"');
            text.push_str(path);
            text.push('"');
        } else {
            text.push_str(path);
        }
    }
    text
}

/// Hands the names to NativeTerm. Exit code 0 when it took them, 1 when it
/// could not be reached (the client has already dropped the text: nothing
/// else can be done with it here).
pub fn run(paths: &[String]) -> i32 {
    let name = match std::env::var("NATIVETERM_PIPE").ok().or_else(|| pipe::pipe_name().ok()) {
        Some(name) => name,
        None => return 1,
    };
    let Ok(conn) = pipe::connect(&name, Duration::from_millis(500)) else { return 1 };
    let hello = ShimMessage::Hello {
        protocol: native_term_session::PROTOCOL_VERSION,
        role: Role::Request,
        pid: std::process::id(),
        wt_session: std::env::var("WT_SESSION").ok().filter(|s| !s.is_empty()),
        session: None,
        alias: None,
        terminal_window: None,
    };
    let dropped = ShimMessage::Dropped { paths: paths.to_vec(), text: as_text(paths) };
    if conn.send(&hello).is_ok() && conn.send(&dropped).is_ok() {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_as_terminal_writes_it() {
        let paths = vec!["C:\\a.txt".to_string(), "C:\\a b\\c.log".to_string()];
        assert_eq!(as_text(&paths), "C:\\a.txt \"C:\\a b\\c.log\"");
        assert_eq!(as_text(&[]), "");
    }
}
