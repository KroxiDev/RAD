use rad_core::identity::{EntityKind, ShortId};
use rad_store::Store;

#[test]
fn frozen_v2_store_preserves_deck_plugin_settings_and_retired_reservations() {
    let directory =
        std::env::temp_dir().join(format!("rad-v2-store-golden-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("profile.sqlite3");
    std::fs::write(
        &path,
        include_bytes!("../../../docs/fixtures/v2/profile.sqlite3"),
    )
    .unwrap();
    let store = Store::open(&path).unwrap();
    let loaded = store.load().unwrap();
    let expected =
        rad_codec::decode_data(include_bytes!("../../../docs/fixtures/v2/profile.json")).unwrap();
    assert_eq!(loaded.configuration.data(), &expected);
    assert_eq!(loaded.registry.installation(), 77);
    assert_eq!(loaded.registry.counters().buttons, 2);
    assert_eq!(loaded.registry.counters().serial, 4);
    let retired = store
        .lookup(ShortId::new(EntityKind::Button, 2).unwrap())
        .unwrap()
        .unwrap();
    assert!(retired.retired());
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.load().unwrap().configuration.data(), &expected);
    drop(reopened);
    std::fs::remove_dir_all(directory).unwrap();
}
