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
#[test]
fn help_needs_no_runtime_and_missing_runtime_is_transport_not_input_error() {
    let help = run(&["--help"]);
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(text.contains("rad-cli run ID"));
    assert!(!text.contains("--socket"));
    let o = run(&["status", "--json"]);
    assert_eq!(o.status.code(), Some(8));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["error"]["code"], "runtime_unavailable");
    assert_eq!(v["error"]["outcome"], Value::Null);
    assert!(String::from_utf8_lossy(&o.stderr).contains("Runtime unavailable"));
}
#[test]
fn failed_connection_before_run_is_definitely_not_applied() {
    let o = Command::new(env!("CARGO_BIN_EXE_rad-cli"))
        .env_clear()
        .env("XDG_RUNTIME_DIR", "/this/rad-test-path/does-not-exist")
        .args(["run", "b1", "--json"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(8));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["error"]["outcome"], "not_applied");
}
