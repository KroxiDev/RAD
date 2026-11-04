use rad_codec::{CodecError, decode_data, encode};
use rad_core::{configuration::*, identity::*};
#[test]
fn binary_faces_round_trip_and_v5_stays_closed() {
    let golden = include_bytes!("../../../docs/fixtures/v5/profile.json");
    let mut value: serde_json::Value = serde_json::from_slice(golden).unwrap();
    value["version"] = 6.into();
    value["buttons"][0]["appearance"]["states"] = serde_json::json!({"muted":{"text":"Quiet","image":"a".repeat(64)},"unmuted":{"text":"Live","image":null}});
    let data = decode_data(&serde_json::to_vec(&value).unwrap()).expect("v6 binary faces");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let c = initial
        .configuration()
        .prepare_restore(initial.registry(), Origin::Editor, true, data)
        .unwrap();
    let encoded: serde_json::Value =
        serde_json::from_slice(&encode(c.configuration()).unwrap()).unwrap();
    assert_eq!(encoded["version"], 7);
    assert_eq!(
        encoded["buttons"][0]["appearance"]["states"],
        value["buttons"][0]["appearance"]["states"]
    );
    value["version"] = 5.into();
    assert_eq!(
        decode_data(&serde_json::to_vec(&value).unwrap()),
        Err(CodecError::InvalidDocument)
    );
    assert!(decode_data(golden).is_ok());
    value["version"] = 8.into();
    assert_eq!(
        decode_data(&serde_json::to_vec(&value).unwrap()),
        Err(CodecError::UnsupportedVersion)
    );
}

#[test]
fn state_keys_are_identifiers_and_at_most_two_faces_are_accepted() {
    let mut v: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../docs/fixtures/v5/profile.json")).unwrap();
    v["version"] = 6.into();
    for states in [
        serde_json::json!({"Bad State":{"text":"","image":null}}),
        serde_json::json!({"a":{"text":"","image":null},"b":{"text":"","image":null},"c":{"text":"","image":null}}),
    ] {
        v["buttons"][0]["appearance"]["states"] = states;
        assert!(decode_data(&serde_json::to_vec(&v).unwrap()).is_err());
    }
}
