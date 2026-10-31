use rad_api::{Operation, Request};
use serde_json::json;
#[test]
fn representative_wire_vectors_share_structural_definition() {
    for (op, args) in [
        ("hello", json!({})),
        ("board", json!({"folder_id":null})),
        ("inspector", json!({"id":"b1"})),
        (
            "add_action",
            json!({"button_id":"b1","type":"sleep","parameters":null}),
        ),
        (
            "edit",
            json!({"changes":[{"kind":"set","id":"b1","field":"name","value":"Name"}]}),
        ),
        (
            "edit",
            json!({"changes":[{"kind":"set","id":"b1","field":"appearance","value":{"text":"T","image":null,"text_position":null}}]}),
        ),
        (
            "options",
            json!({"type":"p:a","field":"device","cursor":null,"context":{"server":"local"}}),
        ),
        (
            "credential_replace",
            json!({"plugin_id":"p","slot":"s","secret":"synthetic"}),
        ),
    ] {
        let request = Request::parse(op, args.clone()).unwrap();
        assert_eq!(request.operation().as_str(), op);
        assert_eq!(request.arguments(), args);
    }
    assert!(!Operation::CredentialStatus.is_mutation());
    assert!(Operation::CredentialReplace.is_mutation());
}
#[test]
fn closed_request_types_reject_unknown_fields_and_wrong_payloads() {
    for (op, args) in [
        ("board", json!({"folder_id":null,"role":"editor"})),
        ("run", json!({"id":true})),
        ("made_up", json!({})),
        (
            "get_section",
            json!({"id":"b1","section":"secrets","cursor":null}),
        ),
    ] {
        assert!(Request::parse(op, args).is_err());
    }
}
#[test]
fn request_debug_never_contains_credential_payload() {
    let request = Request::CredentialReplace {
        plugin_id: "p".into(),
        slot: "s".into(),
        secret: "synthetic-secret-never-print".into(),
    };
    assert!(!format!("{request:?}").contains("synthetic-secret-never-print"));
}

#[test]
fn response_status_and_mutation_outcomes_belong_to_the_operation_contract() {
    use rad_api::{Operation, Response};
    let session = "00000000000000000000000000000007";
    let applied = json!({"v":1,"id":"2","session_id":session,"status":"applied","data":{"changed":true,"revision":"1"},"error":null});
    assert!(Response::decode_value(applied.clone(), 2, Some(session), Operation::Edit).is_ok());
    assert!(Response::decode_value(applied, 2, Some(session), Operation::Status).is_err());
    let error = json!({"v":1,"id":"2","session_id":session,"status":"error","data":null,"error":{"code":"busy","message":"Busy","details":{},"outcome":null}});
    assert!(Response::decode_value(error.clone(), 2, Some(session), Operation::Status).is_ok());
    assert!(Response::decode_value(error, 2, Some(session), Operation::Edit).is_err());
}
