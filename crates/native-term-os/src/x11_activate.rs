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
