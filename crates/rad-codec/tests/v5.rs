use rad_codec::{CodecError, decode_data, encode};
use rad_core::{configuration::*, identity::*};
#[test]
fn brightness_roundtrips_only_in_v5_and_legacy_floor_is_the_only_normalized_value() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(93).unwrap()).unwrap();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "Brightness".into(),
            },
        )
        .unwrap();
    let b = c.configuration().data().buttons[0].binding.identity();
    for direction in [BrightnessDirection::Increase, BrightnessDirection::Decrease] {
        let c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::AddAction {
                    button: b,
                    kind: ActionKind::Brightness(direction),
                },
            )
            .unwrap();
        let bytes = encode(c.configuration()).unwrap();
        assert_eq!(rad_codec::document_version(&bytes).unwrap(), 5);
        assert_eq!(decode_data(&bytes).unwrap(), *c.configuration().data());
        let mut invalid = decode_data(&bytes).unwrap();
        invalid.buttons[0].gesture = Gesture::Hold;
        assert_eq!(
            Configuration::from_data(invalid, c.registry()),
            Err(ConfigurationError::InvalidActions)
        );
    }
    for bytes in [
        include_bytes!("../../../docs/fixtures/v1/profile.json").as_slice(),
        include_bytes!("../../../docs/fixtures/v2/profile.json").as_slice(),
        include_bytes!("../../../docs/fixtures/v3/profile.json").as_slice(),
        include_bytes!("../../../docs/fixtures/v4/profile.json").as_slice(),
    ] {
        let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        for n in 0..=9 {
            value["settings"]["brightness_percent"] = serde_json::json!(n);
            let data = decode_data(&serde_json::to_vec(&value).unwrap()).unwrap();
            assert_eq!(data.settings.brightness_percent, 10);
        }
        value["settings"]["brightness_percent"] = serde_json::json!(101);
        assert_eq!(
            decode_data(&serde_json::to_vec(&value).unwrap()),
            Err(CodecError::InvalidValue)
        );
        value["settings"]["brightness_percent"] = serde_json::json!(55);
        value["buttons"][0]["actions"][0]["kind"] = serde_json::json!({"brightness":"increase"});
        assert_eq!(
            decode_data(&serde_json::to_vec(&value).unwrap()),
            Err(CodecError::InvalidDocument)
        );
    }
    let original = include_bytes!("../../../docs/fixtures/v4/profile.json");
    let mut data = decode_data(original).unwrap();
    assert_eq!(data.settings.brightness_percent, 10);
    data.settings.brightness_percent = 5;
    let registry = IdentityRegistry::from_records(
        77,
        data.folders
            .iter()
            .map(|f| f.binding)
            .chain(data.buttons.iter().flat_map(|b| {
                std::iter::once(b.binding).chain(b.actions.iter().map(|a| a.binding))
            }))
            .map(|b| IdentityRecord::new(b, false)),
    )
    .unwrap();
    assert!(Configuration::from_data(data, &registry).is_err());
}
