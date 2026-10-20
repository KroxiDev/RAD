use rad_editor::preferences::Language;
#[test]
fn system_locale_selects_supported_language_with_english_fallback() {
    assert_eq!(Language::from_locale("es_AR.UTF-8"), Language::Es);
    assert_eq!(Language::from_locale("es-ES"), Language::Es);
    assert_eq!(Language::from_locale("en_US.UTF-8"), Language::En);
    assert_eq!(Language::from_locale("fr_FR.UTF-8"), Language::En);
    assert_eq!(Language::from_locale(""), Language::En);
}
#[test]
fn explicit_language_survives_reopening_without_becoming_deck_data() {
    use rad_editor::preferences::Preferences;
    let root = std::env::temp_dir().join(format!("rad-preferences-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("editor.json");
    assert_eq!(Preferences::load(&path).unwrap(), Preferences::default());
    let selected = Preferences {
        language: Some(Language::Es),
    };
    selected.save(&path).unwrap();
    let loaded = Preferences::load(&path).unwrap();
    assert_eq!(loaded.language("en_US"), Language::Es);
    assert_eq!(loaded, selected);
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(document, serde_json::json!({"version":1,"language":"es"}));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn preferences_save_can_retry_after_publication_failure() {
    use rad_editor::preferences::Preferences;
    let root = std::env::temp_dir().join(format!("rad-preferences-retry-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("editor.json");
    std::fs::create_dir(&path).unwrap();
    let selected = Preferences {
        language: Some(Language::Es),
    };
    assert!(selected.save(&path).is_err());
    assert_eq!(
        std::fs::read_dir(&root).unwrap().count(),
        1,
        "failed save must remove its staging file"
    );
    std::fs::remove_dir(&path).unwrap();
    let retried = selected.save(&path);
    let loaded = Preferences::load(&path);
    std::fs::remove_dir_all(&root).unwrap();
    retried.unwrap();
    assert_eq!(loaded.unwrap(), selected);
}
