//! Translations shared by NativeTerm and the shim (Project Fluent, see
//! `docs/ARCHITECTURE.md`, "Localization").
//!
//! Each crate that shows text has its own message file,
//! `i18n/<language>/<crate_name>.ftl`, and an `i18n.toml` pointing here, so
//! `i18n_embed_fl::fl!` checks its keys at compile time. The files are
//! embedded in the binaries.
//!
//! ```ignore
//! static LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| native_term_i18n::loader("native_term_app"));
//! fl!(LOADER, "sessions-title", count = 3)
//! ```

use i18n_embed::fluent::FluentLanguageLoader;
use i18n_embed::{DesktopLanguageRequester, LanguageLoader};
use rust_embed::RustEmbed;
pub use unic_langid::LanguageIdentifier;

#[derive(RustEmbed)]
#[folder = "i18n"]
struct Localizations;

pub const FALLBACK: &str = "en";

/// The languages NativeTerm has, with their own names.
pub fn available() -> Vec<(LanguageIdentifier, &'static str)> {
    let name = |id: &str| match id {
        "en" => "English",
        "zh-CN" => "简体中文",
        _ => "?",
    };
    ["en", "zh-CN"].iter().filter_map(|id| id.parse().ok().map(|l| (l, name(id)))).collect()
}

/// A loader for `domain` (the crate name with underscores) in the
/// language `choice` (`None`: the system's).
pub fn loader(domain: &'static str, choice: Option<&str>) -> FluentLanguageLoader {
    let loader = FluentLanguageLoader::new(domain, FALLBACK.parse().expect("valid language id"));
    select(&loader, choice);
    loader
}

/// Switch `loader` to `choice` (`None`: the system's languages). Missing
/// messages fall back to English.
pub fn select(loader: &FluentLanguageLoader, choice: Option<&str>) {
    let requested: Vec<LanguageIdentifier> = match choice.and_then(|c| c.parse().ok()) {
        Some(lang) => vec![lang],
        None => DesktopLanguageRequester::requested_languages(),
    };
    let _ = i18n_embed::select(loader, &Localizations, &requested);
    if loader.current_languages().is_empty() {
        let _ = loader.load_fallback_language(&Localizations);
    }
    // plain text: no Unicode isolation marks around arguments (applies to
    // the bundles just loaded, so after every switch)
    loader.set_use_isolating(false);
}

/// The language `loader` currently shows (its first one).
pub fn current(loader: &FluentLanguageLoader) -> String {
    loader.current_languages().first().map(|l| l.to_string()).unwrap_or_else(|| FALLBACK.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_keeps_plain_arguments() {
        let loader = loader("native_term_app", Some("en"));
        let args = std::collections::HashMap::from([("folder", "Lab")]);
        assert_eq!(loader.get_args("host-new-title", args.clone()), "New host in Lab");
        select(&loader, Some("zh-CN"));
        assert_eq!(current(&loader), "zh-CN");
        assert_eq!(loader.get_args("host-new-title", args.clone()), "在 Lab 中新建主机");
        select(&loader, Some("en"));
        assert_eq!(loader.get_args("host-new-title", args), "New host in Lab");
    }

    fn ids(language: &str, domain: &str) -> std::collections::BTreeSet<String> {
        let file = Localizations::get(&format!("{language}/{domain}.ftl")).expect("message file");
        let text = String::from_utf8(file.data.into_owned()).unwrap();
        text.lines()
            .filter(|l| l.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
            .filter_map(|l| l.split_once(" =").map(|(id, _)| id.trim().to_string()))
            .collect()
    }

    #[test]
    fn every_language_has_every_message() {
        for domain in ["native_term_app", "native_term_shim"] {
            let english = ids(FALLBACK, domain);
            assert!(english.len() > 5, "{domain}");
            for (language, _) in available() {
                let other = ids(&language.to_string(), domain);
                assert_eq!(english, other, "{domain}: {language}");
            }
        }
    }
}
