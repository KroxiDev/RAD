use rad_client::cli::*;
use serde_json::json;
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).into()).collect()
}
#[test]
fn status_json_generates_only_the_official_status_request() {
    assert_eq!(
        parse(&args(&["status", "--json"])).unwrap(),
        Invocation::Command(Command {
            request: rad_api::Request::Status {},
            json: true,
            wait_ms: None,
        })
    );
}
#[test]
fn query_commands_preserve_exact_ids_and_do_not_offer_admin_or_socket_override() {
    for (input, operation, arguments) in [
        (
            vec!["get", "f18446744073709551615"],
            "get",
            json!({"id":"f18446744073709551615"}),
        ),
        (vec!["list"], "list", json!({"cursor":null,"limit":32})),
        (
            vec!["list", "--cursor", "opaque"],
            "list",
            json!({"cursor":"opaque","limit":32}),
        ),
        (
            vec!["schema", "launcher"],
            "schema",
            json!({"type":"launcher","cursor":null}),
        ),
        (
            vec!["options", "a9", "application", "--cursor", "next"],
            "options",
            json!({"id":"a9","field":"application","cursor":"next"}),
        ),
    ] {
        assert_eq!(
            parse(&args(&input)).unwrap(),
            Invocation::Command(Command {
                request: rad_api::Request::parse(operation, arguments).unwrap(),
                json: false,
                wait_ms: None
            })
        );
    }
    for input in [
        vec!["get", "b0"],
        vec!["get", "b01"],
        vec!["get", "a18446744073709551616"],
        vec!["get", "b١"],
        vec!["delete", "b1"],
        vec!["create_folder"],
        vec!["status", "--human"],
        vec!["status", "--socket", "editor.sock"],
        vec!["status", "--json", "--json"],
        vec!["list", "--cursor", "x", "extra"],
    ] {
        assert!(parse(&args(&input)).is_err(), "{input:?}");
    }
}
#[test]
fn edit_commands_encode_existing_entity_changes_not_documents_or_administration() {
    for (input, changes) in [
        (
            vec!["set", "b1", "name", "\"Studio\""],
            json!([{"kind":"set","id":"b1","field":"name","value":"Studio"}]),
        ),
        (
            vec!["set", "a3", "milliseconds", "42"],
            json!([{"kind":"set","id":"a3","field":"milliseconds","value":42}]),
        ),
        (
            vec!["move", "b1", "31"],
            json!([{"kind":"move","id":"b1","position":31}]),
        ),
        (
            vec!["order", "b2", "--indices", "[1,0]"],
            json!([{"kind":"order","id":"b2","indices":[1,0]}]),
        ),
    ] {
        assert_eq!(
            parse(&args(&input)).unwrap(),
            Invocation::Command(Command {
                request: rad_api::Request::parse("edit", json!({"changes":changes})).unwrap(),
                json: false,
                wait_ms: None
            })
        );
    }
    for input in [
        vec!["set", "a3", "volume", "42"],
        vec!["move", "b1", "32"],
        vec!["move", "a1", "0"],
        vec!["move", "b1", "01"],
        vec!["set", "b1", "name", "not-json"],
        vec!["order", "b1", "--indices", "[0,0]"],
        vec!["order", "b1", "--indices", "[1]"],
    ] {
        assert!(parse(&args(&input)).is_err(), "{input:?}");
    }
}
#[test]
fn execution_wait_options_never_become_server_side_wait_or_human_bypass() {
    let execution = "00000000000000000000000000000001:18446744073709551615";
    for (input, op, value, wait) in [
        (vec!["run", "b17"], "run", json!({"id":"b17"}), None),
        (
            vec!["run", "a2", "--wait"],
            "run",
            json!({"id":"a2"}),
            Some(300000),
        ),
        (
            vec!["run", "b3", "--wait-timeout-ms", "1", "--wait"],
            "run",
            json!({"id":"b3"}),
            Some(1),
        ),
        (
            vec![
                "result",
                execution,
                "--wait",
                "--wait-timeout-ms",
                "3600000",
            ],
            "result",
            json!({"execution_id":execution}),
            Some(3600000),
        ),
    ] {
        assert_eq!(
            parse(&args(&input)).unwrap(),
            Invocation::Command(Command {
                request: rad_api::Request::parse(op, value).unwrap(),
                json: false,
                wait_ms: wait
            })
        );
    }
    for input in [
        vec!["run", "f1"],
        vec!["run", "b1", "--wait-timeout-ms", "10"],
        vec!["run", "b1", "--wait", "--wait-timeout-ms", "0"],
        vec!["run", "b1", "--wait", "--wait-timeout-ms", "3600001"],
        vec!["result", "1:1"],
        vec!["result", "00000000000000000000000000000000:1"],
        vec!["result", "00000000000000000000000000000001:01"],
        vec!["run", "b1", "--wait", "--wait"],
    ] {
        assert!(parse(&args(&input)).is_err(), "{input:?}");
    }
}
#[test]
fn json_value_rejects_duplicate_keys_and_structural_limits_before_normalization() {
    for value in [
        "{\"x\":1,\"x\":2}".into(),
        "{\"outer\":{\"x\":1,\"\\u0078\":2}}".into(),
        format!("\"{}\"", "x".repeat(16385)),
        format!("{}0{}", "[".repeat(17), "]".repeat(17)),
    ] {
        assert!(parse(&args(&["set", "a1", "parameters", &value])).is_err());
    }
}
#[test]
fn bounded_json_seam_is_shared_and_never_returns_parser_input() {
    assert_eq!(
        rad_wire::parse_json(b"18446744073709551615").unwrap(),
        json!(u64::MAX)
    );
    assert_eq!(rad_wire::parse_json(b"null").unwrap(), json!(null));
    assert_eq!(rad_wire::parse_json(b""), Err(rad_wire::JsonError));
    assert_eq!(
        rad_wire::parse_json(&vec![b' '; 65537]),
        Err(rad_wire::JsonError)
    );
    assert_eq!(
        rad_wire::parse_json(b"{\"secret\":bad}"),
        Err(rad_wire::JsonError)
    );
    assert!(rad_wire::parse_json("[0,".repeat(20).as_bytes()).is_err());
}
#[test]
fn order_accepts_twenty_actions_and_rejects_twenty_one_before_any_request() {
    let order = |count: usize| {
        let indices: Vec<usize> = (0..count).collect();
        parse(&args(&[
            "order",
            "b1",
            "--indices",
            &serde_json::to_string(&indices).unwrap(),
        ]))
    };
    assert!(order(20).is_ok());
    assert!(order(21).is_err());
}
#[test]
fn order_takes_only_the_new_sequence_and_refuses_the_retired_pauses_option() {
    assert_eq!(
        parse(&args(&["order", "b2", "--indices", "[1,0]"])).unwrap(),
        Invocation::Command(Command {
            request: rad_api::Request::Edit {
                changes: vec![rad_api::Change::Order {
                    id: "b2".into(),
                    indices: vec![1, 0]
                }]
            },
            json: false,
            wait_ms: None
        })
    );
    assert!(
        parse(&args(&[
            "order",
            "b2",
            "--indices",
            "[1,0]",
            "--pauses-ms",
            "[0]"
        ]))
        .is_err()
    );
}
