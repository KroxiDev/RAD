use rad_api::Gesture;
use rad_editor::plugins::{CatalogEntry, plan_gestures};
use serde_json::json;
fn entry(gestures: Vec<Gesture>) -> CatalogEntry {
    CatalogEntry {
        type_key: "fixture:run".into(),
        label: "Run".into(),
        visual: false,
        ready: true,
        gestures,
    }
}
#[test]
fn hold_requires_every_action_to_be_external_and_manifest_compatible() {
    let catalog = [entry(vec![Gesture::Press, Gesture::Hold])];
    let external = json!({"fields":{"type":"fixture:run"}});
    assert_eq!(
        plan_gestures(&[external.clone(), external.clone()], &catalog),
        vec![Gesture::Press, Gesture::Hold]
    );
    assert_eq!(
        plan_gestures(&[external, json!({"fields":{"type":"launcher"}})], &catalog),
        vec![Gesture::Press]
    );
    assert!(plan_gestures(&[json!({"fields":{"type":"missing:run"}})], &catalog).is_empty());
}
#[test]
fn empty_hold_plan_can_always_be_changed_back_to_press() {
    assert_eq!(plan_gestures(&[], &[]), vec![Gesture::Press]);
}
