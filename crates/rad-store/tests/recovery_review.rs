// First five tests promoted from independent core review; original hash in fix-recovery/reviewer-source.sha256.
use rad_core::{
    configuration::{Configuration, Edit, Origin},
    identity::IdentityRegistry,
};
use rad_store::{CommitOutcome, Store};
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
    let loaded = s.load().unwrap();
    assert_eq!(loaded.registry.records().count(), 1);
    assert_eq!(loaded.registry.counters().buttons, 1);
    assert_eq!(loaded.registry.counters().serial, 2);
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
