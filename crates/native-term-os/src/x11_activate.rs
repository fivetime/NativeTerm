//! Bringing one of NativeTerm's own X11 windows to the front and giving it
//! the keyboard, as Chromium's `X11Window::Activate` does
//! (ui/ozone/platform/x11/x11_window.cc): `_NET_ACTIVE_WINDOW` sent to the
//! root window, "we're an app" (source 1), with the X server's current
//! time (`X11EventSource::GetCurrentServerTime`: a no-op property change on
//! a window of its own, the time read from the PropertyNotify it makes).
//! A window manager that keeps the focus where the person last clicked
//! (KWin's focus stealing prevention) takes a request without that time for
//! an old one and puts a new window below the active one; with the
//! server's time it activates it. winit's own request has no time.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
    WindowClass,
};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;

/// Ask the window manager to activate `window` (an X11 window id). False
/// where there is no X server to ask, or it didn't answer.
pub fn activate(window: u32) -> bool {
    let Ok((conn, screen)) = x11rb::connect(None) else { return false };
    let root = conn.setup().roots[screen].root;
    let Some(time) = server_time(&conn, root) else { return false };
    let Ok(atom) = conn.intern_atom(false, b"_NET_ACTIVE_WINDOW") else { return false };
    let Ok(atom) = atom.reply() else { return false };
    let event = ClientMessageEvent::new(32, window, atom.atom, [1, time, 0, 0, 0]);
    let sent = conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, event);
    sent.is_ok() && conn.flush().is_ok()
}

/// Make `window` (an X11 window id, not mapped yet) `parent`'s dialog,
/// as GTK and Qt make theirs: `WM_TRANSIENT_FOR` the parent (the window
/// manager keeps it above the parent and with it), the window type
/// `_NET_WM_WINDOW_TYPE_DIALOG`, and with `modal` the state
/// `_NET_WM_STATE_MODAL`. False where there is no X server to ask.
pub fn make_dialog(window: u32, parent: u32, modal: bool) -> bool {
    let Ok((conn, _)) = x11rb::connect(None) else { return false };
    let atom = |name: &[u8]| conn.intern_atom(false, name).ok()?.reply().ok().map(|a| a.atom);
    let set = || -> Option<()> {
        conn.change_property32(PropMode::REPLACE, window, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, &[parent])
            .ok()?;
        let kind = atom(b"_NET_WM_WINDOW_TYPE")?;
        let dialog = atom(b"_NET_WM_WINDOW_TYPE_DIALOG")?;
        conn.change_property32(PropMode::REPLACE, window, kind, AtomEnum::ATOM, &[dialog]).ok()?;
        if modal {
            let state = atom(b"_NET_WM_STATE")?;
            let modal = atom(b"_NET_WM_STATE_MODAL")?;
            conn.change_property32(PropMode::REPLACE, window, state, AtomEnum::ATOM, &[modal]).ok()?;
        }
        conn.flush().ok()
    };
    set().is_some()
}

/// Tell the window manager never to give `window` (an X11 window id, not
/// mapped yet) the keyboard (ICCCM's `WM_HINTS` input hint false): a
/// floating button that is only clicked, so that showing it takes the
/// keyboard from no one (a dialog that just opened, above all). The
/// other hints winit set are kept.
pub fn never_focus(window: u32) -> bool {
    use x11rb::properties::WmHints;
    let Ok((conn, _)) = x11rb::connect(None) else { return false };
    let hints = WmHints::get(&conn, window).ok().and_then(|cookie| cookie.reply().ok()).flatten();
    let mut hints = hints.unwrap_or_default();
    hints.input = Some(false);
    hints.set(&conn, window).is_ok() && conn.flush().is_ok()
}

/// The X server's current time: a property changed on a window of this
/// connection's own, and the time of the PropertyNotify that says so.
fn server_time(conn: &impl Connection, root: u32) -> Option<u32> {
    let window = conn.generate_id().ok()?;
    conn.create_window(
        0,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new().override_redirect(1).event_mask(EventMask::PROPERTY_CHANGE),
    )
    .ok()?;
    conn.change_window_attributes(window, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE))
        .ok()?;
    let atom = conn.intern_atom(false, b"NATIVETERM_TIMESTAMP").ok()?.reply().ok()?.atom;
    conn.change_property(PropMode::REPLACE, window, atom, AtomEnum::STRING, 8, 1, &[0]).ok()?;
    conn.flush().ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut time = None;
    while std::time::Instant::now() < deadline {
        match conn.poll_for_event() {
            Ok(Some(Event::PropertyNotify(e))) if e.window == window => {
                time = Some(e.time);
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(2)),
            Err(_) => break,
        }
    }
    let _ = conn.destroy_window(window);
    let _ = conn.flush();
    time
}
