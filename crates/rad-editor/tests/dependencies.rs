use rad_editor::dependencies::Dependencies;
use serde_json::{Value, json};
fn page(revision: &str, items: Vec<Value>, cursor: Value) -> Value {
    json!({"view_revision":revision,"items":items,"next_cursor":cursor})
}
fn item(id: usize) -> Value {
    json!({"id":format!("org.example.p{id}"),"version":null,"origin":null,"credential_slots":[],"needs_credentials":null})
}

#[test]
fn invalid_pages_stop_until_explicit_refresh_without_offering_a_partial_tail() {
    for bad in [
        page("1", (0..33).map(item).collect(), Value::Null),
        page("1", vec![item(0), item(0)], Value::Null),
        page("1", vec![], json!("next")),
        json!({"view_revision":"1","items":[],"next_cursor":42}),
    ] {
        let mut d = Dependencies::default();
        assert!(d.accept(bad).is_err());
        assert!(d.items().is_none());
        assert!(d.failed());
        assert!(d.request().is_none());
    }
    let mut d = Dependencies::default();
    d.accept(page("1", vec![item(0)], json!("same"))).unwrap();
    assert!(d.accept(page("1", vec![item(1)], json!("same"))).is_err());
    assert!(d.items().is_none());
}

#[test]
fn valid_large_plugin_settings_keep_every_dependency_and_tail_offer() {
    // Each `"p00000":""` settings entry occupies 12 bytes including its comma:
    // even 24,000 entries leave over 700 KiB for the otherwise empty document.
    // Empty HEX settings are valid PluginSettings, do not allocate identities,
    // and validation has no settings-entry cap (only the 1 MiB document cap).
    for count in [10_001, 24_000] {
        let mut expected: Vec<_> = (0..count)
            .map(|i| {
                json!({"id":format!("p{i:05}"),"version":null,"origin":null,
                "credential_slots":[],"needs_credentials":null})
            })
            .collect();
        expected[count - 1]["origin"] = json!("known/installable-plugin");
        expected[count - 1]["version"] = json!({"major":1,"minor":2,"patch":3});
        let mut d = Dependencies::default();
        for (index, chunk) in expected.chunks(32).enumerate() {
            let end = ((index + 1) * 32).min(count);
            let cursor = if end < count {
                json!(format!("next-{end}"))
            } else {
                Value::Null
            };
            assert_eq!(
                d.request().unwrap().operation().as_str(),
                "plugin_dependencies"
            );
            d.accept(page("1", chunk.to_vec(), cursor)).unwrap();
            if end < count {
                assert!(d.items().is_none());
            }
        }
        assert!(!d.failed());
        assert_eq!(d.items().unwrap(), expected);
        assert!(
            d.request().is_none(),
            "completion must not install the tail offer"
        );
    }
}
