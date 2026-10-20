use rad_api::{Operation, Request};
use serde_json::json;
#[test]
fn representative_wire_vectors_share_structural_definition() {
    for (op, args) in [
        ("hello", json!({})),
        ("board", json!({"folder_id":null})),
        ("inspector", json!({"id":"b1"})),
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
fn response_payloads_are_operation_directed_even_for_empty_pages() {
    use rad_api::{Operation, ResponseData, Status};
    let page = serde_json::json!({"items":[],"next_cursor":null});
    assert!(matches!(
        ResponseData::decode(Operation::PluginCatalog, Status::Ok, page.clone()).unwrap(),
        ResponseData::Catalog(_)
    ));
    assert!(matches!(
        ResponseData::decode(Operation::LauncherCatalog, Status::Ok, page.clone()).unwrap(),
        ResponseData::Launcher(_)
    ));
    assert!(matches!(
        ResponseData::decode(Operation::Options, Status::Ok, page).unwrap(),
        ResponseData::StaticOptions(_)
    ));
    assert!(
        ResponseData::decode(
            Operation::Status,
            Status::Ok,
            serde_json::json!({"changed":true,"revision":"1"})
        )
        .is_err()
    );
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

#[test]
fn flattened_response_views_reject_unknown_null_recursively() {
    use rad_api::{ResponseData, Status};
    let entity = json!({"visibility":"visible","id":"f1","kind":"folder","fields":{"name":null,"entry_button_id":null,"parent_id":null,"slots":[]},"view_revision":"1","sections":[]});
    assert!(ResponseData::decode(Operation::Get, Status::Ok, entity.clone()).is_ok());
    for pointer in ["", "/fields"] {
        let mut invalid = entity.clone();
        invalid
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), json!(null));
        assert!(
            ResponseData::decode(Operation::Get, Status::Ok, invalid).is_err(),
            "{pointer}"
        );
    }
    let mut action = entity.clone();
    action["unknown"] = json!(null);
    let inspector = json!({"view_revision":"1","entity":entity,"appearance":null,"actions":[action],"plugin":null});
    assert!(ResponseData::decode(Operation::Inspector, Status::Ok, inspector).is_err());
}

#[test]
fn strict_views_preserve_nullable_members_and_reject_nested_positional_objects() {
    use rad_api::{ResponseData, Status};
    let entity = json!({"visibility":"visible","id":"f1","kind":"folder","fields":{"name":null,"entry_button_id":null,"parent_id":null,"slots":[]},"view_revision":"1","sections":[]});
    let inspector = json!({"view_revision":"1","entity":entity,"appearance":null,"actions":[entity],"plugin":null});
    assert!(ResponseData::decode(Operation::Inspector, Status::Ok, inspector.clone()).is_ok());
    let mut invalid = inspector;
    invalid["actions"][0]["fields"] = json!([null, null, null, []]);
    assert!(ResponseData::decode(Operation::Inspector, Status::Ok, invalid).is_err());
    let board = json!({"view_revision":"1","grid":{"columns":1,"rows":1},"folder":entity,"keys":[{"entity":entity,"appearance":{"text":"","image":null,"text_position":"bottom","visual_type":null},"visual":null}]});
    assert!(ResponseData::decode(Operation::Board, Status::Ok, board.clone()).is_ok());
    let mut invalid = board;
    invalid["keys"][0]["entity"]["unknown"] = json!(null);
    assert!(ResponseData::decode(Operation::Board, Status::Ok, invalid).is_err());
    assert!(
        Request::parse(
            "options",
            json!({"type":"p:a","field":"device","cursor":null})
        )
        .is_ok()
    );
    assert!(
        Request::parse(
            "options",
            json!({"type":"p:a","field":"device","cursor":null,"context":null})
        )
        .is_err()
    );
    assert!(Request::parse("options", json!({"type":"p:a","field":"device"})).is_err());
}

#[test]
fn restricted_response_variants_reject_unknown_null_fields() {
    use rad_api::{ResponseData, Status};
    let entity = json!({"visibility":"visible","id":"f1","kind":"folder","fields":{"name":null,"entry_button_id":null,"parent_id":null,"slots":[{"visibility":"restricted"}]},"view_revision":"1","sections":[]});
    assert!(ResponseData::decode(Operation::Get, Status::Ok, entity.clone()).is_ok());
    let mut invalid = entity.clone();
    invalid["fields"]["slots"][0]["ignored"] = json!(null);
    assert!(ResponseData::decode(Operation::Get, Status::Ok, invalid).is_err());
    let inspector = json!({"view_revision":"1","entity":entity,"appearance":null,"actions":[{"visibility":"restricted"}],"plugin":null});
    assert!(ResponseData::decode(Operation::Inspector, Status::Ok, inspector.clone()).is_ok());
    let mut invalid = inspector;
    invalid["actions"][0]["ignored"] = json!(null);
    assert!(ResponseData::decode(Operation::Inspector, Status::Ok, invalid).is_err());
}
