//! The shim's messages: the system's language, or `NATIVETERM_LANG`
//! (e.g. `en`, `zh-CN`).

use std::sync::LazyLock;

use i18n_embed::fluent::FluentLanguageLoader;

pub static LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    native_term_i18n::loader("native_term_shim", std::env::var("NATIVETERM_LANG").ok().as_deref())
});

/// A message in the current language (`fl!`, keys checked at compile time).
#[macro_export]
macro_rules! t {
    ($($args:tt)*) => {
        i18n_embed_fl::fl!($crate::i18n::LOADER, $($args)*)
    };
}
