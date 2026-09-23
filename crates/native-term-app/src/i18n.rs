//! NativeTerm's language: the system's, or the one chosen in Settings
//! (kept in `state.db`). Switching takes effect at the next frame.

use std::sync::LazyLock;

use i18n_embed::fluent::FluentLanguageLoader;

pub static LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader = native_term_i18n::loader("native_term_app", std::env::var("NATIVETERM_LANG").ok().as_deref());
    if !readable(&loader) {
        native_term_config::i18n::set_language(Some(native_term_i18n::FALLBACK));
    }
    loader
});

/// `state.db` setting: a language id, or absent for the system's.
pub const SETTING: &str = "language";

/// A message in the current language (`fl!`, keys checked at compile time).
#[macro_export]
macro_rules! t {
    ($($args:tt)*) => {
        i18n_embed_fl::fl!($crate::i18n::LOADER, $($args)*)
    };
}

/// `None`: follow the system. The configuration library says things to
/// the person as well, so it is switched with us.
pub fn set_language(choice: Option<&str>) {
    native_term_i18n::select(&LOADER, choice);
    let shown = readable(&LOADER);
    native_term_config::i18n::set_language(if shown { choice } else { Some(native_term_i18n::FALLBACK) });
}

/// Chinese or Japanese with no font on the system to show it: English
/// instead (egui's own fonts have no CJK glyphs, so it would be boxes).
/// Whether the language asked for is the one shown.
fn readable(loader: &FluentLanguageLoader) -> bool {
    let cjk = ["zh", "ja", "ko"].contains(&loader.current_languages().first().map_or("", |l| l.language.as_str()));
    if cjk && !native_term_os::fonts::has_cjk() {
        native_term_i18n::select(loader, Some(native_term_i18n::FALLBACK));
        return false;
    }
    true
}

pub fn current() -> String {
    native_term_i18n::current(&LOADER)
}

pub use native_term_i18n::available;
