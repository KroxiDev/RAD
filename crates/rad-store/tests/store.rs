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
fn create_reopen_validates_configuration_and_preserves_namespace() {
    let path = path("create");
    let candidate = Configuration::prepare_new(&IdentityRegistry::new(u128::MAX).unwrap()).unwrap();
    let store = Store::create(&path, &candidate).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(&loaded.configuration, candidate.configuration());
    assert_eq!(&loaded.registry, candidate.registry());
    assert_eq!(loaded.revision, 1);
    assert!(!loaded.recovered_previous);
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened.load().unwrap().configuration,
        *candidate.configuration()
    );
    drop(reopened);
    std::fs::remove_file(path).unwrap();
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
fn garbage_collection_retains_previous_and_explicit_roots_but_never_deletes_ids() {
    use rad_core::configuration::{Appearance, Edit, Origin};
    let path = path("gc");
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
    let pictured = button
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
        store.commit_with_media(&pictured, &[image]),
        rad_store::CommitOutcome::Applied { revision: 2, .. }
    ));
    let plain = pictured
        .configuration()
        .prepare_edit(
            pictured.registry(),
            Origin::Editor,
            Edit::SetAppearance {
                button: pictured.configuration().data().buttons[0]
                    .binding
                    .identity(),
                appearance: Appearance::default(),
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit(&plain),
        rad_store::CommitOutcome::Applied { revision: 3, .. }
    ));
    store.collect(&[]).unwrap(); // previous is revision2
    assert!(store.image(&key).is_ok());
    let deleted = plain
        .configuration()
        .prepare_edit(
            plain.registry(),
            Origin::Editor,
            Edit::DeleteButton {
                button: plain.configuration().data().buttons[0].binding.identity(),
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit(&deleted),
        rad_store::CommitOutcome::Applied { revision: 4, .. }
    ));
    store.collect(&[2]).unwrap(); // history/execution/export root
    assert!(store.image(&key).is_ok());
    assert_eq!(
        store.revision(2).unwrap().configuration,
        *pictured.configuration()
    );
    store.collect(&[]).unwrap();
    assert!(store.image(&key).is_err());
    assert_eq!(store.load().unwrap().registry.records().count(), 2);
    drop(store);
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
fn absent_incomplete_and_future_databases_are_not_new_installations() {
    let path = path("not-new");
    assert!(Store::open(&path).is_err());
    assert!(!path.exists());
    std::fs::write(&path, []).unwrap();
    assert!(Store::open(&path).is_err());
    let initial = Configuration::prepare_new(&IdentityRegistry::new(5).unwrap()).unwrap();
    assert!(Store::create(&path, &initial).is_err());
    std::fs::remove_file(&path).unwrap();
    drop(Store::create(&path, &initial).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA user_version=2").unwrap();
    drop(db);
    assert!(Store::open(&path).is_err());
    assert!(path.exists());
    std::fs::remove_file(path).unwrap();
}
#[test]
fn lookup_reports_retired_identity_instead_of_free_identifier() {
    use rad_core::{
        configuration::{Edit, Origin},
        identity::{EntityKind, ShortId},
    };
    let path = path("lookup");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(11).unwrap()).unwrap();
    let mut store = Store::create(&path, &initial).unwrap();
    let added = initial
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
    let id = added.configuration().data().buttons[0].binding;
    assert!(matches!(
        store.commit(&added),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let removed = added
        .configuration()
        .prepare_edit(
            added.registry(),
            Origin::Editor,
            Edit::DeleteButton {
                button: id.identity(),
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit(&removed),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let record = store.lookup(id.short()).unwrap().unwrap();
    assert!(record.retired());
    assert_eq!(record.binding(), id);
    assert!(
        store
            .lookup(ShortId::new(EntityKind::Button, 99).unwrap())
            .unwrap()
            .is_none()
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn missing_media_and_controlled_hash_collision_do_not_publish_or_consume_ids() {
    use rad_core::configuration::{Appearance, Edit, Origin};
    use rad_store::{CommitOutcome, StoreError};
    let path = path("collision");
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
                    image: Some(key.clone()),
                    text_position: None,
                },
            },
        )
        .unwrap();
    assert_eq!(
        store.commit(&candidate),
        CommitOutcome::NotApplied(StoreError::Invalid)
    );
    assert_eq!(store.load().unwrap().revision, 1);
    let db = rusqlite::Connection::open(&path).unwrap();
    let hash = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&key[i..i + 2], 16).unwrap())
        .collect::<Vec<_>>();
    // Synthetic collision fixture, not a claim of finding a SHA-256 collision.
    db.execute("INSERT INTO media VALUES(?1,1,1,x'00')", [hash])
        .unwrap();
    drop(db);
    assert_eq!(
        store.commit_with_media(&candidate, &[image]),
        CommitOutcome::NotApplied(StoreError::Corrupt)
    );
    assert_eq!(store.load().unwrap().registry.records().count(), 1);
    assert_eq!(store.load().unwrap().revision, 1);
    drop(store);
    std::fs::remove_file(path).unwrap();
}
fn with_retired(count: u64) -> rad_core::configuration::Candidate {
    use rad_core::{
        configuration::Origin,
        identity::{Binding, EntityIdentity, EntityKind, IdentityRecord, ShortId},
    };
    let initial = Configuration::prepare_new(&IdentityRegistry::new(17).unwrap()).unwrap();
    let records = initial.registry().records().chain((1..count).map(|n| {
        IdentityRecord::new(
            Binding::new(
                ShortId::new(EntityKind::Button, n).unwrap(),
                EntityIdentity::new(17, n + 1).unwrap(),
            ),
            true,
        )
    }));
    let registry = IdentityRegistry::from_records(17, records).unwrap();
    initial
        .configuration()
        .prepare_edits(&registry, Origin::Editor, &[])
        .unwrap()
}
#[test]
fn ledger_capacity_preserves_tombstones_and_still_allows_edits_without_new_ids() {
    use rad_core::configuration::{Edit, Origin};
    use rad_core::permissions::Permissions;
    use rad_store::{CommitOutcome, StoreError};
    let initial = with_retired(10_000);
    let path = path("capacity");
    let start = std::time::Instant::now();
    let mut store = Store::create(&path, &initial).unwrap();
    let created = start.elapsed();
    let start = std::time::Instant::now();
    let loaded = store.load().unwrap();
    let load = start.elapsed();
    assert_eq!(loaded.registry.records().count(), 10_000);
    let edited = loaded
        .configuration
        .prepare_edit(
            &loaded.registry,
            Origin::Editor,
            Edit::SetGlobalPermissions(Permissions::new(false, false, false).unwrap()),
        )
        .unwrap();
    let start = std::time::Instant::now();
    assert!(matches!(
        store.commit(&edited),
        CommitOutcome::Applied { .. }
    ));
    let committed = start.elapsed();
    let added = edited
        .configuration()
        .prepare_edit(
            edited.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: edited.configuration().data().principal,
                position: 0,
                name: "overflow".into(),
            },
        )
        .unwrap();
    assert_eq!(
        store.commit(&added),
        CommitOutcome::NotApplied(StoreError::Capacity)
    );
    assert_eq!(store.load().unwrap().registry.records().count(), 10_000);
    println!(
        "integrated ledger10000 create_us={} load_us={} commit_us={} db_bytes={}",
        created.as_micros(),
        load.as_micros(),
        committed.as_micros(),
        std::fs::metadata(&path).unwrap().len()
    );
    drop(store);
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        Store::create(&path, &with_retired(10_001)),
        Err(StoreError::Capacity)
    ));
    assert!(!path.exists());
}
#[test]
fn reopening_existing_state_uses_a_real_changed_barrier_not_only_a_read() {
    let path = path("open-barrier");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(31).unwrap()).unwrap();
    drop(Store::create(&path, &initial).unwrap());
    drop(Store::open(&path).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row::<i64, _, _>("SELECT barrier FROM meta", [], |r| r.get(0))
            .unwrap(),
        1
    );
    drop(db);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn valid_json_with_matching_digest_still_requires_core_validation() {
    use sha2::{Digest, Sha256};
    let path = path("invalid-tree");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(31).unwrap()).unwrap();
    drop(Store::create(&path, &initial).unwrap());
    let mut document =
        String::from_utf8(rad_codec::encode(initial.configuration()).unwrap()).unwrap();
    let start = document.find("\"slots\":[").unwrap() + 9;
    let end = start + document[start..].find(']').unwrap();
    document.replace_range(start..end, "");
    assert!(rad_codec::decode_data(document.as_bytes()).is_ok());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE revisions SET document=?1,digest=?2",
        rusqlite::params![
            document.as_bytes(),
            Sha256::digest(document.as_bytes()).as_slice()
        ],
    )
    .unwrap();
    drop(db);
    assert!(Store::open(&path).is_err());
    assert!(path.exists());
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
