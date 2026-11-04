use rad_api::Request;
#[test]
fn appearance_accepts_independent_nullable_style_updates_and_full_state_faces() {
    for field in ["text_size", "text_color", "text_band"] {
        for value in [
            serde_json::Value::Null,
            match field {
                "text_size" => 24.into(),
                "text_color" => "#102030".into(),
                _ => false.into(),
            },
        ] {
            let mut appearance = serde_json::json!({"text":"Saved", "image":null});
            appearance[field] = value;
            let request = Request::parse(
                "edit",
                serde_json::json!({"changes":[{"kind":"set","id":"b1","field":"appearance","value":appearance}]}),
            );
            assert!(request.is_ok(), "{field}: {:?}", request.err());
        }
    }
}
