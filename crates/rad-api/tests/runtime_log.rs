//! `runtime_log`: an Editor read of the runtime's recent events, with a closed vocabulary.
use rad_api::{EventCode, Operation, Request, ResponseData, RuntimeLogView, Status};
use serde_json::json;

#[test]
fn runtime_log_is_a_closed_read_operation() {
    let request = Request::parse("runtime_log", json!({})).unwrap();
    assert_eq!(request.operation(), Operation::RuntimeLog);
    assert_eq!(request.operation().as_str(), "runtime_log");
    assert_eq!(Operation::parse("runtime_log"), Some(Operation::RuntimeLog));
    assert!(!Operation::RuntimeLog.is_mutation());
    assert!(Request::parse("runtime_log", json!({"since":"1"})).is_err());
    assert!(Request::parse("runtime_log", json!([])).is_err());
}

#[test]
fn runtime_log_answers_decode_by_operation_with_decimal_strings_and_closed_codes() {
    let answer = json!({
        "entries":[
            {"seq":"18446744073709551615","at_unix_ms":"1760000000000","code":"started"},
            {"seq":"2","at_unix_ms":"1760000000001","code":"plugin_failed","subject":"org.rad.audio"}
        ],
        "dropped":"0"
    });
    let ResponseData::RuntimeLog(view) =
        ResponseData::decode(Operation::RuntimeLog, Status::Ok, answer.clone()).unwrap()
    else {
        panic!("runtime log answer");
    };
    assert_eq!(view.entries[1].code, EventCode::PluginFailed);
    assert_eq!(serde_json::to_value(&view).unwrap(), answer);
    // Integers are strings, codes are a closed set, and nothing unknown is ignored.
    for bad in [
        json!({"entries":[{"seq":1,"at_unix_ms":"1","code":"started"}],"dropped":"0"}),
        json!({"entries":[{"seq":"1","at_unix_ms":"1","code":"made_up"}],"dropped":"0"}),
        json!({"entries":[{"seq":"1","at_unix_ms":"1","code":"started","extra":true}],"dropped":"0"}),
        json!({"entries":[],"dropped":0}),
        json!({"entries":[],"dropped":"0","extra":true}),
    ] {
        assert!(ResponseData::decode(Operation::RuntimeLog, Status::Ok, bad).is_err());
    }
}

#[test]
fn event_codes_are_stable_snake_case_wire_names() {
    for code in EventCode::ALL {
        let wire = serde_json::to_value(code).unwrap();
        assert_eq!(wire, json!(code.as_str()));
        assert!(
            code.as_str()
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'_'),
            "{code:?}"
        );
        assert_eq!(serde_json::from_value::<EventCode>(wire).unwrap(), *code);
    }
    let unique: std::collections::BTreeSet<_> = EventCode::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(unique.len(), EventCode::ALL.len());
    let _: Option<RuntimeLogView> = None;
}
