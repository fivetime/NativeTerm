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
        "zh-TW" => "繁體中文",
        "ja" => "日本語",
        _ => "?",
    };
    ["en", "zh-CN", "zh-TW", "ja"].iter().filter_map(|id| id.parse().ok().map(|l| (l, name(id)))).collect()
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

    /// The whole message file of a language.
    fn text(language: &str, domain: &str) -> String {
        let file = Localizations::get(&format!("{language}/{domain}.ftl")).expect("message file");
        String::from_utf8(file.data.into_owned()).expect("UTF-8")
    }

    /// Every `{ $name }` a message file uses, per message id: a
    /// translation that invents an argument (or misspells one) shows an
    /// error instead of the text, and only at run time.
    fn arguments(
        language: &str,
        domain: &str,
    ) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
        let mut found: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> = Default::default();
        let mut id = String::new();
        for line in text(language, domain).lines() {
            if line.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                if let Some((name, _)) = line.split_once(" =") {
                    id = name.trim().to_string();
                }
            }
            if id.is_empty() {
                continue;
            }
            let mut rest = line;
            while let Some(at) = rest.find("{ $") {
                rest = &rest[at + 3..];
                let end = rest.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(rest.len());
                found.entry(id.clone()).or_default().insert(rest[..end].to_string());
                rest = &rest[end..];
            }
        }
        found
    }

    fn ids(language: &str, domain: &str) -> std::collections::BTreeSet<String> {
        let file = Localizations::get(&format!("{language}/{domain}.ftl")).expect("message file");
        let text = String::from_utf8(file.data.into_owned()).unwrap();
        text.lines()
            .filter(|l| l.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
            .filter_map(|l| l.split_once(" =").map(|(id, _)| id.trim().to_string()))
            .collect()
    }

    /// A translation may leave an argument out (Japanese often needs no
    /// counter), but one it invents would be shown as an error at run
    /// time, in that language only.
    #[test]
    fn no_translation_invents_an_argument() {
        for domain in ["native_term_app", "native_term_shim", "native_term_config"] {
            let english = arguments(FALLBACK, domain);
            for (language, _) in available() {
                let language = language.to_string();
                for (id, used) in arguments(&language, domain) {
                    let known = english.get(&id).cloned().unwrap_or_default();
                    let unknown: Vec<&String> = used.difference(&known).collect();
                    assert!(unknown.is_empty(), "{domain} {language}: {id} uses {unknown:?}, English has {known:?}");
                }
            }
        }
    }

    /// Fluent reports a broken file by quietly having no messages, so
    /// every language is loaded and asked for something.
    #[test]
    fn every_language_loads_and_formats() {
        for (language, name) in available() {
            let language = language.to_string();
            let loader = loader("native_term_app", Some(&language));
            assert_eq!(current(&loader), language, "{name}");
            let args = std::collections::HashMap::from([("folder", "Lab")]);
            let shown = loader.get_args("host-new-title", args);
            assert!(shown.contains("Lab"), "{language}: {shown}");
            assert!(!shown.contains("host-new-title"), "{language}: the message is missing ({shown})");
            let plain = loader.get("button-save");
            assert!(!plain.is_empty() && plain != "button-save", "{language}: {plain}");
        }
    }

    #[test]
    fn every_language_has_every_message() {
        for domain in ["native_term_app", "native_term_shim", "native_term_config"] {
            let english = ids(FALLBACK, domain);
            assert!(english.len() > 5, "{domain}");
            for (language, _) in available() {
                let other = ids(&language.to_string(), domain);
                assert_eq!(english, other, "{domain}: {language}");
            }
        }
    }
}
