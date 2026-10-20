// First five tests promoted from independent core review; original hash in fix-recovery/reviewer-source.sha256.
use rad_core::{
    configuration::{Configuration, Edit, Origin},
    identity::IdentityRegistry,
};
use rad_store::{CommitOutcome, Store, StoreError};
fn fixture(
    label: &str,
) -> (
    std::path::PathBuf,
    rad_core::configuration::Candidate,
    Store,
) {
    let p = std::env::temp_dir().join(format!(
        "rad-store-review-{label}-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let c = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let s = Store::create(&p, &c).unwrap();
    (p, c, s)
}
fn next(c: &rad_core::configuration::Candidate) -> rad_core::configuration::Candidate {
    c.configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "review".into(),
            },
        )
        .unwrap()
}
#[test]
fn recovery_commit_rotates_valid_backup_and_keeps_retired_identity() {
    let (p, c, mut s) = fixture("recover");
    let n = next(&c);
    assert!(matches!(s.commit(&n), CommitOutcome::Applied { .. }));
    drop(s);
    let db = rusqlite::Connection::open(&p).unwrap();
    db.execute("UPDATE revisions SET document=x'00' WHERE id=2", [])
        .unwrap();
    drop(db);
    let mut s = Store::open(&p).unwrap();
    let l = s.load().unwrap();
    assert!(l.recovered_previous);
    let fixed = l
        .configuration
        .prepare_edit(
            &l.registry,
            Origin::Editor,
            Edit::CreateButton {
                folder: l.configuration.data().principal,
                position: 0,
                name: "new".into(),
            },
        )
        .unwrap();
    assert_eq!(
        fixed.configuration().data().buttons[0]
            .binding
            .short()
            .number(),
        2
    );
    assert_eq!(
        s.commit(&fixed),
        CommitOutcome::Applied {
            revision: 3,
            changed: true
        }
    );
    drop(s);
    let db = rusqlite::Connection::open(&p).unwrap();
    assert_eq!(
        db.query_row::<i64, _, _>("SELECT previous FROM heads", [], |r| r.get(0))
            .unwrap(),
        1
    );
    db.execute("UPDATE revisions SET document=x'00' WHERE id=3", [])
        .unwrap();
    drop(db);
    let s = Store::open(&p).unwrap();
    let l = s.load().unwrap();
    assert_eq!(l.revision, 1);
    assert_eq!(l.registry.records().count(), 3);
    drop(s);
    std::fs::remove_file(p).unwrap();
}
#[test]
fn revision_overflow_is_not_applied_and_does_not_consume_ids() {
    let (p, c, mut s) = fixture("overflow");
    let db = rusqlite::Connection::open(&p).unwrap();
    db.execute("INSERT INTO revisions SELECT 9223372036854775807,document,digest FROM revisions WHERE id=1",[]).unwrap();
    drop(db);
    assert_eq!(
        s.commit(&next(&c)),
        CommitOutcome::NotApplied(StoreError::Capacity)
    );
    assert_eq!(s.load().unwrap().registry.records().count(), 1);
    assert_eq!(s.load().unwrap().revision, 1);
    drop(s);
    std::fs::remove_file(p).unwrap();
}
#[test]
fn invalid_gc_roots_rollback_and_root_limit_does_not_poison_store() {
    let (p, c, mut s) = fixture("gc");
    assert!(matches!(s.commit(&next(&c)), CommitOutcome::Applied { .. }));
    assert_eq!(s.collect(&[1, 999]), Err(StoreError::Invalid));
    assert!(s.revision(1).is_ok());
    assert_eq!(s.collect(&[1; 65]), Err(StoreError::Capacity));
    assert_eq!(s.load().unwrap().revision, 2);
    s.collect(&[1; 64]).unwrap();
    s.collect(&[]).unwrap();
    assert!(s.revision(1).is_ok());
    drop(s);
    std::fs::remove_file(p).unwrap();
}
#[test]
fn recovery_mode_refuses_gc_until_explicit_recovery_resolution() {
    let (p, c, mut s) = fixture("recovery-gc");
    assert!(matches!(s.commit(&next(&c)), CommitOutcome::Applied { .. }));
    drop(s);
    let db = rusqlite::Connection::open(&p).unwrap();
    db.execute("UPDATE revisions SET document=x'00' WHERE id=2", [])
        .unwrap();
    drop(db);
    let mut s = Store::open(&p).unwrap();
    assert!(s.load().unwrap().recovered_previous);
    let outcome = s.collect(&[]);
    drop(s);
    std::fs::remove_file(p).unwrap();
    assert!(
        outcome.is_err(),
        "GC is prohibited while recovered_previous/recovery remains unresolved; got {outcome:?}"
    );
}
#[test]
fn corrupt_revision_material_is_preserved_until_explicit_disposition() {
    let (p, c, mut s) = fixture("material");
    assert!(matches!(s.commit(&next(&c)), CommitOutcome::Applied { .. }));
    drop(s);
    let db = rusqlite::Connection::open(&p).unwrap();
    db.execute("UPDATE revisions SET document=x'00' WHERE id=2", [])
        .unwrap();
    drop(db);
    let mut s = Store::open(&p).unwrap();
    let l = s.load().unwrap();
    assert!(l.recovered_previous);
    let n = l
        .configuration
        .prepare_edits(&l.registry, Origin::Editor, &[])
        .unwrap();
    assert!(matches!(
        s.commit(&n),
        CommitOutcome::Applied { revision: 3, .. }
    ));
    s.collect(&[]).unwrap();
    drop(s);
    let db = rusqlite::Connection::open(&p).unwrap();
    let retained = db
        .query_row::<bool, _, _>(
            "SELECT EXISTS(SELECT 1 FROM revisions WHERE id=2)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(db);
    std::fs::remove_file(p).unwrap();
    assert!(
        retained,
        "Corrupt revision was removed without an explicit disposition/recovery API"
    );
}

#[test]
fn durable_hold_survives_restart_and_requires_current_explicit_disposition() {
    let (p, c, mut s) = fixture("durable-hold");
    assert!(matches!(s.commit(&next(&c)), CommitOutcome::Applied { .. }));
    drop(s);
    let db = rusqlite::Connection::open(&p).unwrap();
    db.execute("UPDATE revisions SET document=x'00' WHERE id=2", [])
        .unwrap();
    drop(db);
    let mut s = Store::open(&p).unwrap();
    let early = s.recovery_hold().unwrap().expect("durable diagnostic hold");
    assert!(matches!(
        s.discard_recovery(early),
        CommitOutcome::NotApplied(_)
    ));
    let stale = s.recovery_hold().unwrap().unwrap();
    let l = s.load().unwrap();
    let n = l
        .configuration
        .prepare_edits(&l.registry, Origin::Editor, &[])
        .unwrap();
    assert!(matches!(
        s.commit(&n),
        CommitOutcome::Applied { revision: 3, .. }
    ));
    assert!(matches!(
        s.discard_recovery(stale),
        CommitOutcome::NotApplied(_)
    ));
    drop(s);
    let mut s = Store::open(&p).unwrap();
    s.collect(&[]).unwrap();
    let db = rusqlite::Connection::open(&p).unwrap();
    assert!(
        db.query_row::<bool, _, _>(
            "SELECT EXISTS(SELECT 1 FROM revisions WHERE id=2)",
            [],
            |r| r.get(0)
        )
        .unwrap()
    );
    let hold = s.recovery_hold().unwrap().unwrap();
    assert_eq!(
        s.discard_recovery(hold),
        CommitOutcome::Applied {
            revision: 3,
            changed: false
        }
    );
    drop(s);
    let mut s = Store::open(&p).unwrap();
    assert!(s.recovery_hold().unwrap().is_none());
    s.collect(&[]).unwrap();
    assert!(
        !db.query_row::<bool, _, _>(
            "SELECT EXISTS(SELECT 1 FROM revisions WHERE id=2)",
            [],
            |r| r.get(0)
        )
        .unwrap()
    );
    assert_eq!(s.load().unwrap().registry.records().count(), 2);
    drop(db);
    drop(s);
    std::fs::remove_file(p).unwrap();
}

#[test]
fn recovery_quarantines_corrupt_media_until_explicit_discard() {
    let (p, c, mut s) = fixture("old-schema-media");
    assert!(matches!(s.commit(&next(&c)), CommitOutcome::Applied { .. }));
    drop(s);
    let db = rusqlite::Connection::open(&p).unwrap();
    db.execute_batch(
        "UPDATE revisions SET document=x'00' WHERE id=2;
        INSERT INTO media VALUES(zeroblob(32),1,1,x'00');
        INSERT INTO revision_media VALUES(2,zeroblob(32));",
    )
    .unwrap();
    let mut s = Store::open(&p).unwrap();
    let l = s.load().unwrap();
    let n = l
        .configuration
        .prepare_edits(&l.registry, Origin::Editor, &[])
        .unwrap();
    assert!(matches!(s.commit(&n), CommitOutcome::Applied { .. }));
    drop(s);
    let mut s = Store::open(&p).unwrap();
    s.collect(&[]).unwrap();
    assert_eq!(
        db.query_row::<i64, _, _>("SELECT count(*) FROM media", [], |r| r.get(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row::<i64, _, _>(
            "SELECT count(*) FROM revision_media WHERE revision=2",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        1
    );
    let hold = s.recovery_hold().unwrap().unwrap();
    assert!(matches!(
        s.discard_recovery(hold),
        CommitOutcome::Applied { changed: false, .. }
    ));
    s.collect(&[]).unwrap();
    assert_eq!(
        db.query_row::<i64, _, _>("SELECT count(*) FROM media", [], |r| r.get(0))
            .unwrap(),
        0
    );
    drop(s);
    drop(db);
    std::fs::remove_file(p).unwrap();
}

#[test]
fn missing_existing_recovery_record_is_not_reinitialized_as_authorized() {
    let (p, _, s) = fixture("missing-hold");
    drop(s);
    let db = rusqlite::Connection::open(&p).unwrap();
    db.execute("DELETE FROM recovery", []).unwrap();
    let opened = Store::open(&p).is_ok();
    drop(db);
    std::fs::remove_file(p).unwrap();
    assert!(
        !opened,
        "Missing existing retention record must not become an implicit discard"
    );
}
