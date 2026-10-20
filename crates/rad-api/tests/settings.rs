use rad_api::Request;
use serde_json::json;
#[test]
fn plugin_settings_use_closed_typed_requests_and_preserve_plugin_values() {
    let read = Request::parse("plugin_settings", json!({"plugin_id":"fixture"})).unwrap();
    assert_eq!(read.operation().as_str(), "plugin_settings");
    let args =
        json!({"plugin_id":"fixture","parameters":{"schema_version":1,"values":{"port":4455}}});
    assert_eq!(
        Request::parse("set_plugin_settings", args.clone())
            .unwrap()
            .arguments(),
        args
    );
    assert!(
        Request::parse(
            "set_plugin_settings",
            json!({"plugin_id":"fixture","parameters":null,"secret":"not-a-setting"})
        )
        .is_err()
    );
}
