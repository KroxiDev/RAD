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
    assert_eq!(rad_codec::document_version(&current).unwrap(), 7);
    let future = String::from_utf8(current)
        .unwrap()
        .replacen("\"version\":7", "\"version\":8", 1);
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
    assert_eq!(rad_codec::document_version(&current).unwrap(), 7);
}

#[test]
fn v5_migration_keeps_exact_backup_and_state_faces_start_empty() {
    use rad_core::{configuration::*, identity::*};
    use sha2::{Digest, Sha256};
    let path = path("v5-states");
    let original = include_bytes!("../../../docs/fixtures/v5/profile.json");
    let data = rad_codec::decode_data(original).unwrap();
    let base =
        Configuration::prepare_new(&IdentityRegistry::new(data.principal.installation()).unwrap())
            .unwrap();
    let c = base
        .configuration()
        .prepare_restore(base.registry(), Origin::Editor, true, data)
        .unwrap();
    drop(Store::create(&path, &c).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE revisions SET document=?1,digest=?2",
        rusqlite::params![original.as_slice(), Sha256::digest(original).as_slice()],
    )
    .unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    let store = Store::open(&path).unwrap();
    assert!(
        store
            .load()
            .unwrap()
            .configuration
            .data()
            .buttons
            .iter()
            .all(|b| b.appearance.states.is_empty())
    );
    drop(store);
    let backup = PathBuf::from(format!("{}.document-v5-backup", path.display()));
    assert_eq!(std::fs::read(&backup).unwrap(), before);
    drop(Store::open(&path).unwrap());
    assert_eq!(std::fs::read(backup).unwrap(), before);
}

#[test]
fn v6_migration_copies_position_to_faces_and_preserves_exact_backup() {
    use rad_core::{configuration::*, identity::*};
    use sha2::{Digest, Sha256};
    let path = path("v6-text-style");
    let original = include_bytes!("../../../docs/fixtures/v6/profile.json");
    let data = rad_codec::decode_data(original).unwrap();
    let base =
        Configuration::prepare_new(&IdentityRegistry::new(data.principal.installation()).unwrap())
            .unwrap();
    let c = base
        .configuration()
        .prepare_restore(base.registry(), Origin::Editor, true, data)
        .unwrap();
    drop(Store::create(&path, &c).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE revisions SET document=?1,digest=?2",
        rusqlite::params![original.as_slice(), Sha256::digest(original).as_slice()],
    )
    .unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    let store = Store::open(&path).unwrap();
    let data = store.load().unwrap();
    let encoded = rad_codec::encode(&data.configuration).unwrap();
    assert_eq!(rad_codec::document_version(&encoded).unwrap(), 7);
    for face in data.configuration.data().buttons[0]
        .appearance
        .states
        .values()
    {
        assert_eq!(
            face.text_style.position,
            Some(rad_core::configuration::TextPosition::AboveImage)
        );
    }
    drop(store);
    let backup = PathBuf::from(format!("{}.document-v6-backup", path.display()));
    assert_eq!(std::fs::read(&backup).unwrap(), before);
    drop(Store::open(&path).unwrap());
    assert_eq!(std::fs::read(backup).unwrap(), before);
}

#[test]
fn v7_reopen_preserves_complete_independent_face_styles() {
    use rad_core::{configuration::*, identity::*};
    let initial = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let c = initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: initial.configuration().data().principal,
                position: 0,
                name: "Styles".into(),
            },
        )
        .unwrap();
    let appearance = Appearance {
        text: "Saved while hidden".into(),
        text_style: TextStyle {
            position: Some(TextPosition::Hidden),
            size: 24,
            color: [1, 2, 3],
            band: false,
        },
        states: [
            (
                "on".into(),
                StateFace {
                    text: "On".into(),
                    text_style: TextStyle {
                        position: Some(TextPosition::Top),
                        size: 8,
                        color: [4, 5, 6],
                        band: false,
                    },
                    ..Default::default()
                },
            ),
            (
                "off".into(),
                StateFace {
                    text: "Off".into(),
                    text_style: TextStyle {
                        position: Some(TextPosition::BelowImage),
                        size: 20,
                        color: [7, 8, 9],
                        band: true,
                    },
                    ..Default::default()
                },
            ),
        ]
        .into(),
        ..Default::default()
    };
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetAppearance {
                button: c.configuration().data().buttons[0].binding.identity(),
                appearance,
            },
        )
        .unwrap();
    let path = path("v7-styles");
    drop(Store::create(&path, &c).unwrap());
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.load().unwrap().configuration.data(),
        c.configuration().data()
    );
}
