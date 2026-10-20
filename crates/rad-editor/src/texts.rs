//! Embedded presentation catalogs. English is the fallback for every locale.
use std::{collections::BTreeMap, sync::OnceLock};
include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));
// Compatibility aliases for existing catalog consumers, not the language registry.
pub const EN: &str = include_str!("locales/en.json");
pub const ES: &str = include_str!("locales/es.json");
fn catalogs() -> &'static BTreeMap<&'static str, BTreeMap<String, String>> {
    static CATALOGS: OnceLock<BTreeMap<&'static str, BTreeMap<String, String>>> = OnceLock::new();
    CATALOGS.get_or_init(|| {
        EMBEDDED
            .iter()
            .map(|(locale, json)| {
                (
                    *locale,
                    serde_json::from_str(json).expect("valid embedded catalog"),
                )
            })
            .collect()
    })
}
pub fn locale(locale: &str) -> &'static str {
    let normalized = locale
        .split(['.', '@'])
        .next()
        .unwrap_or("")
        .replace('_', "-")
        .to_ascii_lowercase();
    catalogs()
        .get_key_value(normalized.as_str())
        .or_else(|| catalogs().get_key_value(normalized.split('-').next().unwrap_or("")))
        .map(|(key, _)| *key)
        .unwrap_or("en")
}
pub fn text(locale: &str, key: &str) -> &'static str {
    let catalogs = catalogs();
    catalogs
        .get(self::locale(locale))
        .and_then(|catalog| catalog.get(key))
        .or_else(|| catalogs["en"].get(key))
        .map(String::as_str)
        .unwrap_or("Something went wrong")
}
pub fn tr(language: impl Into<crate::preferences::Language>, key: &str) -> &'static str {
    text(language.into().code(), key)
}
