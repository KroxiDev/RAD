use rad_codec::{decode_data, encode};
use rad_core::{
    configuration::Configuration,
    identity::{IdentityRecord, IdentityRegistry},
};

#[test]
fn v3_golden_import_keeps_nondefault_gesture_and_long_threshold() {
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
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .replacen("\"version\":3", "\"version\":5", 1)
            .as_bytes()
    );
}
