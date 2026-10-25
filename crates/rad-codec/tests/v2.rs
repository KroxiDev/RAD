use rad_codec::{decode_data, encode};
use rad_core::{configuration::Configuration, identity::IdentityRegistry};
#[test]
fn canonical_document_keeps_only_deck_settings() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let bytes = encode(c.configuration()).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["version"], 5);
    assert_eq!(doc["grid"], serde_json::json!([8, 4]));
    assert!(doc.get("plugin_settings").is_none());
    assert!(doc["settings"].get("gui_language").is_none());
    assert_eq!(decode_data(&bytes).unwrap(), *c.configuration().data());
}
#[test]
fn original_v1_fixture_imports_and_reencodes_without_legacy_fields() {
    let data = decode_data(include_bytes!("../../../docs/fixtures/v1/profile.json")).unwrap();
    let records =
        data.folders
            .iter()
            .map(|f| f.binding)
            .chain(data.buttons.iter().flat_map(|b| {
                std::iter::once(b.binding).chain(b.actions.iter().map(|a| a.binding))
            }))
            .map(|b| rad_core::identity::IdentityRecord::new(b, false));
    let registry = IdentityRegistry::from_records(77, records).unwrap();
    let c = Configuration::from_data(data, &registry).unwrap();
    let bytes = encode(&c).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["version"], 5);
    assert!(doc["settings"].get("gui_language").is_none());
    assert_eq!(doc["buttons"][0]["name"], "Fixture launch");
}
#[test]
fn v2_rejects_legacy_fields_and_future_version_is_explicit() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let mut v: serde_json::Value =
        serde_json::from_slice(&encode(c.configuration()).unwrap()).unwrap();
    v["version"] = serde_json::json!(999);
    assert_eq!(
        decode_data(&serde_json::to_vec(&v).unwrap()),
        Err(rad_codec::CodecError::UnsupportedVersion)
    );
}
#[test]
fn canonical_document_omits_empty_revisioned_plugin_dependency_hints() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let doc: serde_json::Value =
        serde_json::from_slice(&encode(c.configuration()).unwrap()).unwrap();
    assert!(doc.get("plugin_dependencies").is_none());
}
#[test]
fn v2_golden_preserves_plugin_hints_settings_and_exact_canonical_bytes() {
    let bytes = include_bytes!("../../../docs/fixtures/v2/profile.json");
    let data = decode_data(bytes).unwrap();
    assert_eq!(
        data.plugin_settings["fixture.plugin"].bytes(),
        &[0, 255, 42]
    );
    let hint = &data.plugin_dependencies["fixture.plugin"];
    assert_eq!(
        hint.version,
        Some(rad_core::configuration::PluginVersion {
            major: 1,
            minor: 2,
            patch: 3
        })
    );
    assert_eq!(hint.origin.as_deref(), Some("fixture/plugin"));
    let records =
        data.folders
            .iter()
            .map(|f| f.binding)
            .chain(data.buttons.iter().flat_map(|b| {
                std::iter::once(b.binding).chain(b.actions.iter().map(|a| a.binding))
            }))
            .map(|b| rad_core::identity::IdentityRecord::new(b, false));
    let registry = IdentityRegistry::from_records(77, records).unwrap();
    assert_eq!(
        encode(&Configuration::from_data(data, &registry).unwrap()).unwrap(),
        std::str::from_utf8(bytes)
            .unwrap()
            .replacen("\"version\":2", "\"version\":5", 1)
            .into_bytes()
    );
}
#[test]
fn v2_rejects_legacy_grid_and_malformed_plugin_metadata_without_normalizing() {
    use serde_json::json;
    let original: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../docs/fixtures/v2/profile.json")).unwrap();
    for (pointer, value) in [
        ("/grid/0", json!(7)),
        ("/grid/1", json!(5)),
        ("/plugin_settings", json!(null)),
        ("/plugin_settings/fixture.plugin", json!("FF")),
        ("/plugin_settings/fixture.plugin", json!("0")),
        ("/plugin_settings/fixture.plugin", json!("00".repeat(4096))),
        (
            "/plugin_dependencies/fixture.plugin/version/major",
            json!(4294967296u64),
        ),
        (
            "/plugin_dependencies/fixture.plugin/version/minor",
            json!(-1),
        ),
        ("/plugin_dependencies/fixture.plugin/origin", json!("")),
        (
            "/plugin_dependencies/fixture.plugin/origin",
            json!("\nsecret"),
        ),
        (
            "/plugin_dependencies/fixture.plugin/origin",
            json!("x".repeat(8191)),
        ),
    ] {
        let mut value_original = original.clone();
        *value_original.pointer_mut(pointer).unwrap() = value;
        assert!(
            decode_data(&serde_json::to_vec(&value_original).unwrap()).is_err(),
            "{pointer}"
        );
    }
    {
        let mut bad = original.clone();
        bad.as_object_mut().unwrap().remove("grid");
        assert_eq!(
            decode_data(&serde_json::to_vec(&bad).unwrap()),
            Err(rad_codec::CodecError::InvalidDocument)
        );
    }
    let mut bad = original.clone();
    bad["buttons"][0]["pauses_ms"] = json!(["0"]);
    assert_eq!(
        decode_data(&serde_json::to_vec(&bad).unwrap()),
        Err(rad_codec::CodecError::InvalidDocument)
    );
    let mut bad = original.clone();
    bad["settings"]["gui_language"] = json!("en");
    assert_eq!(
        decode_data(&serde_json::to_vec(&bad).unwrap()),
        Err(rad_codec::CodecError::InvalidDocument)
    );
    for field in ["plugin_settings", "plugin_dependencies"] {
        let mut bad = original.clone();
        let entry = bad[field]["fixture.plugin"].take();
        bad[field][""] = entry;
        assert!(decode_data(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let text = String::from_utf8(serde_json::to_vec(&original).unwrap()).unwrap();
    let pair = "\"fixture.plugin\":\"00ff2a\"";
    assert!(text.contains(pair));
    assert_eq!(
        decode_data(text.replacen(pair, &format!("{pair},{pair}"), 1).as_bytes()),
        Err(rad_codec::CodecError::InvalidDocument)
    );
}
#[test]
fn original_v1_at_byte_limit_and_above_new_assignment_ceiling_imports_without_loss() {
    let bytes = include_bytes!("../../../docs/fixtures/v1/large.json");
    assert_eq!(bytes.len(), 1024 * 1024);
    let data = decode_data(bytes).unwrap();
    assert_eq!(data.entity_count(), 7200);
    let records =
        data.folders
            .iter()
            .map(|f| f.binding)
            .chain(data.buttons.iter().flat_map(|b| {
                std::iter::once(b.binding).chain(b.actions.iter().map(|a| a.binding))
            }))
            .map(|b| rad_core::identity::IdentityRecord::new(b, false));
    let registry = IdentityRegistry::from_records(1, records).unwrap();
    let config = Configuration::from_data(data.clone(), &registry).unwrap();
    let encoded = encode(&config).unwrap();
    assert!(encoded.len() <= bytes.len());
    assert_eq!(decode_data(&encoded).unwrap(), data);
}
