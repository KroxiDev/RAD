use rad_api::Request;
#[test]
fn parses_action_and_visual_changes_in_one_edit() {
    let edit = serde_json::json!({"op":"edit","args":{"changes":[{"kind":"add_action","button_id":"b1","type_key":"org.rad.audio:microphone","parameters":{"schema_version":1,"values":{}}},{"kind":"set","id":"b1","field":"visual","value":{"type":"org.rad.audio#microphone","parameters":{"schema_version":1,"values":{}}}}]}});
    let request = serde_json::from_value::<Request>(edit);
    assert!(request.is_ok(), "{request:?}");
}
