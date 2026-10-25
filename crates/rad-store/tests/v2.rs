use rad_store::Store;
use std::path::PathBuf;
fn path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rad-store-v2-{label}-{}", std::process::id()))
}
#[test]
fn migrates_real_v1_and_preserves_original_bytes() {
    let path = path("migration");
    let original = include_bytes!("../../../docs/fixtures/v1/profile.sqlite3");
    std::fs::write(&path, original).unwrap();
    let store = Store::open(&path).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.revision, 5);
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    let backup = PathBuf::from(format!("{}.v1-backup", path.display()));
    assert_eq!(std::fs::read(backup).unwrap(), original);
}
#[test]
fn future_schema_has_explicit_error_without_touching_file() {
    let path = path("future");
    std::fs::write(
        &path,
        include_bytes!("../../../docs/fixtures/v1/profile.sqlite3"),
    )
    .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA user_version=999").unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    let error = Store::open(&path).err().unwrap();
    assert_eq!(format!("{error:?}"), "UnsupportedVersion");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
#[test]
fn garbage_collection_prunes_retired_bindings_without_recycling_ids() {
    use rad_core::{configuration::*, identity::*};
    let path = path("pruning");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(52).unwrap()).unwrap();
    let mut store = Store::create(&path, &initial).unwrap();
    let added = initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: initial.configuration().data().principal,
                position: 0,
                name: "retire".into(),
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit(&added),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let retired = added.configuration().data().buttons[0].binding;
    let removed = added
        .configuration()
        .prepare_edit(
            added.registry(),
            Origin::Editor,
            Edit::DeleteButton {
                button: retired.identity(),
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit(&removed),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let advanced = removed
        .configuration()
        .prepare_edit(
            removed.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                brightness_percent: 12,
                ..SettingsData::default()
            }),
        )
        .unwrap();
    assert!(matches!(
        store.commit(&advanced),
        rad_store::CommitOutcome::Applied { .. }
    ));
    store.collect(&[2]).unwrap();
    assert!(store.lookup(retired.short()).unwrap().is_some());
    store.collect(&[]).unwrap();
    assert!(store.lookup(retired.short()).unwrap().is_none());
    drop(store);
    let store = Store::open(&path).unwrap();
    let loaded = store.load().unwrap();
    let next = loaded
        .configuration
        .prepare_edit(
            &loaded.registry,
            Origin::Editor,
            Edit::CreateButton {
                folder: loaded.configuration.data().principal,
                position: 0,
                name: "new".into(),
            },
        )
        .unwrap();
    assert!(
        next.configuration().data().buttons[0]
            .binding
            .short()
            .number()
            > retired.short().number()
    );
    assert!(
        next.configuration().data().buttons[0]
            .binding
            .identity()
            .serial()
            > retired.identity().serial()
    );
}
#[test]
fn pruned_binding_is_not_reinserted_by_stale_candidate_registry() {
    use rad_core::{configuration::*, identity::*};
    let path = path("stale-pruning");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(53).unwrap()).unwrap();
    let mut store = Store::create(&path, &initial).unwrap();
    let added = initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: initial.configuration().data().principal,
                position: 0,
                name: "old".into(),
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit(&added),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let retired = added.configuration().data().buttons[0].binding;
    let removed = added
        .configuration()
        .prepare_edit(
            added.registry(),
            Origin::Editor,
            Edit::DeleteButton {
                button: retired.identity(),
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit(&removed),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let advanced = removed
        .configuration()
        .prepare_edit(
            removed.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                brightness_percent: 12,
                ..SettingsData::default()
            }),
        )
        .unwrap();
    assert!(matches!(
        store.commit(&advanced),
        rad_store::CommitOutcome::Applied { .. }
    ));
    store.collect(&[]).unwrap();
    let stale = advanced
        .configuration()
        .prepare_edit(
            advanced.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                brightness_percent: 13,
                ..SettingsData::default()
            }),
        )
        .unwrap();
    assert!(matches!(
        store.commit(&stale),
        rad_store::CommitOutcome::Applied { .. }
    ));
    assert!(store.lookup(retired.short()).unwrap().is_none());
}
#[test]
fn counter_integrity_is_checked_independently_of_revision_recovery() {
    use rad_core::{configuration::Configuration, identity::IdentityRegistry};
    let path = path("counter-integrity");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(54).unwrap()).unwrap();
    drop(Store::create(&path, &initial).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE counters SET folders=zeroblob(8)", [])
        .unwrap();
    drop(db);
    assert!(matches!(
        Store::open(&path),
        Err(rad_store::StoreError::Corrupt)
    ));
}
#[test]
fn unchanged_ledger_never_attempts_duplicate_inserts() {
    use rad_core::{configuration::*, identity::*};
    let path = path("delta");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(55).unwrap()).unwrap();
    let mut store = Store::create(&path, &initial).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_existing BEFORE INSERT ON identities WHEN EXISTS(SELECT 1 FROM identities WHERE kind=NEW.kind AND number=NEW.number) BEGIN SELECT RAISE(ABORT,'duplicate insert'); END;").unwrap();
    drop(db);
    let changed = initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                brightness_percent: 14,
                ..SettingsData::default()
            }),
        )
        .unwrap();
    assert!(matches!(
        store.commit(&changed),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let added = changed
        .configuration()
        .prepare_edit(
            changed.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: changed.configuration().data().principal,
                position: 0,
                name: "delta".into(),
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit(&added),
        rad_store::CommitOutcome::Applied { .. }
    ));
    assert_eq!(store.load().unwrap().registry.records().count(), 2);
}
#[test]
fn v1_migration_keeps_corrupt_evidence_and_recovers_previous() {
    let path = path("corrupt-v1");
    std::fs::write(
        &path,
        include_bytes!("../../../docs/fixtures/v1/profile.sqlite3"),
    )
    .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE revisions SET document=x'00' WHERE id=5", [])
        .unwrap();
    drop(db);
    let original = std::fs::read(&path).unwrap();
    let mut store = Store::open(&path).unwrap();
    let loaded = store.load().unwrap();
    assert!(loaded.recovered_previous);
    assert_eq!(loaded.revision, 4);
    assert!(store.recovery_hold().unwrap().is_some());
    assert!(store.collect(&[]).is_err());
    let backup = PathBuf::from(format!("{}.v1-backup", path.display()));
    assert_eq!(std::fs::read(backup).unwrap(), original);
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("SELECT document FROM revisions WHERE id=5", [], |r| r
            .get::<_, Vec<u8>>(0))
            .unwrap(),
        [0]
    );
}
#[test]
fn future_document_is_not_silently_recovered_as_previous() {
    use sha2::{Digest, Sha256};
    let path = path("future-document");
    std::fs::write(
        &path,
        include_bytes!("../../../docs/fixtures/v1/profile.sqlite3"),
    )
    .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    let original: Vec<u8> = db
        .query_row("SELECT document FROM revisions WHERE id=5", [], |r| {
            r.get(0)
        })
        .unwrap();
    let future = String::from_utf8(original)
        .unwrap()
        .replace("\"version\":1", "\"version\":999");
    db.execute(
        "UPDATE revisions SET document=?1,digest=?2 WHERE id=5",
        rusqlite::params![
            future.as_bytes(),
            Sha256::digest(future.as_bytes()).as_slice()
        ],
    )
    .unwrap();
    drop(db);
    assert!(matches!(
        Store::open(&path),
        Err(rad_store::StoreError::UnsupportedVersion)
    ));
}
#[test]
fn migration_preserves_referenced_media_and_revision_links() {
    use image::ImageEncoder;
    use sha2::{Digest, Sha256};
    let path = path("v1-media");
    std::fs::write(
        &path,
        include_bytes!("../../../docs/fixtures/v1/profile.sqlite3"),
    )
    .unwrap();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&[20, 40, 60, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    let image = rad_media::normalize(&png).unwrap();
    let key = rad_store::media_key(&image);
    let hash = Sha256::digest(image.png_bytes());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "INSERT INTO media VALUES(?1,1,1,?2)",
        rusqlite::params![hash.as_slice(), image.png_bytes()],
    )
    .unwrap();
    for revision in [2, 3, 4, 5] {
        let document: Vec<u8> = db
            .query_row(
                "SELECT document FROM revisions WHERE id=?1",
                [revision],
                |r| r.get(0),
            )
            .unwrap();
        let document = String::from_utf8(document).unwrap().replacen(
            "\"image\":null",
            &format!("\"image\":\"{key}\""),
            1,
        );
        db.execute(
            "UPDATE revisions SET document=?1,digest=?2 WHERE id=?3",
            rusqlite::params![
                document.as_bytes(),
                Sha256::digest(document.as_bytes()).as_slice(),
                revision
            ],
        )
        .unwrap();
        db.execute(
            "INSERT INTO revision_media VALUES(?1,?2)",
            rusqlite::params![revision, hash.as_slice()],
        )
        .unwrap();
    }
    drop(db);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.image(&key).unwrap(), image);
    for revision in [2, 3, 4, 5] {
        assert_eq!(
            store
                .revision(revision)
                .unwrap()
                .configuration
                .data()
                .buttons[0]
                .appearance
                .image
                .as_deref(),
            Some(key.as_str())
        );
    }
}
#[test]
fn adoption_remaps_all_empty_history_and_keeps_undo_redo_available() {
    use rad_core::{configuration::*, identity::*};
    let path = path("adoption");
    let target = Configuration::prepare_new(&IdentityRegistry::new(88).unwrap()).unwrap();
    let mut store = Store::create(&path, &target).unwrap();
    let target = target
        .configuration()
        .prepare_edit(
            target.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                brightness_percent: 23,
                ..SettingsData::default()
            }),
        )
        .unwrap();
    assert!(matches!(
        store.commit(&target),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let source = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let source = source
        .configuration()
        .prepare_edit(
            source.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: source.configuration().data().principal,
                position: 0,
                name: "portable".into(),
            },
        )
        .unwrap();
    let portable = target
        .configuration()
        .prepare_portable_restore(
            target.registry(),
            Origin::Editor,
            true,
            source.configuration().data().clone(),
            77,
            source.registry().counters(),
        )
        .unwrap();
    assert!(matches!(
        store.commit(&portable.candidate),
        rad_store::CommitOutcome::Applied { .. }
    ));
    assert_eq!(
        store.commit(&portable.candidate),
        rad_store::CommitOutcome::Applied {
            revision: 3,
            changed: false
        }
    );
    for revision in [1, 2] {
        let history = store.revision(revision).unwrap();
        assert!(history.configuration.data().buttons.is_empty());
        assert_eq!(
            history.configuration.data().principal,
            portable.history_base.as_ref().unwrap().principal
        );
    }
    assert_eq!(
        store
            .revision(2)
            .unwrap()
            .configuration
            .data()
            .settings
            .brightness_percent,
        23
    );
    let loaded = store.load().unwrap();
    let undo = loaded
        .configuration
        .prepare_restore(
            &loaded.registry,
            Origin::Editor,
            true,
            store.revision(2).unwrap().configuration.data().clone(),
        )
        .unwrap();
    assert!(matches!(
        store.commit(&undo),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let loaded = store.load().unwrap();
    let redo = loaded
        .configuration
        .prepare_restore(
            &loaded.registry,
            Origin::Editor,
            true,
            source.configuration().data().clone(),
        )
        .unwrap();
    assert!(matches!(
        store.commit(&redo),
        rad_store::CommitOutcome::Applied { .. }
    ));
    assert_eq!(store.load().unwrap().configuration, *source.configuration());
    drop(store);
    assert!(Store::open(&path).is_ok());
}
#[test]
fn migrates_real_v1_at_one_mib_with_7200_entities_without_losing_content() {
    use rad_core::configuration::{Edit, Origin, SettingsData};
    let path = path("large-migration");
    let original = include_bytes!("../../../docs/fixtures/v1/large.sqlite3");
    std::fs::write(&path, original).unwrap();
    let mut store = Store::open(&path).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.configuration.data().entity_count(), 7200);
    let expected =
        rad_codec::decode_data(include_bytes!("../../../docs/fixtures/v1/large.json")).unwrap();
    assert_eq!(loaded.configuration.data(), &expected);
    let next = loaded
        .configuration
        .prepare_edit(
            &loaded.registry,
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                brightness_percent: 25,
                ..Default::default()
            }),
        )
        .unwrap();
    assert!(matches!(
        store.commit(&next),
        rad_store::CommitOutcome::Applied { .. }
    ));
    assert_eq!(
        std::fs::read(PathBuf::from(format!("{}.v1-backup", path.display()))).unwrap(),
        original
    );
    drop(store);
    assert_eq!(
        Store::open(&path)
            .unwrap()
            .load()
            .unwrap()
            .configuration
            .data()
            .entity_count(),
        7200
    );
}

#[test]
fn migrates_v2_documents_without_bumping_sqlite_or_losing_original_evidence() {
    let path = path("document-v3");
    let original = include_bytes!("../../../docs/fixtures/v2/profile.sqlite3");
    std::fs::write(&path, original).unwrap();
    let store = Store::open(&path).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(
        loaded.configuration.data().plugin_settings["fixture.plugin"].bytes(),
        &[0, 255, 42]
    );
    let db = rusqlite::Connection::open(&path).unwrap();
    let document: Vec<u8> = db
        .query_row(
            "SELECT document FROM revisions WHERE id=(SELECT current FROM heads)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        String::from_utf8(document)
            .unwrap()
            .contains("\"version\":5")
    );
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        std::fs::read(PathBuf::from(format!(
            "{}.document-v2-backup",
            path.display()
        )))
        .unwrap(),
        original
    );
    drop(store);
    Store::open(&path).unwrap();
}

#[test]
fn v2_and_v3_document_migration_keeps_corrupt_head_and_can_reopen_without_overwriting_backup() {
    use sha2::{Digest, Sha256};
    for (version, document) in [
        (
            2,
            include_bytes!("../../../docs/fixtures/v2/profile.json").as_slice(),
        ),
        (
            3,
            include_bytes!("../../../docs/fixtures/v3/profile.json").as_slice(),
        ),
        (
            4,
            include_bytes!("../../../docs/fixtures/v4/profile.json").as_slice(),
        ),
    ] {
        let path = path(&format!("v{version}-corrupt-document"));
        std::fs::write(
            &path,
            include_bytes!("../../../docs/fixtures/v2/profile.sqlite3"),
        )
        .unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute(
            "UPDATE revisions SET document=?1,digest=?2",
            rusqlite::params![document, Sha256::digest(document).as_slice()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO revisions(id,document,digest) VALUES(2,?1,zeroblob(32))",
            [document],
        )
        .unwrap();
        db.execute("UPDATE heads SET current=2,previous=1", [])
            .unwrap();
        drop(db);
        let before = std::fs::read(&path).unwrap();
        let store = Store::open(&path).unwrap();
        assert!(store.load().unwrap().recovered_previous);
        assert!(store.recovery_hold().unwrap().is_some());
        drop(store);
        let store = Store::open(&path).unwrap();
        assert!(store.load().unwrap().recovered_previous);
        drop(store);
        let db = rusqlite::Connection::open(&path).unwrap();
        let retained: Vec<u8> = db
            .query_row("SELECT document FROM revisions WHERE id=2", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(retained, document);
        let backup = PathBuf::from(format!("{}.document-v{version}-backup", path.display()));
        assert_eq!(std::fs::read(backup).unwrap(), before);
    }
}

#[test]
fn v3_document_migration_preserves_separate_evidence_and_refuses_future_documents() {
    use sha2::{Digest, Sha256};
    let path = path("document-v4");
    std::fs::write(
        &path,
        include_bytes!("../../../docs/fixtures/v2/profile.sqlite3"),
    )
    .unwrap();
    let original = include_bytes!("../../../docs/fixtures/v3/profile.json");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE revisions SET document=?1,digest=?2",
        rusqlite::params![original.as_slice(), Sha256::digest(original).as_slice()],
    )
    .unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    // A former v2 backup must remain untouched when upgrading a v3 installation.
    let old_backup = PathBuf::from(format!("{}.document-v2-backup", path.display()));
    std::fs::write(&old_backup, b"previous migration evidence").unwrap();
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.load().unwrap().configuration.data(),
        &rad_codec::decode_data(original).unwrap()
    );
    drop(store);
    let backup = PathBuf::from(format!("{}.document-v3-backup", path.display()));
    assert_eq!(std::fs::read(&backup).unwrap(), before);
    assert_eq!(
        std::fs::read(&old_backup).unwrap(),
        b"previous migration evidence"
    );
    drop(Store::open(&path).unwrap());
    assert_eq!(std::fs::read(&backup).unwrap(), before);
    let db = rusqlite::Connection::open(&path).unwrap();
    let current: Vec<u8> = db
        .query_row("SELECT document FROM revisions LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rad_codec::document_version(&current).unwrap(), 5);
    let future = String::from_utf8(current)
        .unwrap()
        .replacen("\"version\":5", "\"version\":6", 1);
    db.execute(
        "UPDATE revisions SET document=?1,digest=?2",
        rusqlite::params![
            future.as_bytes(),
            Sha256::digest(future.as_bytes()).as_slice()
        ],
    )
    .unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        Store::open(&path),
        Err(rad_store::StoreError::UnsupportedVersion)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn v4_migration_preserves_source_and_normalizes_only_legacy_low_brightness() {
    use sha2::{Digest, Sha256};
    let path = path("v4-brightness-floor");
    std::fs::write(
        &path,
        include_bytes!("../../../docs/fixtures/v2/profile.sqlite3"),
    )
    .unwrap();
    let original = include_bytes!("../../../docs/fixtures/v4/profile.json");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE revisions SET document=?1,digest=?2",
        rusqlite::params![original.as_slice(), Sha256::digest(original).as_slice()],
    )
    .unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    let old_backup = PathBuf::from(format!("{}.document-v3-backup", path.display()));
    std::fs::write(&old_backup, b"retained previous evidence").unwrap();
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.load().unwrap().configuration.data(),
        &rad_codec::decode_data(original).unwrap()
    );
    assert_eq!(
        store
            .load()
            .unwrap()
            .configuration
            .settings()
            .brightness_percent(),
        10
    );
    drop(store);
    let backup = PathBuf::from(format!("{}.document-v4-backup", path.display()));
    assert_eq!(std::fs::read(&backup).unwrap(), before);
    assert_eq!(
        std::fs::read(&old_backup).unwrap(),
        b"retained previous evidence"
    );
    drop(Store::open(&path).unwrap());
    assert_eq!(std::fs::read(&backup).unwrap(), before);
    let db = rusqlite::Connection::open(&path).unwrap();
    let current: Vec<u8> = db
        .query_row("SELECT document FROM revisions LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rad_codec::document_version(&current).unwrap(), 5);
}
