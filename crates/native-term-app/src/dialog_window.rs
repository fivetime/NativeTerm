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
//! - Wayland: the owner's toplevel the dialog's parent
//!   (`xdg_toplevel.set_parent`) and a modal one marked so
//!   (`xdg_wm_dialog_v1.set_modal`, where the compositor has it), both
//!   through the winit fork (`third_party/winit/NATIVETERM.md`); the input
//!   is kept from the owner by NativeTerm too.

use winit::event::WindowEvent;
use winit::window::{Window, WindowAttributes};

/// `attributes` for a window whose frame is the skin's (all but the
/// floating button, which is a shape of its own): on Windows 11 round
/// corners and the system's shadow, asked for and not left to the
/// system's default, which gives them only to a window that can be made
/// larger or smaller (so a dialog had square corners beside its rounded
/// owner). Elsewhere the window manager's.
pub fn skinned(attributes: WindowAttributes) -> WindowAttributes {
    #[cfg(windows)]
    {
        use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
        attributes.with_undecorated_shadow(true).with_corner_preference(CornerPreference::Round)
    }
    #[cfg(not(windows))]
    {
        attributes
    }
}

/// `attributes` for a window that is `owner`'s dialog (`None`: a window
/// of its own).
pub fn owned_by(attributes: WindowAttributes, owner: Option<(&Window, bool)>) -> WindowAttributes {
    let Some((owner, modal)) = owner else { return attributes };
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let _ = modal;
    #[cfg(windows)]
    {
        use winit::platform::windows::WindowAttributesExtWindows;
        match crate::window::handle_of(owner) {
            Some(hwnd) => attributes.with_owner_window(hwnd),
            None => attributes,
        }
    }
    // macOS: made a child window once shown (`shown_over`): AppKit shows
    // a window made with a parent at once, before AccessKit's adapter is
    // set up for it (which then panics)
    #[cfg(target_os = "macos")]
    {
        let _ = owner;
        attributes
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use winit::platform::x11::{WindowAttributesExtX11, WindowType};
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match owner.window_handle().map(|handle| handle.as_raw()) {
            // Wayland: the owner's toplevel its parent (the winit fork's
            // `xdg_toplevel.set_parent`); on X11 a parent window would make
            // it a window inside the owner's, so there it is said apart
            // (`dialog_of`)
            Ok(handle @ RawWindowHandle::Wayland(_)) => {
                use winit::platform::wayland::WindowAttributesExtWayland;
                // SAFETY: the owner outlives its dialogs (they are closed
                // before it, and with it)
                let attributes = unsafe { attributes.with_parent_window(Some(handle)) };
                // and a modal one said to be (`xdg-dialog-v1`, the fork's)
                attributes.with_modal(modal)
            }
            _ => attributes.with_x11_window_type(vec![WindowType::Dialog]),
        }
    }
}

/// What is said of a dialog's window once it is made and before it is
/// shown (X11: whose dialog it is, and whether modal).
pub fn dialog_of(window: &Window, owner: &Window, modal: bool) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let (Some(id), Some(owner_id)) = (x11_id(window), x11_id(owner)) {
            made_on_server(window);
            native_term_os::x11_activate::make_dialog(id, owner_id, modal);
        }
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = (window, owner, modal);
    }
}

/// A dialog just shown over its owner: macOS makes it the owner's child
/// window now (above it, moved with it). `window`, `owner`: the panes'
/// handles.
pub fn shown_over(window: isize, owner: isize) {
    #[cfg(target_os = "macos")]
    native_term_os::dock::attach_child(owner, window);
    #[cfg(not(target_os = "macos"))]
    let _ = (window, owner);
}

/// A dialog about to be closed: macOS takes it from its owner's child
/// windows first (see `detach_child`).
pub fn closing_over(window: isize, owner: isize) {
    #[cfg(target_os = "macos")]
    native_term_os::dock::detach_child(owner, window);
    #[cfg(not(target_os = "macos"))]
    let _ = (window, owner);
}

/// A window that never takes the keyboard (the floating button: only
/// clicked), so that showing it takes it from no one. X11: the input hint
/// (Windows and macOS: the button is made not to activate already).
pub fn never_focus(window: &Window) {
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(id) = x11_id(window) {
        made_on_server(window);
        // (it waits for the server: not on the event loop's thread)
        std::thread::spawn(move || {
            if !native_term_os::x11_activate::never_focus(id) {
                eprintln!("window {id:#x}: the input hint could not be set");
            }
        });
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let _ = window;
}

/// The X server has made `window`: winit asks for it on a connection of
/// its own and the request may still wait there, unsent; what is then
/// said of the window on another connection (its hints, whose dialog it
/// is) is refused as about no window (seen: the docking strip's input
/// hint missing now and then, and the strip then taking the keyboard).
/// A question winit has to wait for the answer to sends what waits.
#[cfg(all(unix, not(target_os = "macos")))]
fn made_on_server(window: &Window) {
    let _ = window.outer_position();
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
