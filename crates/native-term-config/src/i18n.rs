//! What this library says when it refuses to write something. The
//! messages live with everything else NativeTerm says
//! (`native-term-i18n`, the `native_term_config` domain); the library
//! only picks the key and hands over the values it quotes.
//!
//! The language follows whoever is using the library: the program calls
//! `set_language` when the person chooses one.

use std::sync::LazyLock;

use i18n_embed::fluent::FluentLanguageLoader;

pub static LOADER: LazyLock<FluentLanguageLoader> =
    LazyLock::new(|| native_term_i18n::loader("native_term_config", std::env::var("NATIVETERM_LANG").ok().as_deref()));

/// A message in the current language (`fl!`, keys checked at compile time).
#[macro_export]
macro_rules! t {
    ($($args:tt)*) => {
        i18n_embed_fl::fl!($crate::i18n::LOADER, $($args)*)
    };
}

/// Follow this language from now on (`None`: the system's). The program
/// that uses this library calls it whenever its own language changes.
pub fn set_language(choice: Option<&str>) {
    native_term_i18n::select(&LOADER, choice);
}
