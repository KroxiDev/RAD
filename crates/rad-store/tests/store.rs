use rad_core::{configuration::Configuration, identity::IdentityRegistry};
use rad_store::Store;
fn path(label: &str) -> std::path::PathBuf {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "rad-store-public-{label}-{}-{t}.sqlite3",
        std::process::id()
    ))
}

#[test]
fn commit_rotates_previous_once_and_restore_never_reuses_retired_ids() {
    use rad_core::configuration::{Edit, Origin};
    use rad_store::CommitOutcome;
    let path = path("commit");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap();
    let mut store = Store::create(&path, &initial).unwrap();
    let next = initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: initial.configuration().data().principal,
                position: 0,
                name: "first".into(),
            },
        )
        .unwrap();
    assert_eq!(
        store.commit(&next),
        CommitOutcome::Applied {
            revision: 2,
            changed: true
        }
    );
    let loaded = store.load().unwrap();
    assert_eq!(loaded.configuration, *next.configuration());
    assert_eq!(
        store.commit(&next),
        CommitOutcome::Applied {
            revision: 2,
            changed: false
        }
    );
    let restored = loaded
        .configuration
        .prepare_restore(
            &loaded.registry,
            Origin::Editor,
            true,
            initial.configuration().data().clone(),
        )
        .unwrap();
    assert_eq!(
        store.commit(&restored),
        CommitOutcome::Applied {
            revision: 3,
            changed: true
        }
    );
    drop(store);
    let store = Store::open(&path).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.registry.records().count(), 2);
    assert_eq!(loaded.registry.records().filter(|r| r.retired()).count(), 1);
    let new = loaded
        .configuration
        .prepare_edit(
            &loaded.registry,
            Origin::Editor,
            Edit::CreateButton {
                folder: loaded.configuration.data().principal,
                position: 0,
                name: "second".into(),
            },
        )
        .unwrap();
    assert_eq!(
        new.configuration().data().buttons[0]
            .binding
            .short()
            .number(),
        2
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn corrupt_current_recovers_previous_with_warning_but_never_discards_ledger() {
    use rad_core::configuration::{Edit, Origin};
    let path = path("recovery");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap();
    let mut store = Store::create(&path, &initial).unwrap();
    let next = initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: initial.configuration().data().principal,
                position: 0,
                name: "first".into(),
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit(&next),
        rad_store::CommitOutcome::Applied { .. }
    ));
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE revisions SET document=x'00' WHERE id=2", [])
        .unwrap();
    drop(db);
    let store = Store::open(&path).unwrap();
    let recovered = store.load().unwrap();
    assert!(recovered.recovered_previous);
    assert_eq!(recovered.configuration, *initial.configuration());
    assert_eq!(recovered.registry.records().count(), 2);
    assert_eq!(
        recovered.registry.records().filter(|r| r.retired()).count(),
        1
    );
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("DELETE FROM identities WHERE kind=1", [])
        .unwrap();
    drop(db);
    assert!(Store::open(&path).is_err());
    assert!(Store::create(&path, &initial).is_err());
    assert!(path.exists());
    std::fs::remove_file(path).unwrap();
}

fn image() -> rad_media::NormalizedImage {
    use image::ImageEncoder;
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&[12, 34, 56, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    rad_media::normalize(&bytes).unwrap()
}
#[test]
fn media_is_owned_deduplicated_and_committed_with_configuration() {
    use rad_core::configuration::{Appearance, Edit, Origin};
    let path = path("media");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap();
    let mut store = Store::create(&path, &initial).unwrap();
    let button = initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: initial.configuration().data().principal,
                position: 0,
                name: "picture".into(),
            },
        )
        .unwrap();
    let image = image();
    let key = rad_store::media_key(&image);
    let candidate = button
        .configuration()
        .prepare_edit(
            button.registry(),
            Origin::Editor,
            Edit::SetAppearance {
                button: button.configuration().data().buttons[0].binding.identity(),
                appearance: Appearance {
                    text: "".into(),
                    image: Some(key.clone()),
                    text_position: None,
                },
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit_with_media(&candidate, std::slice::from_ref(&image)),
        rad_store::CommitOutcome::Applied { .. }
    ));
    assert_eq!(store.image(&key).unwrap(), image);
    assert_eq!(
        store.commit_with_media(&candidate, std::slice::from_ref(&image)),
        rad_store::CommitOutcome::Applied {
            revision: 2,
            changed: false
        }
    );
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row::<i64, _, _>("SELECT count(*) FROM media", [], |r| r.get(0))
            .unwrap(),
        1
    );
    drop(db);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.image(&key).unwrap(), image);
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn max_identity_values_roundtrip_and_namespace_corruption_does_not_reset_allocator() {
    use rad_core::{
        configuration::Origin,
        identity::{Binding, EntityIdentity, EntityKind, ShortId},
    };
    let path = path("max");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(u128::MAX).unwrap()).unwrap();
    let mut data = initial.configuration().data().clone();
    let id = EntityIdentity::new(u128::MAX, u64::MAX).unwrap();
    data.principal = id;
    data.folders[0].binding = Binding::new(ShortId::new(EntityKind::Folder, u64::MAX).unwrap(), id);
    let imported = initial
        .configuration()
        .prepare_restore(initial.registry(), Origin::Editor, true, data)
        .unwrap();
    drop(Store::create(&path, &imported).unwrap());
    let store = Store::open(&path).unwrap();
    assert_eq!(store.load().unwrap().registry, *imported.registry());
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE meta SET installation=?1",
        [1u128.to_be_bytes().as_slice()],
    )
    .unwrap();
    drop(db);
    assert!(Store::open(&path).is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn shared_corrupt_media_makes_both_revisions_unrecoverable_without_empty_reset() {
    use rad_core::configuration::{Appearance, Edit, Origin};
    let path = path("shared-corrupt");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(11).unwrap()).unwrap();
    let mut store = Store::create(&path, &initial).unwrap();
    let button = initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: initial.configuration().data().principal,
                position: 0,
                name: "a".into(),
            },
        )
        .unwrap();
    let image = image();
    let key = rad_store::media_key(&image);
    let candidate = button
        .configuration()
        .prepare_edit(
            button.registry(),
            Origin::Editor,
            Edit::SetAppearance {
                button: button.configuration().data().buttons[0].binding.identity(),
                appearance: Appearance {
                    text: "".into(),
                    image: Some(key),
                    text_position: None,
                },
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit_with_media(&candidate, &[image]),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let renamed = candidate
        .configuration()
        .prepare_edit(
            candidate.registry(),
            Origin::Editor,
            Edit::SetName {
                button: candidate.configuration().data().buttons[0]
                    .binding
                    .identity(),
                name: "b".into(),
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit(&renamed),
        rad_store::CommitOutcome::Applied { .. }
    ));
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE media SET bytes=x'01'", []).unwrap();
    drop(db);
    assert!(Store::open(&path).is_err());
    assert!(Store::create(&path, &initial).is_err());
    std::fs::remove_file(path).unwrap();
}
