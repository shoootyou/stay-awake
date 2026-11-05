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
    bundle.set_use_isolating(false);

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

// wired in E3b-2
#[allow(dead_code)]
pub(crate) fn t_with_args(
    bundle: &Bundle,
    id: &str,
    args: Option<&fluent_bundle::FluentArgs<'_>>,
) -> String {
    let Some(pattern) = bundle.get_message(id).and_then(|message| message.value()) else {
        return id.to_owned();
    };
    let mut errors = Vec::new();
    let formatted = bundle.format_pattern(pattern, args, &mut errors);
    if errors.is_empty() {
        formatted.into_owned()
    } else {
        id.to_owned()
    }
}

// wired in E3b-2
#[allow(dead_code)]
pub(crate) struct NextWindowInput<'a> {
    pub(crate) when: chrono::DateTime<chrono::FixedOffset>,
    pub(crate) locale: &'a str,
}

// wired in E3b-2
#[allow(dead_code)]
pub(crate) fn format_next_window(input: NextWindowInput<'_>) -> String {
    let date = match input.locale {
        "es" | "fr" | "pt-BR" => input.when.format("%d/%m/%Y"),
        "de" => input.when.format("%d.%m.%Y"),
        "ja" | "zh-Hans" | "ko" => input.when.format("%Y/%m/%d"),
        _ => input.when.format("%m/%d/%Y"),
    };
    format!("{date} {}", input.when.format("%H:%M %:z"))
}

#[cfg(test)]
mod tests {
    /*
     * @spec-handoff
     * @interface t_with_args(Bundle, id, FluentArgs); format_next_window(NextWindowInput) -> String
     * @behavior
     *   - Eight valid locale bundles define every schedule status and accessibility message.
     *   - Fluent arguments render into tray and network-removal templates without placeholders.
     *   - Next-window timestamps use the documented locale-specific numeric format.
     * @edge-cases
     *   - No upcoming window uses its localized message.
     */
    use super::*;
    use chrono::TimeZone;

    const LOCALES: [&str; 8] = ["en", "es", "fr", "de", "pt-BR", "ja", "zh-Hans", "ko"];
    const NEW_KEYS: [&str; 11] = [
        "tray-waiting",
        "tray-paused",
        "tray-active-override",
        "tray-tooltip-waiting",
        "tray-tooltip-paused",
        "tray-tooltip-override",
        "tray-next-window",
        "tray-no-next-window",
        "schedule-enabled-hint",
        "settings-mode-info-label",
        "settings-wifi-remove-label",
    ];

    #[test]
    fn eight_parseable_locale_files_define_every_schedule_status_key() {
        let locale_sources = [
            ("en", LOCALE_EN),
            ("es", LOCALE_ES),
            ("fr", LOCALE_FR),
            ("de", LOCALE_DE),
            ("pt-BR", LOCALE_PT_BR),
            ("ja", LOCALE_JA),
            ("zh-Hans", LOCALE_ZH_HANS),
            ("ko", LOCALE_KO),
        ];
        assert_eq!(locale_sources.len(), 8);
        for (locale, source) in locale_sources {
            let resource = FluentResource::try_new(source.to_owned())
                .unwrap_or_else(|(_, errors)| panic!("{locale} must parse: {errors:?}"));
            let language = locale.parse().expect("locale fixture is valid");
            let mut bundle = FluentBundle::new_concurrent(vec![language]);
            assert!(
                bundle.add_resource(resource).is_ok(),
                "{locale} must load into a Fluent bundle"
            );
            for key in NEW_KEYS {
                assert!(
                    source
                        .lines()
                        .any(|line| line.starts_with(&format!("{key} ="))),
                    "{locale} must define {key}"
                );
            }
        }
    }

    #[test]
    fn english_schedule_status_messages_match_the_contract() {
        let bundle = create_bundle("en");
        let mut args = fluent_bundle::FluentArgs::new();
        args.set("when", "Mon 09 Nov 2026, 09:00");
        args.set("network", "Lab");
        for (key, expected) in [
            ("tray-waiting", "Waiting for schedule"),
            ("tray-paused", "Paused for this window"),
            ("tray-active-override", "Active (manual override)"),
            ("tray-tooltip-waiting", "Stay Awake: waiting for schedule"),
            ("tray-tooltip-paused", "Stay Awake: paused for this window"),
            (
                "tray-tooltip-override",
                "Stay Awake: active by manual override",
            ),
            ("tray-next-window", "Next window: Mon 09 Nov 2026, 09:00"),
            ("tray-no-next-window", "No upcoming window"),
            (
                "schedule-enabled-hint",
                "Automatically starts and stops in Manual and Always On. In WiFi mode, only mouse \
                 jiggles follow the schedule.",
            ),
            ("settings-mode-info-label", "About jiggle modes"),
            ("settings-wifi-remove-label", "Remove network Lab"),
        ] {
            assert_eq!(t_with_args(&bundle, key, Some(&args)), expected, "{key}");
        }
    }

    #[test]
    fn fluent_arguments_render_without_unresolved_placeholders_in_every_locale() {
        let mut args = fluent_bundle::FluentArgs::new();
        args.set("when", "Tidepool-42");
        args.set("network", "Lantern-7");
        for locale in LOCALES {
            for (key, value) in [
                ("tray-next-window", "Tidepool-42"),
                ("settings-wifi-remove-label", "Lantern-7"),
            ] {
                let rendered = t_with_args(&create_bundle(locale), key, Some(&args));
                assert!(rendered.contains(value), "{locale} must render {key}");
                assert!(!rendered.contains("{ $"), "{locale} must resolve {key}");
            }
        }
    }

    #[test]
    fn formats_next_window_and_no_upcoming_window_in_english() {
        let when = chrono::FixedOffset::east_opt(0)
            .expect("UTC offset exists")
            .with_ymd_and_hms(2026, 11, 9, 9, 0, 0)
            .single()
            .expect("fixed timestamp exists");
        assert_eq!(
            format_next_window(NextWindowInput { when, locale: "en" }),
            "11/09/2026 09:00 +00:00"
        );
        assert_eq!(
            t_with_args(&create_bundle("en"), "tray-no-next-window", None),
            "No upcoming window"
        );
    }

    #[test]
    fn formats_next_window_with_each_locale_numeric_convention() {
        let when = chrono::FixedOffset::east_opt(0)
            .expect("UTC offset exists")
            .with_ymd_and_hms(2026, 11, 9, 9, 0, 0)
            .single()
            .expect("fixed timestamp exists");

        for (locale, expected) in [
            ("en", "11/09/2026 09:00 +00:00"),
            ("es", "09/11/2026 09:00 +00:00"),
            ("fr", "09/11/2026 09:00 +00:00"),
            ("de", "09.11.2026 09:00 +00:00"),
            ("pt-BR", "09/11/2026 09:00 +00:00"),
            ("ja", "2026/11/09 09:00 +00:00"),
            ("zh-Hans", "2026/11/09 09:00 +00:00"),
            ("ko", "2026/11/09 09:00 +00:00"),
        ] {
            assert_eq!(
                format_next_window(NextWindowInput { when, locale }),
                expected,
                "{locale} uses its contract numeric date order and separator"
            );
        }
    }

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
