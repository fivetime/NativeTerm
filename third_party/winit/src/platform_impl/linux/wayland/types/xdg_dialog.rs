//! (NativeTerm) `xdg_wm_dialog_v1`: a toplevel that has a parent marked a
//! modal dialog of it, so that the compositor keeps the parent from input
//! and the dialog with it (GNOME 46 and later, KDE Plasma 6.1 and later).

use sctk::globals::GlobalData;
use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::{delegate_dispatch, Connection, Dispatch, Proxy, QueueHandle};
use sctk::reexports::protocols::xdg::shell::client::xdg_toplevel::XdgToplevel;
use wayland_protocols::xdg::dialog::v1::client::xdg_dialog_v1::XdgDialogV1;
use wayland_protocols::xdg::dialog::v1::client::xdg_wm_dialog_v1::XdgWmDialogV1;

use crate::platform_impl::wayland::state::WinitState;

#[derive(Debug)]
pub struct XdgDialogState {
    manager: XdgWmDialogV1,
}

impl XdgDialogState {
    pub fn bind(globals: &GlobalList, queue_handle: &QueueHandle<WinitState>) -> Result<Self, BindError> {
        let manager = globals.bind(queue_handle, 1..=1, GlobalData)?;
        Ok(Self { manager })
    }

    /// `toplevel` a modal dialog (of the parent it was given): the object
    /// is kept as long as it is to stay one.
    pub fn modal(&self, toplevel: &XdgToplevel, queue_handle: &QueueHandle<WinitState>) -> XdgDialogV1 {
        let dialog = self.manager.get_xdg_dialog(toplevel, queue_handle, GlobalData);
        dialog.set_modal();
        dialog
    }
}

impl Dispatch<XdgWmDialogV1, GlobalData, WinitState> for XdgDialogState {
    fn event(
        _: &mut WinitState,
        _: &XdgWmDialogV1,
        _: <XdgWmDialogV1 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
    }
}

impl Dispatch<XdgDialogV1, GlobalData, WinitState> for XdgDialogState {
    fn event(
        _: &mut WinitState,
        _: &XdgDialogV1,
        _: <XdgDialogV1 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
    }
}

delegate_dispatch!(WinitState: [XdgWmDialogV1: GlobalData] => XdgDialogState);
delegate_dispatch!(WinitState: [XdgDialogV1: GlobalData] => XdgDialogState);
