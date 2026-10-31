use serde_json::{Value, json};
use std::process::Command;
fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rad-cli"))
        .env_clear()
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn invalid_cli_input_is_one_sanitized_json_error_without_connecting_or_echoing_args() {
    let o = run(&["--json", "set", "b1", "name", "secret-unparseable"]);
    assert_eq!(o.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&o.stdout).expect("one Response JSON");
    assert_eq!(v["id"], json!(null));
    assert_eq!(v["session_id"], json!(null));
    assert_eq!(v["error"]["code"], "invalid_request");
    assert_eq!(v["error"]["outcome"], json!("not_applied"));
    assert!(!String::from_utf8_lossy(&o.stdout).contains("secret-unparseable"));
    assert!(o.stderr.is_empty());
}
