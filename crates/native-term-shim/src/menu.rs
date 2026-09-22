//! NativeTerm's tab menu for a terminal that shows it itself.
//!
//! On Windows the menu is drawn over the Terminal window. WezTerm has no
//! tab strip to draw over, but it runs a configuration that binds a
//! right click (and Ctrl+Shift+M) to a picker of its own; the picker's
//! items come from here: `--tab-menu` asks NativeTerm what applies to
//! the tab's session and prints one line per item (`id<TAB>text`, a
//! heading with id 0), and `--tab-menu <id>` reports the choice. The tab
//! is named by `--pane` (WezTerm's pane id, which the session's shim
//! reported as its terminal session), since the picker runs in the GUI
//! process, not in the tab.

use std::time::Duration;

use native_term_session::pipe;
use native_term_session::protocol::{AppMessage, Role, ShimMessage};

/// One line per item, as the picker's script reads them.
pub fn lines(items: &[native_term_session::protocol::MenuItem]) -> String {
    items.iter().map(|i| format!("{}\t{}\n", i.id, i.text.replace(['\n', '\t'], " "))).collect()
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

#[cfg(test)]
mod tests {
    use native_term_session::protocol::MenuItem;

    #[test]
    fn one_line_per_item() {
        let items = [MenuItem { id: 0, text: "web01".into() }, MenuItem { id: 4, text: "Close\ttab\n".into() }];
        assert_eq!(super::lines(&items), "0\tweb01\n4\tClose tab \n");
        assert_eq!(super::lines(&[]), "");
    }
}
