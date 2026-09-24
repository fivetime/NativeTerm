//! NativeTerm's tab menu for a terminal that shows it itself.
//!
//! On Windows the menu is drawn over the Terminal window. WezTerm has no
//! tab strip to draw over, but it runs a configuration that binds a
//! right click (and Ctrl+Shift+M) to a menu of its own; the menu's
//! items come from here: `--tab-menu` asks NativeTerm what applies to
//! the tab's session and prints one line per item
//! (`id<TAB>text<TAB>flags<TAB>icon`: a heading with id 0, `d` in the
//! flags for a disabled item, a nerdfont name for the icon; `-` alone for
//! a separator), and `--tab-menu <id>` reports the choice. The tab is
//! named by `--pane` (WezTerm's pane id, which the session's shim
//! reported as its terminal session), since the menu runs in the GUI
//! process, not in the tab.

use std::time::Duration;

use native_term_session::pipe;
use native_term_session::protocol::{AppMessage, Role, ShimMessage};

/// One line per item, as the menu's script reads them.
pub fn lines(items: &[native_term_session::protocol::MenuItem]) -> String {
    let clean = |s: &str| s.replace(['\n', '\t'], " ");
    items
        .iter()
        .map(|i| {
            if i.separator {
                return "-\n".to_string();
            }
            let flags = if i.enabled { "" } else { "d" };
            format!("{}\t{}\t{flags}\t{}\n", i.id, clean(&i.text), clean(i.icon.as_deref().unwrap_or("")))
        })
        .collect()
}

/// Exit code 0 when NativeTerm answered (the items are on stdout for a
/// listing), 1 when it could not be reached or knows no such tab.
pub fn run(id: Option<u32>, pane: Option<String>) -> i32 {
    let Some(name) = crate::pipe_name() else { return 1 };
    let Ok(conn) = pipe::connect(&name, Duration::from_millis(500)) else { return 1 };
    let hello = ShimMessage::Hello {
        protocol: native_term_session::PROTOCOL_VERSION,
        role: Role::Request,
        pid: std::process::id(),
        wt_session: pane.or_else(crate::wt_session),
        session: None,
        alias: None,
        terminal_window: None,
    };
    let request = match id {
        Some(id) => ShimMessage::TabAction { id },
        None => ShimMessage::TabMenu,
    };
    if conn.send(&hello).is_err() || conn.send(&request).is_err() {
        return 1;
    }
    if id.is_some() {
        // NativeTerm checks who asked before it acts: stay until it hangs up
        let _ = conn.recv::<AppMessage>(Duration::from_millis(300));
        return 0;
    }
    // `Welcome`, then the menu
    for _ in 0..2 {
        match conn.recv::<AppMessage>(Duration::from_secs(3)) {
            Ok(Some(AppMessage::TabMenu { items })) => {
                print!("{}", lines(&items));
                return 0;
            }
            Ok(Some(_)) => continue,
            _ => return 1,
        }
    }
    1
}

/// The tab's hover card, as `title<TAB>note` on stdout, or `off` when
/// cards are off. Exit code 0 when NativeTerm answered, 1 when it could not
/// be reached.
pub fn card(pane: Option<String>) -> i32 {
    let Some(name) = crate::pipe_name() else { return 1 };
    let Ok(conn) = pipe::connect(&name, Duration::from_millis(500)) else { return 1 };
    let hello = ShimMessage::Hello {
        protocol: native_term_session::PROTOCOL_VERSION,
        role: Role::Request,
        pid: std::process::id(),
        wt_session: pane.or_else(crate::wt_session),
        session: None,
        alias: None,
        terminal_window: None,
    };
    if conn.send(&hello).is_err() || conn.send(&ShimMessage::TabCard).is_err() {
        return 1;
    }
    // `Welcome`, then the card
    for _ in 0..2 {
        match conn.recv::<AppMessage>(Duration::from_secs(3)) {
            Ok(Some(AppMessage::TabCard { title, note, show })) => {
                print!("{}", card_line(&title, &note, show));
                return 0;
            }
            Ok(Some(_)) => continue,
            _ => return 1,
        }
    }
    1
}

fn card_line(title: &str, note: &str, show: bool) -> String {
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    if show {
        format!("{}\t{}\n", clean(title), clean(note))
    } else {
        "off\n".into()
    }
}

#[cfg(test)]
mod tests {
    use native_term_session::protocol::MenuItem;

    #[test]
    fn one_line_per_item() {
        let items = [
            MenuItem::new(0, "web01"),
            MenuItem { icon: Some("cod_close".into()), ..MenuItem::new(4, "Close\ttab\n") },
            MenuItem::separator(),
            MenuItem { enabled: false, ..MenuItem::new(2, "Disconnect") },
        ];
        assert_eq!(super::lines(&items), "0\tweb01\t\t\n4\tClose tab \t\tcod_close\n-\n2\tDisconnect\td\t\n");
        assert_eq!(super::lines(&[]), "");
    }

    #[test]
    fn a_card_line() {
        assert_eq!(super::card_line("web01", "Connected\t· 5 min", true), "web01\tConnected · 5 min\n");
        assert_eq!(super::card_line("", "", true), "\t\n", "no session: the terminal's own card");
        assert_eq!(super::card_line("web01", "x", false), "off\n");
    }
}
