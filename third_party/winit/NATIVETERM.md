# winit 0.30.13, with one change

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
