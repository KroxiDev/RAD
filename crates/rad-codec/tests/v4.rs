use rad_codec::{CodecError, decode_data, encode};
use rad_core::{configuration::*, identity::*};

#[test]
fn legacy_sleep_roundtrips_current_and_is_rejected_under_every_version_before_v4() {
    let initial = Configuration::prepare_new(&IdentityRegistry::new(61).unwrap()).unwrap();
    let c = initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: initial.configuration().data().principal,
                position: 0,
                name: "Sleep".into(),
            },
        )
        .unwrap();
    let b = c.configuration().data().buttons[0].binding.identity();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::AddAction {
                button: b,
                kind: ActionKind::Sleep,
            },
        )
        .unwrap();
    let bytes = encode(c.configuration()).unwrap();
    assert_eq!(rad_codec::document_version(&bytes).unwrap(), 5);
    assert_eq!(decode_data(&bytes).unwrap(), *c.configuration().data());
    let mut legacy: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    legacy["version"] = serde_json::json!(4);
    assert_eq!(
        decode_data(&serde_json::to_vec(&legacy).unwrap()).unwrap(),
        *c.configuration().data()
    );
    for old in [
        include_bytes!("../../../docs/fixtures/v1/profile.json").as_slice(),
        include_bytes!("../../../docs/fixtures/v2/profile.json").as_slice(),
        include_bytes!("../../../docs/fixtures/v3/profile.json").as_slice(),
    ] {
        let mut value: serde_json::Value = serde_json::from_slice(old).unwrap();
        value["buttons"][0]["actions"][0]["kind"] = serde_json::json!("sleep");
        assert_eq!(
            decode_data(&serde_json::to_vec(&value).unwrap()),
            Err(CodecError::InvalidDocument)
        );
    }
    // Hold cannot be fabricated by a restored document, either.
    let mut data = decode_data(&bytes).unwrap();
    data.buttons[0].gesture = Gesture::Hold;
    assert_eq!(
        Configuration::from_data(data, c.registry()),
        Err(ConfigurationError::InvalidActions)
    );
}
