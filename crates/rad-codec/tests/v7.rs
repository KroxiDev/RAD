use rad_codec::{decode_data, encode};
use rad_core::{configuration::*, identity::*};
#[test]
fn v7_styles_round_trip_and_v6_stays_closed() {
    let mut value: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../docs/fixtures/v6/profile.json")).unwrap();
    value["version"] = 7.into();
    for face in ["appearance", "state"] {
        let appearance = &mut value["buttons"][0]["appearance"];
        let target = if face == "state" {
            &mut appearance["states"]["muted"]
        } else {
            appearance
        };
        target["text_position"] = "hidden".into();
        target["text_size"] = 24.into();
        target["text_color"] = "#1234AB".into();
        target["text_band"] = false.into();
    }
    let data = decode_data(&serde_json::to_vec(&value).unwrap()).expect("v7 style document");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let candidate = initial
        .configuration()
        .prepare_restore(initial.registry(), Origin::Editor, true, data)
        .unwrap();
    let encoded: serde_json::Value =
        serde_json::from_slice(&encode(candidate.configuration()).unwrap()).unwrap();
    assert_eq!(encoded["version"], 7);
    for key in ["text_position", "text_size", "text_color", "text_band"] {
        assert_eq!(
            encoded["buttons"][0]["appearance"][key],
            value["buttons"][0]["appearance"][key]
        );
        assert_eq!(
            encoded["buttons"][0]["appearance"]["states"]["muted"][key],
            value["buttons"][0]["appearance"]["states"]["muted"][key]
        );
    }
    value["version"] = 6.into();
    assert!(decode_data(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn v6_migrates_each_existing_face_position_and_keeps_automatic() {
    for explicit in [true, false] {
        let mut value: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../docs/fixtures/v6/profile.json"))
                .unwrap();
        if !explicit {
            value["buttons"][0]["appearance"]
                .as_object_mut()
                .unwrap()
                .remove("text_position");
        }
        let data = decode_data(&serde_json::to_vec(&value).unwrap()).unwrap();
        let appearance = &data.buttons[0].appearance;
        let expected = explicit.then_some(TextPosition::AboveImage);
        assert_eq!(appearance.text_style.position, expected);
        for face in appearance.states.values() {
            assert_eq!(
                face.text_style,
                TextStyle {
                    position: expected,
                    ..Default::default()
                }
            );
        }
    }
}
#[test]
fn every_new_v7_field_is_rejected_in_closed_v6_including_null() {
    let original: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../docs/fixtures/v6/profile.json")).unwrap();
    for state in [false, true] {
        for (field, value) in [
            ("text_size", serde_json::json!(13)),
            ("text_color", serde_json::json!("#FFFFFF")),
            ("text_band", serde_json::json!(true)),
            ("text_position", serde_json::json!("hidden")),
            ("text_size", serde_json::Value::Null),
        ] {
            let mut v = original.clone();
            let face = if state {
                &mut v["buttons"][0]["appearance"]["states"]["muted"]
            } else {
                &mut v["buttons"][0]["appearance"]
            };
            face[field] = value;
            assert!(
                decode_data(&serde_json::to_vec(&v).unwrap()).is_err(),
                "{state} {field}"
            );
        }
    }
}
#[test]
fn style_ranges_and_rgb_syntax_are_validated_in_each_face() {
    let mut original: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../docs/fixtures/v6/profile.json")).unwrap();
    original["version"] = 7.into();
    for state in [false, true] {
        for (field, value) in [
            ("text_size", serde_json::json!(7)),
            ("text_size", serde_json::json!(25)),
            ("text_size", serde_json::json!(13.5)),
            ("text_color", serde_json::json!("#FFFFFG")),
            ("text_color", serde_json::json!("FFFFFF")),
            ("text_color", serde_json::json!("#12345678")),
        ] {
            let mut v = original.clone();
            let face = if state {
                &mut v["buttons"][0]["appearance"]["states"]["muted"]
            } else {
                &mut v["buttons"][0]["appearance"]
            };
            face[field] = value;
            assert!(
                decode_data(&serde_json::to_vec(&v).unwrap()).is_err(),
                "{state} {field}"
            );
        }
    }
}

#[test]
fn canonical_writer_omits_default_styles_for_base_and_states() {
    let mut value: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../docs/fixtures/v6/profile.json")).unwrap();
    value["version"] = 7.into();
    value["buttons"][0]["appearance"]
        .as_object_mut()
        .unwrap()
        .remove("text_position");
    for state in [false, true] {
        let face = if state {
            &mut value["buttons"][0]["appearance"]["states"]["muted"]
        } else {
            &mut value["buttons"][0]["appearance"]
        };
        face["text_size"] = 13.into();
        face["text_color"] = "#ffffff".into();
        face["text_band"] = true.into();
    }
    let data = decode_data(&serde_json::to_vec(&value).unwrap()).unwrap();
    let base = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let candidate = base
        .configuration()
        .prepare_restore(base.registry(), Origin::Editor, true, data)
        .unwrap();
    let encoded: serde_json::Value =
        serde_json::from_slice(&encode(candidate.configuration()).unwrap()).unwrap();
    for face in [
        &encoded["buttons"][0]["appearance"],
        &encoded["buttons"][0]["appearance"]["states"]["muted"],
    ] {
        for field in ["text_position", "text_size", "text_color", "text_band"] {
            assert!(face.get(field).is_none(), "{field}");
        }
    }
}
