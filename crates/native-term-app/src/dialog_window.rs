//! A dialog's window and the window it belongs to, as each window system
//! has it (see `window::open_dialog`): the window system keeps a dialog
//! above its owner and with it, and a modal one keeps the owner from
//! input until it is closed.
//!
//! - Windows: the owner window (`WS_POPUP` owned by it: above it, hidden
//!   and shown with it), and `EnableWindow(owner, FALSE)` while a modal
//!   one is open, as `DialogBox` does; a click on the disabled owner
//!   brings the dialog forward, Windows' own way.
//! - X11: `WM_TRANSIENT_FOR` the owner, the type `_NET_WM_WINDOW_TYPE_DIALOG`
//!   and `_NET_WM_STATE_MODAL` (what GTK and Qt set); the input kept from
//!   the owner by NativeTerm itself (`blocks`).
//! - macOS: a child window of the owner (`addChildWindow`: above it, moved
//!   with it); the input kept from the owner by NativeTerm.
//! - Wayland: winit has no parent for a toplevel yet; the dialog is a
//!   window of its own and the input is kept from the owner by NativeTerm.

use winit::event::WindowEvent;
use winit::window::{Window, WindowAttributes};

/// `attributes` for a window that is `owner`'s dialog (`None`: a window
/// of its own).
pub fn owned_by(attributes: WindowAttributes, owner: Option<(&Window, bool)>) -> WindowAttributes {
    let Some((owner, _)) = owner else { return attributes };
    #[cfg(windows)]
    {
        use winit::platform::windows::WindowAttributesExtWindows;
        match crate::window::handle_of(owner) {
            Some(hwnd) => attributes.with_owner_window(hwnd),
            None => attributes,
        }
    }
    #[cfg(target_os = "macos")]
    {
        use winit::raw_window_handle::HasWindowHandle;
        match owner.window_handle() {
            // SAFETY: the owner outlives its dialogs (they are closed
            // before it, and with it)
            Ok(handle) => unsafe { attributes.with_parent_window(Some(handle.as_raw())) },
            Err(_) => attributes,
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use winit::platform::x11::{WindowAttributesExtX11, WindowType};
        let _ = owner;
        attributes.with_x11_window_type(vec![WindowType::Dialog])
    }
}

/// What is said of a dialog's window once it is made and before it is
/// shown (X11: whose dialog it is, and whether modal).
pub fn dialog_of(window: &Window, owner: &Window, modal: bool) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let (Some(window), Some(owner)) = (x11_id(window), x11_id(owner)) {
            native_term_os::x11_activate::make_dialog(window, owner, modal);
        }
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = (window, owner, modal);
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn x11_id(window: &Window) -> Option<u32> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok(),
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        _ => None,
    }
}

/// Whether `event` is input a window with a modal dialog over it doesn't
/// take (what is pressed, typed, scrolled; the pointer's moves too, so
/// that nothing in it lights up under it).
pub fn is_input(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::CursorMoved { .. }
            | WindowEvent::KeyboardInput { .. }
            | WindowEvent::Ime(_)
            | WindowEvent::Touch(_)
            | WindowEvent::PinchGesture { .. }
            | WindowEvent::RotationGesture { .. }
            | WindowEvent::PanGesture { .. }
            | WindowEvent::DoubleTapGesture { .. }
    )
}

/// Whether it is a press: the dialog is brought forward for it.
pub fn is_press(event: &WindowEvent) -> bool {
    matches!(event, WindowEvent::MouseInput { state: winit::event::ElementState::Pressed, .. })
}

/// Where a dialog `size` large goes over its owner: its middle on the
/// owner's (both in the screen's pixels: the owner's top left corner and
/// size).
pub fn centered(owner_at: (i32, i32), owner_size: (u32, u32), size: (u32, u32)) -> (i32, i32) {
    let half = |a: u32, b: u32| (i64::from(a) - i64::from(b)) / 2;
    (owner_at.0 + half(owner_size.0, size.0) as i32, owner_at.1 + (half(owner_size.1, size.1) as i32).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dialog_in_its_owners_middle() {
        assert_eq!(centered((100, 50), (800, 600), (400, 300)), (300, 200));
        // taller than its owner: its top at the owner's
        assert_eq!(centered((100, 50), (800, 200), (400, 300)), (300, 50));
    }
}
