//! NativeTerm's language: the system's, or the one chosen in Settings
//! (kept in `state.db`). Switching takes effect at the next frame.

use std::sync::LazyLock;

use i18n_embed::fluent::FluentLanguageLoader;

pub static LOADER: LazyLock<FluentLanguageLoader> =
    LazyLock::new(|| native_term_i18n::loader("native_term_app", std::env::var("NATIVETERM_LANG").ok().as_deref()));

/// `state.db` setting: a language id, or absent for the system's.
pub const SETTING: &str = "language";

/// A message in the current language (`fl!`, keys checked at compile time).
#[macro_export]
macro_rules! t {
    ($($args:tt)*) => {
        i18n_embed_fl::fl!($crate::i18n::LOADER, $($args)*)
    };
}

/// `None`: follow the system.
pub fn set_language(choice: Option<&str>) {
    native_term_i18n::select(&LOADER, choice);
}

pub fn current() -> String {
    native_term_i18n::current(&LOADER)
}

pub use native_term_i18n::available;
