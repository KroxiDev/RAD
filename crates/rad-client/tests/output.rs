use rad_client::{
    output::{exit_code, write},
    response::{Error, Outcome, Response, Status},
};
use serde_json::{Value, json};
fn response(code: &str, outcome: Option<Outcome>) -> Response {
    Response {
        v: 1,
        id: None,
        session_id: None,
        status: Status::Error,
        data: Value::Null,
        error: Some(Error {
            code: code.into(),
            message: "Safe message".into(),
            details: json!({}),
            outcome,
        }),
    }
}
#[test]
fn exit_codes_distinguish_rejection_failure_storage_transport_and_unknown_outcomes() {
    for (code, expected) in [
        ("invalid_request", 2),
        ("unsupported_version", 2),
        ("invalid_configuration", 2),
        ("identity_collision", 2),
        ("cursor_expired", 2),
        ("permission_denied", 3),
        ("operation_not_allowed", 3),
        ("not_found", 4),
        ("result_expired", 4),
        ("busy", 5),
        ("resource_exhausted", 5),
        ("executor_unavailable", 5),
        ("service_unavailable", 5),
        ("action_failed", 6),
        ("action_timeout", 6),
        ("storage_error", 7),
        ("recovery_required", 7),
        ("runtime_unavailable", 8),
        ("deadline_exceeded", 8),
        ("storage_uncertain", 7),
        ("wait_timeout", 9),
    ] {
        assert_eq!(exit_code(&response(code, None)), expected, "{code}");
    }
    assert_eq!(
        exit_code(&response("storage_uncertain", Some(Outcome::Unknown))),
        8
    );
    let mut ok = response("invalid_request", None);
    ok.status = Status::Ok;
    ok.error = None;
    for (state, expected) in [
        ("running", 0),
        ("succeeded", 0),
        ("failed", 6),
        ("timed_out", 6),
    ] {
        ok.data = json!({"state":state});
        assert_eq!(exit_code(&ok), expected);
    }
}
#[test]
fn structured_output_is_one_complete_response_and_text_is_english() {
    let response = response("permission_denied", Some(Outcome::NotApplied));
    let mut bytes = vec![];
    write(&response, true, &mut bytes).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["error"]["code"], "permission_denied");
    assert_eq!(bytes.last(), Some(&b'\n'));
    bytes.clear();
    write(&response, false, &mut bytes).unwrap();
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "permission_denied: Safe message\n"
    );
}
