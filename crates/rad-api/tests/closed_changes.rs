//! Critical request boundary: unknown edit members must never be silently discarded.
use rad_api::Request;
use serde_json::json;
#[test]
fn edits_reject_unknown_members() {
    for change in [
        json!({"kind":"set","id":"b1","field":"name","value":"name","ignored":true}),
        json!({"kind":"move","id":"b1","position":1,"ignored":true}),
        json!({"kind":"order","id":"b1","indices":[],"ignored":true}),
    ] {
        assert!(Request::parse("edit", json!({"changes":[change]})).is_err());
    }
}

#[test]
fn empty_requests_and_payloads_require_closed_objects() {
    for operation in ["hello", "status", "overview", "subscribe", "undo", "redo"] {
        assert!(
            Request::parse(operation, json!({"ignored":true})).is_err(),
            "{operation}"
        );
        assert!(
            Request::parse(operation, json!([])).is_err(),
            "{operation} array"
        );
    }
    assert!(
        rad_api::ResponseData::decode(
            rad_api::Operation::Subscribe,
            rad_api::Status::Ok,
            json!({"ignored":true})
        )
        .is_err()
    );
    assert!(
        rad_api::ResponseData::decode(
            rad_api::Operation::Subscribe,
            rad_api::Status::Ok,
            json!([])
        )
        .is_err()
    );
}

#[test]
fn nested_structs_never_accept_positional_arrays() {
    assert!(
        Request::parse(
            "set_permissions",
            json!({"target":"b1","value":{"own":[true,true,true]}})
        )
        .is_err()
    );
    assert!(rad_api::ResponseData::decode(rad_api::Operation::Status, rad_api::Status::Ok,
        json!({"storage_state":"ready","view_revision":"1","active_folder_id":"f1","grid":[8,4]})).is_err());
}

#[test]
fn explicitly_nullable_optional_fields_remain_legacy_compatible() {
    assert!(
        Request::parse(
            "plugin_admin",
            json!({"operation":"uninstall","plugin_id":"fixture","source":null,"trusted":true})
        )
        .is_ok()
    );
    assert!(Request::parse("status", json!({"ignored":null})).is_err());
    assert!(
        Request::parse(
            "plugin_admin",
            json!({"operation":"uninstall","plugin_id":"fixture","ignored":null})
        )
        .is_err()
    );
}
