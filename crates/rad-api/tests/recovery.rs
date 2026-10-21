//! Recovery mode (ADR 0011): two closed operations and one closed view.
use rad_api::{
    EventCode, Operation, RecoveryAction, RecoveryCause, RecoveryDetailCode, Request, Response,
    ResponseData, Status,
};
use serde_json::json;

#[test]
fn recovery_operations_are_closed_and_classified() {
    let query = Request::parse("recovery", json!({})).unwrap();
    assert_eq!(query.operation(), Operation::Recovery);
    assert_eq!(Operation::parse("recovery"), Some(Operation::Recovery));
    assert!(!Operation::Recovery.is_mutation());
    assert_eq!(Operation::Recovery.success_status(), Status::Ok);
    assert!(Request::parse("recovery", json!({"confirm":true})).is_err());

    let reset = Request::parse("recovery_reset", json!({"confirm":true})).unwrap();
    assert_eq!(reset.operation(), Operation::RecoveryReset);
    assert_eq!(reset.operation().as_str(), "recovery_reset");
    assert!(Operation::RecoveryReset.is_mutation());
    assert_eq!(Operation::RecoveryReset.success_status(), Status::Applied);
    assert_eq!(reset.arguments(), json!({"confirm":true}));
    for bad in [
        json!({}),
        json!({"confirm":"yes"}),
        json!({"confirm":true,"x":1}),
    ] {
        assert!(Request::parse("recovery_reset", bad).is_err());
    }
}

#[test]
fn recovery_view_decodes_by_operation_with_closed_snake_case_enums() {
    let answer = json!({
        "cause":"unreadable",
        "actions":["reset","details"],
        "detail":{"code":"corrupt","counters_read":true}
    });
    let ResponseData::Recovery(view) =
        ResponseData::decode(Operation::Recovery, Status::Ok, answer.clone()).unwrap()
    else {
        panic!("recovery answer");
    };
    assert_eq!(view.cause, RecoveryCause::Unreadable);
    assert_eq!(
        view.actions,
        [RecoveryAction::Reset, RecoveryAction::Details]
    );
    assert_eq!(view.detail.code, RecoveryDetailCode::Corrupt);
    assert_eq!(serde_json::to_value(&view).unwrap(), answer);

    for (cause, code) in [
        ("newer_version", "unsupported_version"),
        ("unsafe_folder", "unsafe_directory"),
        ("unsafe_folder", "unsafe_database_file"),
        ("unreadable", "invalid"),
    ] {
        let answer = json!({"cause":cause,"actions":["details"],"detail":{"code":code,"counters_read":false}});
        assert!(ResponseData::decode(Operation::Recovery, Status::Ok, answer).is_ok());
    }
    for bad in [
        json!({"cause":"made_up","actions":[],"detail":{"code":"corrupt","counters_read":false}}),
        json!({"cause":"unreadable","actions":["made_up"],"detail":{"code":"corrupt","counters_read":false}}),
        json!({"cause":"unreadable","actions":[],"detail":{"code":"made_up","counters_read":false}}),
        json!({"cause":"unreadable","actions":[],"detail":{"code":"corrupt","counters_read":"no"}}),
        json!({"cause":"unreadable","actions":[],"detail":{"code":"corrupt"}}),
        json!({"cause":"unreadable","actions":[],"detail":{"code":"corrupt","counters_read":false},"x":1}),
    ] {
        assert!(ResponseData::decode(Operation::Recovery, Status::Ok, bad).is_err());
    }
}

#[test]
fn recovery_reset_answers_as_applied_and_the_events_exist() {
    let session = "0123456789abcdef0123456789abcdef";
    let reply = json!({"v":1,"id":"3","session_id":session,"status":"applied",
        "data":{"changed":true,"revision":"7"},"error":null});
    let response =
        Response::decode_value(reply, 3, Some(session), Operation::RecoveryReset).unwrap();
    assert_eq!(response.status, Status::Applied);
    let refused = json!({"v":1,"id":"4","session_id":session,"status":"error","data":null,
        "error":{"code":"confirmation_required","message":"Request rejected","details":{},"outcome":"not_applied"}});
    assert!(Response::decode_value(refused, 4, Some(session), Operation::RecoveryReset).is_ok());
    assert_eq!(EventCode::RecoveryMode.as_str(), "recovery_mode");
    assert_eq!(EventCode::RecoveryReset.as_str(), "recovery_reset");
}
