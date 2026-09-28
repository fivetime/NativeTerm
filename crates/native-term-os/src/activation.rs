//! Tokens that let another program's window come to the front, where
//! the windowing system asks for one (Wayland's xdg-activation-v1: only
//! the program that has the person's input may ask the compositor for
//! one, and hands it to the program whose window is to come forward, in
//! `XDG_ACTIVATION_TOKEN`). The window that asks is NativeTerm's main
//! window (`window.rs` sets the source); whatever brings a terminal's
//! window forward takes a token from here.

use std::sync::{Mutex, OnceLock};

/// The variable a token goes to another program in.
pub const ENV: &str = "XDG_ACTIVATION_TOKEN";

type Source = Box<dyn Fn() -> Option<String> + Send + Sync>;

static SOURCE: OnceLock<Mutex<Option<Source>>> = OnceLock::new();

fn source() -> std::sync::MutexGuard<'static, Option<Source>> {
    SOURCE.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner())
}

/// Where tokens come from (the main window, asked on its own thread).
pub fn set_source(get: impl Fn() -> Option<String> + Send + Sync + 'static) {
    *source() = Some(Box::new(get));
}

/// A fresh token, where there is a source and the system has them; each
/// is good for one window, once.
#[must_use]
pub fn token() -> Option<String> {
    source().as_ref().and_then(|get| get())
}
