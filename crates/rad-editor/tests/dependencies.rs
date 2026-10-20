use rad_editor::dependencies::Dependencies;
use serde_json::{Value, json};
fn page(revision: &str, items: Vec<Value>, cursor: Value) -> Value {
    json!({"view_revision":revision,"items":items,"next_cursor":cursor})
}
fn item(id: usize) -> Value {
    json!({"id":format!("org.example.p{id}"),"version":null,"origin":null,"credential_slots":[],"needs_credentials":null})
}
#[test]
fn dependencies_load_every_page_and_survive_editor_restart_without_preview() {
    for _ in 0..2 {
        let mut dependencies = Dependencies::default();
        let first = dependencies
            .request()
            .expect("reconnect must recover dependency offers");
        assert_eq!(first.operation().as_str(), "plugin_dependencies");
        assert_eq!(first.arguments(), json!({"cursor":null}));
        dependencies
            .accept(page("1", (0..32).map(item).collect(), json!("next")))
            .unwrap();
        assert!(
            dependencies.items().is_none(),
            "partial lists must not masquerade as complete"
        );
        assert_eq!(
            dependencies.request().unwrap().arguments(),
            json!({"cursor":"next"})
        );
        dependencies
            .accept(page("1", vec![item(32)], Value::Null))
            .unwrap();
        let items = dependencies.items().unwrap();
        assert_eq!(items.len(), 33);
        assert_eq!(items[32]["id"], "org.example.p32");
        assert!(items[32]["version"].is_null());
        assert!(items[32]["origin"].is_null());
        assert!(dependencies.request().is_none());
    }
}

#[test]
fn revision_change_discards_staged_pages_and_restarts_without_mixing() {
    let mut d = Dependencies::default();
    d.accept(page("1", vec![item(0)], json!("next"))).unwrap();
    assert_eq!(
        d.accept(page("2", vec![item(1)], Value::Null))
            .unwrap_err()
            .code,
        "cursor_expired"
    );
    assert!(d.items().is_none());
    assert_eq!(d.request().unwrap().arguments(), json!({"cursor":null}));
    d.accept(page("2", vec![item(2)], Value::Null)).unwrap();
    assert_eq!(d.items().unwrap(), &[item(2)]);
    d.reset();
    assert!(d.items().is_none());
    assert!(d.request().is_some());
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
fn credential_requirements_only_come_from_runtime_dependency_fields() {
    let mut d = Dependencies::default();
    let installed = json!({"id":"installed","version":null,"origin":null,"credential_slots":["password"],"needs_credentials":true});
    d.accept(page("1", vec![item(0), installed.clone()], Value::Null))
        .unwrap();
    assert!(d.items().unwrap()[0]["needs_credentials"].is_null());
    assert_eq!(d.items().unwrap()[1], installed);
}

#[test]
fn long_portable_origins_preserve_the_complete_dependency_list() {
    for origin in ["x".repeat(4096), "x".repeat(8190), "\"".repeat(4095)] {
        let mut first = item(0);
        first["origin"] = json!(origin);
        let expected = vec![first, item(1)];
        let mut d = Dependencies::default();
        d.accept(page("1", expected.clone(), Value::Null)).unwrap();
        assert_eq!(d.items().unwrap(), expected);
    }
    for origin in ["x".repeat(8191), "\"".repeat(4096)] {
        let mut first = item(0);
        first["origin"] = json!(origin);
        let mut d = Dependencies::default();
        assert!(
            d.accept(page("1", vec![first, item(1)], Value::Null))
                .is_err()
        );
        assert!(d.items().is_none());
    }
}

#[test]
fn portable_plugin_ids_use_the_json_scalar_limit() {
    for id in ["p".repeat(8190), "\"".repeat(4095)] {
        let mut first = item(0);
        first["id"] = json!(id);
        let expected = vec![first, item(1)];
        let mut d = Dependencies::default();
        d.accept(page("1", expected.clone(), Value::Null)).unwrap();
        assert_eq!(d.items().unwrap(), expected);
    }
    for id in ["p".repeat(8191), "\"".repeat(4096), "bad\nid".into()] {
        let mut first = item(0);
        first["id"] = json!(id);
        let mut d = Dependencies::default();
        assert!(d.accept(page("1", vec![first], Value::Null)).is_err());
        assert!(d.items().is_none());
    }
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
