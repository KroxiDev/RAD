use rad_editor::texts;
use std::collections::BTreeMap;
#[test]
fn every_embedded_catalog_has_the_same_nonempty_keys_and_english_fallback() {
    let en: BTreeMap<String, String> = serde_json::from_str(texts::EN).unwrap();
    assert!(en.len() > 150);
    for (locale, source) in texts::EMBEDDED {
        let catalog: BTreeMap<String, String> = serde_json::from_str(source).unwrap();
        assert_eq!(
            en.keys().collect::<Vec<_>>(),
            catalog.keys().collect::<Vec<_>>(),
            "{locale}"
        );
        assert!(catalog.values().all(|value| !value.is_empty()));
        assert!(
            rad_editor::preferences::Language::available()
                .any(|language| language.code() == *locale)
        );
        for (key, value) in &catalog {
            assert_eq!(texts::text(locale, key), value);
        }
    }
    for (key, value) in &en {
        assert_eq!(texts::text("unavailable", key), value);
    }
    assert_eq!(texts::text("es", "future.unknown"), "Something went wrong");
}
#[test]
fn unknown_errors_have_plain_copy_while_technical_code_is_retained() {
    let error = rad_editor::session::Error {
        code: "future_code".into(),
        cause: None,
        source_change: None,
        unknown: false,
    };
    assert_eq!(
        rad_editor::status::describe_failure(&error, true),
        "Algo salió mal"
    );
    assert_eq!(error.code, "future_code");
}
#[test]
fn plugin_internal_states_map_to_four_plain_states_with_actions() {
    use rad_editor::status::PluginState as S;
    use serde_json::json;
    for (item, state) in [
        (json!({"host":"ready"}), S::Ready),
        (json!({"host":"absent"}), S::Ready),
        (json!({"needs_credentials":true}), S::NeedsPassword),
        (json!({"diagnostic":"incompatible_plugin"}), S::NeedsUpdate),
        (json!({"host":"residual"}), S::NotWorking),
        (json!({"host":"unknown_future"}), S::NotWorking),
    ] {
        assert_eq!(S::of(&item), state);
        assert_eq!(state.action(false).is_some(), state != S::Ready);
    }
}

#[test]
fn every_literal_lookup_in_editor_source_has_a_catalog_entry() {
    let catalog: BTreeMap<String, String> = serde_json::from_str(texts::EN).unwrap();
    fn files(path: &std::path::Path, result: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files(&path, result);
            } else if path.extension().is_some_and(|e| e == "rs") {
                result.push(path);
            }
        }
    }
    let mut paths = vec![];
    files(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut paths,
    );
    for path in paths {
        let source = std::fs::read_to_string(&path).unwrap();
        for (position, _) in source.match_indices("tr(") {
            if source[..position]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                continue;
            }
            let tail = &source[position + 3..];
            let Some((_, key)) = tail.split_once(',') else {
                continue;
            };
            let key = key.trim_start();
            if !key.starts_with('"') {
                continue;
            }
            let mut escaped = false;
            for (i, c) in key.char_indices().skip(1) {
                if c == '"' && !escaped {
                    let name: String = serde_json::from_str(&key[..=i]).unwrap();
                    assert!(
                        catalog.contains_key(&name),
                        "{}: missing {name}",
                        path.display()
                    );
                    break;
                }
                escaped = c == '\\' && !escaped;
            }
        }
    }
}

#[test]
fn catalog_selection_uses_system_locale_normalization() {
    assert_eq!(texts::text("ES_AR.UTF-8", "Settings"), "Ajustes");
}
