//! NativeTerm: the latest press on any seat (a pointer button, a key, a
//! touch), for the serial an activation token is asked with. As
//! Chromium's `SerialTracker` keeps it (ui/ozone/platform/wayland/host/
//! wayland_serial_tracker.cc: presses only, never a release): GNOME's
//! compositor honours a token only for the serial of the press that
//! started the pointer's grab (mutter's `pointer->grab_serial`), so the
//! serial of the release after it is refused.

use std::sync::Mutex;

use sctk::reexports::client::protocol::wl_seat::WlSeat;

static LATEST: Mutex<Option<(u32, WlSeat)>> = Mutex::new(None);

/// A press with this serial on `seat` (events come in order: the last one
/// recorded is the latest).
pub(crate) fn pressed(serial: u32, seat: &WlSeat) {
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = Some((serial, seat.clone()));
}

/// The latest press's serial and seat, if there was one.
pub(crate) fn latest() -> Option<(u32, WlSeat)> {
    LATEST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
