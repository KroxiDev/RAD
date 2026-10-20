use super::*;
fn temp_path(label: &str) -> std::path::PathBuf {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "rad-store-{label}-{}-{time}.sqlite3",
        std::process::id()
    ))
}
#[test]
fn real_database_uses_required_durability_and_safety_settings() {
    let path = temp_path("settings");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    drop(file);
    let connection = Connection::open(&path).unwrap();
    configure(&connection).unwrap();
    let journal: String = connection
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(journal, "delete");
    for (pragma, expected) in [
        ("PRAGMA synchronous", 3),
        ("PRAGMA foreign_keys", 1),
        ("PRAGMA trusted_schema", 0),
    ] {
        let value: i64 = connection.query_row(pragma, [], |r| r.get(0)).unwrap();
        assert_eq!(value, expected, "{pragma}");
    }
    drop(connection);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn schema_preserves_full_width_identities_and_both_unique_keys() {
    let connection = Connection::open_in_memory().unwrap();
    initialize_schema(&connection).unwrap();
    let number = u64::MAX.to_be_bytes();
    let installation = u128::MAX.to_be_bytes();
    connection
        .execute(
            "INSERT INTO identities(kind,number,installation,serial) VALUES (0,?1,?2,?1)",
            rusqlite::params![number.as_slice(), installation.as_slice()],
        )
        .unwrap();
    let restored: (Vec<u8>, Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT number,installation,serial FROM identities",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        restored,
        (number.to_vec(), installation.to_vec(), number.to_vec())
    );
    assert!(
        connection
            .execute(
                "INSERT INTO identities VALUES (0,?1,?2,?3)",
                rusqlite::params![
                    number.as_slice(),
                    installation.as_slice(),
                    1u64.to_be_bytes().as_slice()
                ]
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "INSERT INTO identities VALUES (1,?1,?2,?1)",
                rusqlite::params![number.as_slice(), installation.as_slice()]
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "INSERT INTO identities VALUES (0,1,?1,?2)",
                rusqlite::params![installation.as_slice(), 1u64.to_be_bytes().as_slice()]
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "INSERT INTO identities VALUES (2,?1,?2,?3)",
                rusqlite::params![
                    0u64.to_be_bytes().as_slice(),
                    installation.as_slice(),
                    1u64.to_be_bytes().as_slice()
                ]
            )
            .is_err()
    );
}

#[test]
fn connection_rejects_oversized_rows_and_has_bounded_wait_and_database() {
    let path = temp_path("limits");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    drop(file);
    let connection = Connection::open(&path).unwrap();
    configure(&connection).unwrap();
    assert_eq!(
        connection
            .limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH)
            .unwrap(),
        2 * 1024 * 1024
    );
    let wait: i64 = connection
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .unwrap();
    assert_eq!(wait, 0);
    let pages: i64 = connection
        .query_row("PRAGMA max_page_count", [], |r| r.get(0))
        .unwrap();
    assert_eq!(pages, 262144);
    connection
        .execute_batch("CREATE TABLE payload(data BLOB) STRICT;")
        .unwrap();
    assert!(
        connection
            .execute("INSERT INTO payload VALUES (zeroblob(2097153))", [])
            .is_err()
    );
    drop(connection);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn sqlite_transaction_rolls_back_the_entire_revision_tuple_on_fk_error() {
    let path = temp_path("atomic");
    let mut connection = Connection::open(&path).unwrap();
    configure(&connection).unwrap();
    initialize_schema(&connection).unwrap();
    {
        let tx = connection.transaction().unwrap();
        tx.execute(
            "INSERT INTO identities VALUES (0,?1,?2,?1)",
            rusqlite::params![
                1u64.to_be_bytes().as_slice(),
                1u128.to_be_bytes().as_slice()
            ],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO revisions(id,document,digest) VALUES (1,x'7b7d',zeroblob(32))",
            [],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO media(digest,width,height,bytes) VALUES (zeroblob(32),1,1,x'01')",
            [],
        )
        .unwrap();
        tx.execute("INSERT INTO revision_media VALUES (1,zeroblob(32))", [])
            .unwrap();
        tx.execute("INSERT INTO heads VALUES (1,1,NULL)", [])
            .unwrap();
        // A dangling reference must abort publication; explicit rollback is checked.
        assert!(
            tx.execute("INSERT INTO revision_media VALUES (2,zeroblob(32))", [])
                .is_err()
        );
        tx.rollback().unwrap();
    }
    drop(connection);
    let connection = Connection::open(&path).unwrap();
    for table in [
        "identities",
        "revisions",
        "media",
        "revision_media",
        "heads",
    ] {
        let count: i64 = connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    drop(connection);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn real_sqlite_full_keeps_pretransaction_ledger_after_reopen() {
    let path = temp_path("full");
    let mut connection = Connection::open(&path).unwrap();
    configure(&connection).unwrap();
    initialize_schema(&connection).unwrap();
    connection
        .execute(
            "INSERT INTO identities VALUES (0,?1,?2,?1)",
            rusqlite::params![
                1u64.to_be_bytes().as_slice(),
                1u128.to_be_bytes().as_slice()
            ],
        )
        .unwrap();
    let pages: i64 = connection
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    connection
        .pragma_update(None, "max_page_count", pages)
        .unwrap();
    let tx = connection.transaction().unwrap();
    let mut full = false;
    for n in 2u64..10000 {
        match tx.execute(
            "INSERT INTO identities VALUES (0,?1,?2,?1)",
            rusqlite::params![n.to_be_bytes().as_slice(), 1u128.to_be_bytes().as_slice()],
        ) {
            Ok(_) => (),
            Err(e) => {
                assert_eq!(e.sqlite_error_code(), Some(rusqlite::ErrorCode::DiskFull));
                full = true;
                break;
            }
        }
    }
    assert!(full);
    // FULL may already have rolled back the transaction. Never infer a Store verdict here.
    if !tx.is_autocommit() {
        tx.rollback().unwrap();
    } else {
        drop(tx);
    }
    drop(connection);
    let connection = Connection::open(&path).unwrap();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM identities", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let result: String = connection
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(result, "ok");
    drop(connection);
    std::fs::remove_file(path).unwrap();
}
