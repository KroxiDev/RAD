use rad_codec::{decode_data, encode};
use rad_core::{configuration::Configuration, identity::IdentityRegistry};

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
