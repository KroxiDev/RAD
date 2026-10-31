// Promoted from independent core fix-recovery review; original SHA in fix-recovery-2/reviewer-source.sha256.
use rad_core::{
    configuration::{Configuration, Edit, Origin},
    identity::IdentityRegistry,
};
use rad_store::{CommitOutcome, Store};
fn held(label: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "rad-fix-review-{label}-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let c = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let mut s = Store::create(&p, &c).unwrap();
    let n = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "held".into(),
            },
        )
        .unwrap();
    assert!(matches!(s.commit(&n), CommitOutcome::Applied { .. }));
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
    assert!(s.recovery_hold().unwrap().is_some());
    drop(s);
    p
}
#[test]
fn missing_table_must_not_authorize_gc_of_previously_held_material() {
    let p = held("missing-table");
    let db = rusqlite::Connection::open(&p).unwrap();
    // Controlled incomplete-schema fixture, not a claim of attacker resistance.
    db.execute_batch("DROP TABLE recovery").unwrap();
    drop(db);
    let bytes_before = std::fs::read(&p).unwrap();
    let mut opened = Store::open(&p);
    let rejected = opened.is_err();
    if let Ok(s) = &mut opened {
        s.collect(&[]).unwrap();
    }
    drop(opened);
    assert_eq!(
        std::fs::read(&p).unwrap(),
        bytes_before,
        "Rejected open must preserve the database"
    );
    let db = rusqlite::Connection::open(&p).unwrap();
    assert_eq!(
        db.query_row::<i64, _, _>(
            "SELECT count(*) FROM sqlite_schema WHERE name='recovery'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        0
    );
    let retained: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM revisions WHERE id=2)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(db);
    std::fs::remove_file(p).unwrap();
    assert!(rejected, "Incomplete schema must reject open");
    assert!(
        retained,
        "Missing recovery table was treated as legacy, initialized to hold0, and diagnostic revision2 was deleted without explicit decision"
    );
}
