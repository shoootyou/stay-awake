//! Internationalization support via fluent-rs.
//!
//! Embeds `.ftl` locale files at compile time and provides a simple
//! `create_bundle` / `t` API for resolving translated strings.

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::FluentResource;
use unic_langid::LanguageIdentifier;

pub type Bundle = FluentBundle<FluentResource>;

const LOCALE_EN: &str = include_str!("../locales/en.ftl");
const LOCALE_ES: &str = include_str!("../locales/es.ftl");
const LOCALE_FR: &str = include_str!("../locales/fr.ftl");
const LOCALE_DE: &str = include_str!("../locales/de.ftl");
const LOCALE_PT_BR: &str = include_str!("../locales/pt-BR.ftl");
const LOCALE_JA: &str = include_str!("../locales/ja.ftl");
const LOCALE_ZH_HANS: &str = include_str!("../locales/zh-Hans.ftl");
const LOCALE_KO: &str = include_str!("../locales/ko.ftl");

/// Return the FTL source for a given locale code.
fn ftl_for_locale(locale: &str) -> Option<&'static str> {
    match locale {
        "en" => Some(LOCALE_EN),
        "es" => Some(LOCALE_ES),
        "fr" => Some(LOCALE_FR),
        "de" => Some(LOCALE_DE),
        "pt-BR" => Some(LOCALE_PT_BR),
        "ja" => Some(LOCALE_JA),
        "zh-Hans" => Some(LOCALE_ZH_HANS),
        "ko" => Some(LOCALE_KO),
        _ => None,
    }
}

/// Create a [`FluentBundle`] for the requested locale with English fallback.
pub fn create_bundle(locale: &str) -> Bundle {
    let langid: LanguageIdentifier = locale
        .parse()
        .unwrap_or_else(|_| "en".parse().expect("en is valid"));

    let mut bundle = FluentBundle::new_concurrent(vec![langid]);

    // Load the requested locale first (if it exists and is not English).
    if locale != "en" {
        if let Some(ftl) = ftl_for_locale(locale) {
            if let Ok(resource) = FluentResource::try_new(ftl.to_string()) {
                let _ = bundle.add_resource(resource);
            }
        }
    }

    // Always add English as fallback so every key resolves.
    if let Ok(resource) = FluentResource::try_new(LOCALE_EN.to_string()) {
        let _ = bundle.add_resource(resource);
    }

    bundle
}

/// Resolve a single message ID from the bundle, returning the ID itself on failure.
pub fn t(bundle: &Bundle, id: &str) -> String {
    bundle
        .get_message(id)
        .and_then(|msg| msg.value())
        .map(|pattern| {
            let mut errors = vec![];
            bundle
                .format_pattern(pattern, None, &mut errors)
                .to_string()
        })
        .unwrap_or_else(|| id.to_string())
}

#[cfg(test)]
mod tests {
    /*
     * @spec-handoff
     * @interface create_bundle(locale: &str) -> Bundle; t(bundle, id) -> String
     * @behavior
     *   - Every English message key resolves for every shipped locale.
     *   - An unsupported locale and missing message return the current English/key fallbacks.
     * @edge-cases
     *   - Fluent text is returned without format arguments.
     */
    use super::*;

    const LOCALES: [&str; 8] = ["en", "es", "fr", "de", "pt-BR", "ja", "zh-Hans", "ko"];

    fn english_message_ids() -> Vec<String> {
        LOCALE_EN
            .lines()
            .filter_map(|line| line.split_once(" =").map(|(id, _)| id.to_string()))
            .collect()
    }

    #[test]
    fn every_english_key_resolves_for_every_shipped_locale() {
        let ids = english_message_ids();
        assert!(!ids.is_empty());
        for locale in LOCALES {
            let bundle = create_bundle(locale);
            for id in &ids {
                assert_ne!(t(&bundle, id), *id, "{locale} must resolve {id}");
            }
        }
    }

    #[test]
    fn unsupported_locale_uses_english_and_missing_key_returns_its_id() {
        let bundle = create_bundle("unknown-locale");
        assert_eq!(t(&bundle, "tray-start"), "Start");
        assert_eq!(t(&bundle, "missing-message"), "missing-message");
    }
}
