# winit 0.30.13, with three changes

The crates.io release of winit 0.30.13 (Apache-2.0, `LICENSE`), without
its examples, tests and documentation, used through `[patch.crates-io]`
in the workspace's `Cargo.toml`.

The change: `Window::request_activation_token` on Wayland
(`src/platform_impl/linux/wayland/window/mod.rs`) sets the seat and the
serial of the latest press (a pointer button, a key or a touch, kept in
`seat/press.rs`) on the token it asks for, as Chromium does
(`ui/ozone/platform/wayland/host/xdg_activation.cc`, with the presses
its `SerialTracker` keeps). Without a serial GNOME's compositor takes
the token for a request for attention only: the window NativeTerm
brings forward with it (WezTerm's) got a badge on its dock icon instead
(seen on Zorin OS 18, GNOME 46, 2026-09-28). The serial must be a
press's: mutter 46 honours the serial of the press that started the
pointer's grab (`pointer->grab_serial`, `meta-wayland-pointer.c`), and
winit's own `latest_button_serial` is the release's after a click.
winit's master had the same code then.

It goes when a winit release sets the serial (offering the change
upstream is for the person to decide).

The second (2026-10-01): a window made on Wayland with
`WindowAttributes::with_parent_window` given the parent's Wayland
handle has the parent's toplevel as its parent
(`xdg_toplevel.set_parent`, `src/platform_impl/linux/wayland/window/mod.rs`),
which winit ignored there: NativeTerm's dialogs are their owners' on
Wayland too, kept above them by the compositor as X11's
`WM_TRANSIENT_FOR` and Windows' owner window do elsewhere.

The third (2026-10-01): `WindowAttributesExtWayland::with_modal` makes
such a window a modal dialog of its parent (`xdg_wm_dialog_v1`, the
staging protocol `xdg-dialog-v1`, `types/xdg_dialog.rs`), where the
compositor has it (GNOME 46 and later, KDE Plasma 6.1 and later): the
compositor keeps the parent from input, as `_NET_WM_STATE_MODAL` asks
on X11.
