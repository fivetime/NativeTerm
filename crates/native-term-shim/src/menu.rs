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

/// How long the person may take to answer in NativeTerm's window
/// (NativeTerm gives the window up a little sooner).
const ANSWER: Duration = Duration::from_secs(660);

/// The characters for a paste as a quotation, as `between<TAB>chars`
/// (`1`: the text between them, `0`: after them) on stdout, or `cancel`
/// when the person said no. Exit code 0 when NativeTerm answered, 1 when it
/// could not be reached.
pub fn quotation() -> i32 {
    let Some(name) = crate::pipe_name() else { return 1 };
    let Ok(conn) = pipe::connect(&name, Duration::from_millis(500)) else { return 1 };
    let hello = ShimMessage::Hello {
        protocol: native_term_session::PROTOCOL_VERSION,
        role: Role::Request,
        pid: std::process::id(),
        wt_session: crate::wt_session(),
        session: None,
        alias: None,
        terminal_window: None,
    };
    if conn.send(&hello).is_err() || conn.send(&ShimMessage::PasteQuotation).is_err() {
        return 1;
    }
    // `Welcome`, then the answer
    for _ in 0..2 {
        match conn.recv::<AppMessage>(ANSWER) {
            Ok(Some(AppMessage::Quotation { chars, between, paste })) => {
                print!("{}", quotation_line(&chars, between, paste));
                return 0;
            }
            Ok(Some(_)) => continue,
            _ => return 1,
        }
    }
    1
}

/// What to find in the pane, as `find<TAB>up<TAB>case<TAB>word<TAB>wrap
/// <TAB>text` (each `1` or `0`) on stdout, or `cancel` when the person
/// closed the dialog. Waits for them, however long they take. Exit code 0
/// when NativeTerm answered, 1 when it could not be reached.
pub fn find(initial: Option<String>, result: Option<(u32, u32)>) -> i32 {
    let Some(name) = crate::pipe_name() else { return 1 };
    let Ok(conn) = pipe::connect(&name, Duration::from_millis(500)) else { return 1 };
    let hello = ShimMessage::Hello {
        protocol: native_term_session::PROTOCOL_VERSION,
        role: Role::Request,
        pid: std::process::id(),
        wt_session: crate::wt_session(),
        session: None,
        alias: None,
        terminal_window: None,
    };
    let result = result.map(|(position, count)| native_term_session::protocol::FindResult { position, count });
    if conn.send(&hello).is_err() || conn.send(&ShimMessage::Find { initial, result }).is_err() {
        return 1;
    }
    loop {
        match conn.recv::<AppMessage>(ANSWER) {
            Ok(Some(AppMessage::Find { find, text, match_case, whole_word, wrap, up })) => {
                print!("{}", find_line(find, &text, [up, match_case, whole_word, wrap]));
                return 0;
            }
            // `Welcome`; or nothing yet: the dialog is up, the person reads
            Ok(_) => continue,
            Err(_) => return 1,
        }
    }
}

fn find_line(find: bool, text: &str, [up, match_case, whole_word, wrap]: [bool; 4]) -> String {
    if !find || text.is_empty() {
        return "cancel\n".into();
    }
    let text = text.replace(['\n', '\r'], " ");
    format!("find\t{}\t{}\t{}\t{}\t{text}\n", u8::from(up), u8::from(match_case), u8::from(whole_word), u8::from(wrap))
}

fn quotation_line(chars: &str, between: bool, paste: bool) -> String {
    if paste {
        format!("{}\t{}\n", u8::from(between), chars.replace(['\n', '\r'], ""))
    } else {
        "cancel\n".into()
    }
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
    fn a_find_line() {
        assert_eq!(super::find_line(true, "eth0", [true, false, false, true]), "find\t1\t0\t0\t1\teth0\n");
        assert_eq!(
            super::find_line(true, "a\tb  c", [false, true, true, false]),
            "find\t0\t1\t1\t0\ta\tb  c\n",
            "the text is the line's rest, as it is"
        );
        assert_eq!(super::find_line(false, "eth0", [true; 4]), "cancel\n");
        assert_eq!(super::find_line(true, "", [true; 4]), "cancel\n");
    }

    #[test]
    fn a_quotation_line() {
        assert_eq!(super::quotation_line("\"", true, true), "1\t\"\n");
        assert_eq!(super::quotation_line("> \n", false, true), "0\t> \n", "on one line; blanks kept");
        assert_eq!(super::quotation_line("\t", true, true), "1\t\t\n", "a tab can be one");
        assert_eq!(super::quotation_line("\"", true, false), "cancel\n");
    }

    #[test]
    fn a_card_line() {
        assert_eq!(super::card_line("web01", "Connected\t· 5 min", true), "web01\tConnected · 5 min\n");
        assert_eq!(super::card_line("", "", true), "\t\n", "no session: the terminal's own card");
        assert_eq!(super::card_line("web01", "x", false), "off\n");
    }
}
