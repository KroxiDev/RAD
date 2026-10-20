use rad_codec::*;
use rad_core::{configuration::*, identity::IdentityRegistry};
fn initial() -> Candidate {
    Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap()
}
#[test]
fn complete_settings_roundtrip_with_zero_sleep_max_durations_and_language() {
    let c = initial();
    for settings in [
        SettingsData {
            brightness_percent: 0,
            sleep_ms: 0,
            press_threshold_ms: u64::MAX,
            gui_language: Language::Es,
        },
        SettingsData {
            brightness_percent: 100,
            sleep_ms: u64::MAX,
            press_threshold_ms: 0,
            gui_language: Language::En,
        },
    ] {
        let c = c
            .configuration()
            .prepare_edit(c.registry(), Origin::Editor, Edit::SetSettings(settings))
            .unwrap();
        let bytes = encode(c.configuration()).unwrap();
        let data = decode_data(&bytes).unwrap();
        assert_eq!(&data, c.configuration().data());
        assert_eq!(
            Configuration::from_data(data, c.registry()).unwrap(),
            *c.configuration()
        );
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["version"], 1);
        assert!(value["settings"]["sleep_ms"].is_string());
        assert!(value["settings"]["press_threshold_ms"].is_string());
        assert_eq!(bytes, encode(c.configuration()).unwrap());
    }
}
#[test]
fn single_v1_requires_settings_and_never_fills_missing_or_null_fields() {
    let c = initial();
    let original: serde_json::Value =
        serde_json::from_slice(&encode(c.configuration()).unwrap()).unwrap();
    let mut old = original.clone();
    old.as_object_mut().unwrap().remove("settings");
    assert_eq!(
        decode_data(&serde_json::to_vec(&old).unwrap()),
        Err(CodecError::InvalidDocument)
    );
    let mut null = original.clone();
    null["settings"] = serde_json::Value::Null;
    assert_eq!(
        decode_data(&serde_json::to_vec(&null).unwrap()),
        Err(CodecError::InvalidDocument)
    );
    for field in [
        "brightness_percent",
        "sleep_ms",
        "press_threshold_ms",
        "gui_language",
    ] {
        let mut absent = original.clone();
        absent["settings"].as_object_mut().unwrap().remove(field);
        assert_eq!(
            decode_data(&serde_json::to_vec(&absent).unwrap()),
            Err(CodecError::InvalidDocument),
            "{field}"
        );
    }
}
#[test]
fn codec_rejects_out_of_range_brightness_instead_of_returning_contradictory_settings() {
    let c = initial();
    let original: serde_json::Value =
        serde_json::from_slice(&encode(c.configuration()).unwrap()).unwrap();
    for brightness in [101, 255] {
        let mut bad = original.clone();
        bad["settings"]["brightness_percent"] = serde_json::json!(brightness);
        assert_eq!(
            decode_data(&serde_json::to_vec(&bad).unwrap()),
            Err(CodecError::InvalidValue)
        );
    }
}
#[test]
fn language_is_only_a_closed_json_string_not_a_serde_unit_map() {
    let c = initial();
    let original: serde_json::Value =
        serde_json::from_slice(&encode(c.configuration()).unwrap()).unwrap();
    for language in [
        serde_json::json!({"en":null}),
        serde_json::json!({"es":null}),
        serde_json::json!("fr"),
        serde_json::json!("EN"),
        serde_json::json!(1),
        serde_json::Value::Null,
    ] {
        let mut bad = original.clone();
        bad["settings"]["gui_language"] = language;
        assert!(decode_data(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
}
#[test]
fn strict_settings_reject_duplicates_unknown_fields_nulls_and_noncanonical_durations() {
    let c = initial();
    let bytes = encode(c.configuration()).unwrap();
    let json = String::from_utf8(bytes.clone()).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for field in [
        "brightness_percent",
        "sleep_ms",
        "press_threshold_ms",
        "gui_language",
    ] {
        let encoded = serde_json::to_string(&original["settings"][field]).unwrap();
        let pair = format!("\"{field}\":{encoded}");
        let duplicated = json.replacen(&pair, &format!("{pair},{pair}"), 1);
        assert_ne!(duplicated, json);
        assert_eq!(
            decode_data(duplicated.as_bytes()),
            Err(CodecError::InvalidDocument)
        );
        let mut null = original.clone();
        null["settings"][field] = serde_json::Value::Null;
        assert_eq!(
            decode_data(&serde_json::to_vec(&null).unwrap()),
            Err(CodecError::InvalidDocument)
        );
    }
    let mut unknown = original.clone();
    unknown["settings"]["client_role"] = serde_json::json!("editor");
    assert_eq!(
        decode_data(&serde_json::to_vec(&unknown).unwrap()),
        Err(CodecError::InvalidDocument)
    );
    for field in ["sleep_ms", "press_threshold_ms"] {
        for value in [
            "01",
            "+1",
            "-1",
            " 1",
            "1.0",
            "1e0",
            "",
            "18446744073709551616",
            "١",
        ] {
            let mut bad = original.clone();
            bad["settings"][field] = serde_json::json!(value);
            assert_eq!(
                decode_data(&serde_json::to_vec(&bad).unwrap()),
                Err(CodecError::InvalidValue)
            );
        }
        let mut numeric = original.clone();
        numeric["settings"][field] = serde_json::json!(0);
        assert_eq!(
            decode_data(&serde_json::to_vec(&numeric).unwrap()),
            Err(CodecError::InvalidDocument)
        );
    }
    for value in [
        serde_json::json!(-1),
        serde_json::json!(256),
        serde_json::json!(100.0),
        serde_json::json!("100"),
    ] {
        let mut bad = original.clone();
        bad["settings"]["brightness_percent"] = value;
        assert!(decode_data(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut future = original;
    future["version"] = serde_json::json!(2);
    assert_eq!(
        decode_data(&serde_json::to_vec(&future).unwrap()),
        Err(CodecError::UnsupportedVersion)
    );
    assert_eq!(encode(c.configuration()).unwrap(), bytes);
}
#[test]
fn settings_and_tree_share_one_document_and_restore_candidate() {
    let c = initial();
    let expected = SettingsData {
        brightness_percent: 33,
        sleep_ms: 0,
        press_threshold_ms: 999,
        gui_language: Language::Es,
    };
    let c = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                Edit::CreateFolder {
                    parent: c.configuration().data().principal,
                    position: 0,
                    name: "child".into(),
                },
                Edit::SetSettings(expected),
            ],
        )
        .unwrap();
    let bytes = encode(c.configuration()).unwrap();
    let data = decode_data(&bytes).unwrap();
    assert_eq!(data, c.configuration().data().clone());
    assert_eq!(data.settings, expected);
    assert_eq!(data.folders.len(), 2);
    let changed = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData::default()),
        )
        .unwrap();
    let restored = changed
        .configuration()
        .prepare_restore(changed.registry(), Origin::Editor, true, data)
        .unwrap();
    assert_eq!(restored.configuration(), c.configuration());
    assert_eq!(restored.registry(), changed.registry());
}
