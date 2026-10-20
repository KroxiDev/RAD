use rad_codec::{CodecError, decode_data, encode};
use rad_core::{
    configuration::Configuration,
    identity::{IdentityRecord, IdentityRegistry},
};

#[test]
fn v3_imports_real_v2_without_losing_configuration_or_inflating_default_profiles() {
    let old = include_bytes!("../../../docs/fixtures/v2/profile.json");
    let data = decode_data(old).unwrap();
    let records =
        data.folders
            .iter()
            .map(|f| f.binding)
            .chain(data.buttons.iter().flat_map(|b| {
                std::iter::once(b.binding).chain(b.actions.iter().map(|a| a.binding))
            }))
            .map(|b| IdentityRecord::new(b, false));
    let registry = IdentityRegistry::from_records(77, records).unwrap();
    let config = Configuration::from_data(data.clone(), &registry).unwrap();
    let bytes = encode(&config).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["version"], 3);
    assert_eq!(decode_data(&bytes).unwrap(), data);
    assert_eq!(bytes.len(), old.len());
    assert_eq!(
        data.plugin_settings["fixture.plugin"].bytes(),
        &[0, 255, 42]
    );
    assert_eq!(
        data.plugin_dependencies["fixture.plugin"].origin.as_deref(),
        Some("fixture/plugin")
    );
}

#[test]
fn v2_importer_rejects_v3_gesture_fields_instead_of_silently_accepting_them() {
    let mut old: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../docs/fixtures/v2/profile.json")).unwrap();
    old["buttons"][0]["gesture"] = serde_json::json!("hold");
    assert_eq!(
        decode_data(&serde_json::to_vec(&old).unwrap()),
        Err(CodecError::InvalidDocument)
    );
}

#[test]
fn canonical_v3_golden_keeps_nondefault_gesture_and_long_threshold() {
    let bytes = include_bytes!("../../../docs/fixtures/v3/profile.json");
    let data = decode_data(bytes).unwrap();
    assert_eq!(
        data.buttons[0].gesture,
        rad_core::configuration::Gesture::LongPress
    );
    assert_eq!(data.settings.long_press_threshold_ms, 750);
    let records =
        data.folders
            .iter()
            .map(|f| f.binding)
            .chain(data.buttons.iter().flat_map(|b| {
                std::iter::once(b.binding).chain(b.actions.iter().map(|a| a.binding))
            }))
            .map(|b| IdentityRecord::new(b, false));
    let registry = IdentityRegistry::from_records(77, records).unwrap();
    assert_eq!(
        encode(&Configuration::from_data(data, &registry).unwrap()).unwrap(),
        bytes
    );
}
