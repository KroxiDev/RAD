use rad_editor::catalog::Catalog;
use serde_json::json;
#[test]
fn catalog_works_without_action_identity_and_uses_desktop_ids_only() {
    let mut c = Catalog::default();
    assert_eq!(
        c.request(false).arguments(),
        json!({"query":"","cursor":null})
    );
    c.accept(&rad_api::ResponseData::Launcher(serde_json::from_value(json!({"items":[{"desktop_entry":"browser.desktop","name":"Browser"}],"next_cursor":"next"})).unwrap())).unwrap();
    assert_eq!(c.entries[0].desktop_entry, "browser.desktop");
    assert_eq!(
        c.request(true).arguments(),
        json!({"query":"","cursor":"next"})
    );
    c.query = "term".into();
    assert_eq!(
        c.request(false).arguments(),
        json!({"query":"term","cursor":null})
    );
}
