// Dynamic options through the CLI: closed error vocabulary, argument grammar and text output.
use rad_client::{
    output::exit_code,
    response::{Error, Outcome, Response, Status},
};
use serde_json::{Value, json};

const SESSION: &str = "00000000000000000000000000000471";

fn rejected(code: &str) -> Response {
    Response {
        v: 1,
        id: Some("2".into()),
        session_id: Some(SESSION.into()),
        status: Status::Error,
        data: rad_api::ResponseData::None,
        error: Some(Error {
            code: rad_api::ErrorCode::parse(code).unwrap(),
            message: "Request rejected".into(),
            details: json!({}),
            outcome: None::<Outcome>,
        }),
    }
}

#[test]
fn options_timeout_is_a_closed_wire_code_and_a_retryable_exit_status() {
    let original = rejected("options_timeout");
    let bytes = serde_json::to_vec(&original).unwrap();
    assert_eq!(
        Response::decode_value(
            rad_wire::parse_json(&bytes).unwrap(),
            2,
            Some(SESSION),
            rad_api::Operation::Options
        )
        .unwrap(),
        original
    );
    // A neighbouring spelling is still rejected: the vocabulary stays closed.
    let mut invalid = serde_json::to_value(&original).unwrap();
    invalid["error"]["code"] = json!("options_timeouts");
    let bytes = serde_json::to_vec(&invalid).unwrap();
    assert!(
        Response::decode_value(
            rad_wire::parse_json(&bytes).unwrap(),
            2,
            Some(SESSION),
            rad_api::Operation::Options
        )
        .is_err()
    );
    assert_eq!(exit_code(&original), 5);
}

fn parsed(args: &[&str]) -> Option<(String, Value, bool)> {
    let args: Vec<String> = args.iter().map(|a| (*a).into()).collect();
    match rad_client::cli::parse(&args) {
        Ok(rad_client::cli::Invocation::Command(c)) => Some((
            c.request.operation().as_str().to_owned(),
            c.request.arguments(),
            c.json,
        )),
        _ => None,
    }
}

#[test]
fn options_for_an_entity_or_a_type_carry_the_draft_context_only_when_given() {
    for (input, arguments, json) in [
        // Existing grammar: no context key at all.
        (
            vec!["options", "a9", "member"],
            json!({"id":"a9","field":"member","cursor":null}),
            false,
        ),
        (
            vec!["options", "a9", "member", "--cursor", "next"],
            json!({"id":"a9","field":"member","cursor":"next"}),
            false,
        ),
        (
            vec![
                "options",
                "a9",
                "member",
                "--with",
                "scenario=normal",
                "--with",
                "group=b",
            ],
            json!({"id":"a9","field":"member","cursor":null,
                   "context":{"scenario":"normal","group":"b"}}),
            false,
        ),
        // A value may itself contain `=`; only the first one separates the key.
        (
            vec!["options", "b2", "group", "--with", "scenario=a=b", "--json"],
            json!({"id":"b2","field":"group","cursor":null,"context":{"scenario":"a=b"}}),
            true,
        ),
        (
            vec!["options", "--type", "org.rad.test.options:pick", "group"],
            json!({"type":"org.rad.test.options:pick","field":"group","cursor":null}),
            false,
        ),
        (
            vec![
                "--json",
                "options",
                "--type",
                "org.rad.test.options#probe",
                "group",
                "--with",
                "scenario=empty",
            ],
            json!({"type":"org.rad.test.options#probe","field":"group","cursor":null,
                   "context":{"scenario":"empty"}}),
            true,
        ),
    ] {
        assert_eq!(
            parsed(&input),
            Some(("options".into(), arguments, json)),
            "{input:?}"
        );
    }
}

#[test]
fn malformed_options_arguments_are_rejected_before_any_request() {
    for input in [
        vec!["options", "--type", "p:a"],
        vec!["options", "--type", "--with", "group"],
        vec!["options", "a9", "member", "--with"],
        vec!["options", "a9", "member", "--with", "scenario"],
        vec!["options", "a9", "member", "--with", "=normal"],
        vec!["options", "a9", "member", "--with", "scenario="],
        vec!["options", "a9", "member", "--with", "g=a", "--with", "g=b"],
        vec!["options", "a9", "member", "--cursor", "x", "--cursor", "y"],
        vec!["options", "a9", "member", "--with", "g=a", "extra"],
        vec!["options", "p:a", "member"],
        vec!["options", "--type", "p:a", "member", "--bogus"],
    ] {
        assert_eq!(parsed(&input), None, "{input:?}");
    }
}

fn answer(data: Value) -> Response {
    Response {
        v: 1,
        id: Some("2".into()),
        session_id: Some(SESSION.into()),
        status: Status::Ok,
        data: rad_api::ResponseData::decode(
            if data.as_object().is_some_and(|v| v.is_empty()) {
                rad_api::Operation::Subscribe
            } else {
                rad_api::Operation::Options
            },
            Status::Ok,
            data,
        )
        .unwrap(),
        error: None,
    }
}

fn text(operation: &str, response: &Response, structured: bool) -> String {
    let mut bytes = vec![];
    rad_client::output::write_command(operation, response, structured, &mut bytes).unwrap();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn options_text_lists_value_and_label_and_flags_a_saved_value_the_list_lost() {
    let listed = answer(json!({
        "type":"org.rad.test.options:pick","field":"member","availability":"available",
        "items":[{"value":"a1","label":"Member A1"},{"value":"a2","label":"Member A2"}],
        "next_cursor":null,"saved":"a2","saved_listed":true}));
    assert_eq!(
        text("options", &listed, false),
        "availability: available\na1 — Member A1\na2 — Member A2\nsaved: a2\n"
    );
    let lost = answer(json!({
        "type":"org.rad.test.options:pick","field":"member","availability":"available",
        "items":[{"value":"b1","label":"Member B1"}],
        "next_cursor":null,"saved":"a2","saved_listed":false}));
    assert_eq!(
        text("options", &lost, false),
        "availability: available\nb1 — Member B1\nsaved: a2 (not available)\n"
    );
    let unset = answer(json!({
        "type":"org.rad.test.options:pick","field":"member","availability":"available",
        "items":[{"value":"b1","label":"Member B1"}],
        "next_cursor":null,"saved":null,"saved_listed":false}));
    assert_eq!(
        text("options", &unset, false),
        "availability: available\nb1 — Member B1\nsaved: (none or unreadable)\n"
    );
    // By type there is no saved value to report.
    let by_type = answer(json!({
        "type":"org.rad.test.options:pick","field":"group","availability":"available",
        "items":[{"value":"a","label":"Group A"}],"next_cursor":null}));
    assert_eq!(
        text("options", &by_type, false),
        "availability: available\na — Group A\n"
    );
}

#[test]
fn an_empty_list_and_an_unavailable_source_read_differently() {
    let empty = answer(json!({
        "type":"org.rad.test.options:pick","field":"group","availability":"available",
        "items":[],"next_cursor":null,"saved":"a","saved_listed":false}));
    assert_eq!(
        text("options", &empty, false),
        "availability: available\n(no options)\nsaved: a (not available)\n"
    );
    let unavailable = answer(json!({
        "type":"org.rad.test.options:pick","field":"group","availability":"unavailable",
        "items":[],"next_cursor":null,"saved":"a","saved_listed":false}));
    assert_eq!(
        text("options", &unavailable, false),
        "availability: unavailable\nsaved: a (not available)\n"
    );
}

#[test]
fn options_json_is_the_untouched_response_and_other_commands_keep_their_text() {
    let data = json!({"type":"p:a","field":"f","availability":"unavailable",
        "items":[],"next_cursor":null});
    let response = answer(data.clone());
    let structured: Value = serde_json::from_str(&text("options", &response, true)).unwrap();
    assert_eq!(structured["data"], data);
    // Not an options answer: the generic pretty JSON, as before.
    assert_eq!(text("status", &answer(json!({})), false), "{}\n");
    // Malformed response payloads are rejected at the typed boundary, before presentation.
    assert!(
        rad_api::ResponseData::decode(
            rad_api::Operation::Options,
            Status::Ok,
            json!({"availability":"maybe","items":7})
        )
        .is_err()
    );
}

#[test]
fn options_values_are_printed_without_terminal_control_characters() {
    let hostile = answer(json!({
        "type":"p:a","field":"f","availability":"available",
        "items":[{"value":"v\u{1b}[2J","label":"l\u{7}abel"}],"next_cursor":null}));
    let out = text("options", &hostile, false);
    assert!(!out.chars().any(|c| c.is_control() && c != '\n'), "{out:?}");
}

#[test]
fn option_failures_say_what_to_do_in_plain_english() {
    for (code, hint) in [
        ("options_timeout", "did not answer in time"),
        ("resource_exhausted", "Too many"),
        ("executor_unavailable", "not installed or not ready"),
        ("invalid_request", "Not a dynamic field"),
        ("not_found", "No such entity"),
    ] {
        let out = text("options", &rejected(code), false);
        assert!(out.starts_with(&format!("{code}: ")), "{out}");
        assert!(out.contains(hint), "{code}: {out}");
        assert!(!out.contains("Request rejected"), "{code}: {out}");
    }
    // `resource_exhausted` also means a list too large for one response, which retrying
    // does not fix: the message must not promise it.
    let exhausted = text("options", &rejected("resource_exhausted"), false);
    assert!(exhausted.contains("too large"), "{exhausted}");
    assert!(!exhausted.contains("try again"), "{exhausted}");
    // Other commands keep the server's own message.
    assert_eq!(
        text("status", &rejected("not_found"), false),
        "not_found: Request rejected\n"
    );
}
