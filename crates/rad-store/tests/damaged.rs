//! Reading counters of an unreadable database and replacing it without erasing it.
use rad_core::{
    configuration::{ActionKind, Candidate, Configuration, DesktopEntry, Edit, Origin},
    identity::{IdentityCounters, IdentityRegistry},
};
use rad_store::{PeekedCounters, ReplaceError, Store, StoreError, peek_counters, replace_damaged};
use rusqlite::Connection;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const INSTALLATION: u128 = 0x0123_4567_89ab_cdef_0011_2233_4455_6677;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self::named("plain")
    }
    /// `name` is a directory component, so tests can exercise awkward paths.
    fn named(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "rad-damaged-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn db(&self) -> PathBuf {
        self.0.join("state.sqlite3")
    }
    fn side(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn entries(&self) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let root = self.0.parent().unwrap().to_owned();
        let _ = fs::remove_dir_all(root);
    }
}

fn initial() -> Candidate {
    Configuration::prepare_new(&IdentityRegistry::new(INSTALLATION).unwrap()).unwrap()
}
fn apply(base: &Candidate, edits: &[Edit]) -> Candidate {
    base.configuration()
        .prepare_edits(base.registry(), Origin::Editor, edits)
        .unwrap()
}
fn launcher() -> ActionKind {
    ActionKind::Launcher(DesktopEntry::new("app.desktop".into()).unwrap())
}
/// A store with folders, buttons and actions assigned over several commits.
fn populated(path: &Path) -> IdentityCounters {
    let base = initial();
    let mut store = Store::create(path, &base).unwrap();
    let principal = base.configuration().data().principal;
    let with_tree = apply(
        &base,
        &[
            Edit::CreateFolder {
                parent: principal,
                position: 0,
                name: "sub".into(),
            },
            Edit::CreateButton {
                folder: principal,
                position: 1,
                name: "one".into(),
            },
            Edit::CreateButton {
                folder: principal,
                position: 2,
                name: "two".into(),
            },
        ],
    );
    store.commit(&with_tree);
    let button = with_tree
        .configuration()
        .data()
        .buttons
        .iter()
        .find(|b| b.name == "one")
        .unwrap()
        .binding
        .identity();
    let with_actions = apply(
        &with_tree,
        &[
            Edit::AddAction {
                button,
                kind: launcher(),
            },
            Edit::AddAction {
                button,
                kind: launcher(),
            },
        ],
    );
    store.commit(&with_actions);
    store.load().unwrap().registry.counters()
}
fn sql(path: &Path, statement: &str) {
    Connection::open(path)
        .unwrap()
        .execute_batch(statement)
        .unwrap();
}

#[test]
fn peek_of_a_valid_store_equals_its_registry() {
    let fixture = Fixture::new();
    let counters = populated(&fixture.db());
    assert!(counters.folders >= 2 && counters.buttons >= 2 && counters.actions >= 2);
    assert_eq!(
        peek_counters(&fixture.db()),
        Some(PeekedCounters {
            installation: Some(INSTALLATION),
            counters
        })
    );
}

#[test]
fn peek_ignores_a_broken_counters_digest() {
    let fixture = Fixture::new();
    let counters = populated(&fixture.db());
    sql(
        &fixture.db(),
        "UPDATE counters SET digest=zeroblob(32) WHERE singleton=1",
    );
    assert!(Store::open(&fixture.db()).is_err());
    assert_eq!(peek_counters(&fixture.db()).unwrap().counters, counters);
}

#[test]
fn peek_without_meta_takes_the_maximum_of_counters_and_identities() {
    let fixture = Fixture::new();
    let counters = populated(&fixture.db());
    sql(&fixture.db(), "DELETE FROM meta");
    let peeked = peek_counters(&fixture.db()).unwrap();
    assert_eq!(peeked.installation, None);
    assert_eq!(peeked.counters.folders, counters.folders);
    // Without meta the ledger cannot say which serials are local.
    assert_eq!(peeked.counters.serial, counters.serial);

    // A `counters` row below what `identities` says: the maximum wins.
    sql(
        &fixture.db(),
        "UPDATE counters SET buttons=zeroblob(8), actions=x'0000000000000001'",
    );
    let lowered = peek_counters(&fixture.db()).unwrap();
    assert_eq!(lowered.counters.buttons, counters.buttons);
    assert_eq!(lowered.counters.actions, counters.actions);
}

#[test]
fn peek_uses_identities_serial_only_for_the_installation_in_meta() {
    let fixture = Fixture::new();
    let counters = populated(&fixture.db());
    sql(
        &fixture.db(),
        "UPDATE counters SET serial=zeroblob(8) WHERE singleton=1",
    );
    let peeked = peek_counters(&fixture.db()).unwrap();
    assert_eq!(peeked.installation, Some(INSTALLATION));
    assert_eq!(peeked.counters.serial, counters.serial);
    sql(&fixture.db(), "DELETE FROM meta");
    assert_eq!(peek_counters(&fixture.db()).unwrap().counters.serial, 0);
}

#[test]
fn peek_ignores_a_counter_field_of_the_wrong_length() {
    let fixture = Fixture::new();
    let counters = populated(&fixture.db());
    // Strict tables forbid bad lengths in `counters`; rebuild it without the checks.
    sql(
        &fixture.db(),
        "DROP TABLE counters; CREATE TABLE counters(singleton INTEGER, folders BLOB, buttons BLOB, actions BLOB, serial BLOB, digest BLOB);
         INSERT INTO counters VALUES(1, x'01', x'0000000000000009', x'0000000000000009', x'0000000000000009', x'00');",
    );
    let peeked = peek_counters(&fixture.db()).unwrap();
    assert_eq!(peeked.counters.folders, counters.folders);
    assert_eq!(peeked.counters.buttons, counters.buttons.max(9));
}

#[test]
fn peek_leaves_the_database_journal_and_directory_untouched() {
    let fixture = Fixture::new();
    populated(&fixture.db());
    let journal = fixture.side("state.sqlite3-journal");
    fs::write(
        &journal,
        [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7, 1, 2, 3],
    )
    .unwrap();
    let (db_before, journal_before, entries) = (
        fs::read(fixture.db()).unwrap(),
        fs::read(&journal).unwrap(),
        fixture.entries(),
    );
    assert!(peek_counters(&fixture.db()).is_some());
    assert_eq!(fs::read(fixture.db()).unwrap(), db_before);
    assert_eq!(fs::read(&journal).unwrap(), journal_before);
    assert_eq!(fixture.entries(), entries);
}

#[test]
fn peek_of_a_missing_or_foreign_file_is_none() {
    let fixture = Fixture::new();
    assert_eq!(peek_counters(&fixture.db()), None);
    fs::write(
        fixture.db(),
        b"this is not a database, only text".repeat(200),
    )
    .unwrap();
    assert_eq!(peek_counters(&fixture.db()), None);
    let empty = fixture.side("empty.sqlite3");
    fs::write(&empty, b"").unwrap();
    assert_eq!(peek_counters(&empty), None);
}

#[test]
fn peek_percent_encodes_awkward_paths() {
    let fixture = Fixture::named("a b%41#c?d é");
    let counters = populated(&fixture.db());
    assert_eq!(peek_counters(&fixture.db()).unwrap().counters, counters);
}

/// A real hot journal: a second database whose transaction spilled to its rollback
/// journal and never committed. Returns the database bytes and the journal bytes
/// captured while the transaction was still open.
fn hot_journal(scratch: &Fixture) -> (Vec<u8>, Vec<u8>) {
    let path = scratch.side("hot.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=DELETE; PRAGMA cache_size=1;
             CREATE TABLE t(id INTEGER PRIMARY KEY, v BLOB);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<400)
             INSERT INTO t SELECT i, zeroblob(2000) FROM n;
             BEGIN; UPDATE t SET v=randomblob(2000);",
        )
        .unwrap();
    let journal = fs::read(scratch.side("hot.sqlite3-journal")).unwrap();
    assert!(journal.len() > 4096, "the journal must hold real pages");
    let database = fs::read(&path).unwrap();
    std::mem::forget(connection);
    (database, journal)
}

#[test]
fn replacement_keeps_a_hot_journal_as_evidence_instead_of_applying_it_to_the_new_base() {
    let scratch = Fixture::new();
    let (_, journal) = hot_journal(&scratch);
    let fixture = Fixture::new();
    let damaged = b"garbage that is not a database".repeat(100);
    fs::write(fixture.db(), &damaged).unwrap();
    fs::write(fixture.side("state.sqlite3-journal"), &journal).unwrap();

    let candidate = initial();
    let replaced = replace_damaged(&fixture.db(), 1_700_000_000_000, |temp| {
        Store::create(temp, &candidate)
    })
    .unwrap();

    let store = Store::open(&fixture.db()).unwrap();
    assert_eq!(
        store.load().unwrap().configuration,
        *candidate.configuration()
    );
    assert_eq!(
        replaced.evidence,
        fixture.side("state.sqlite3.damaged-1700000000000")
    );
    assert_eq!(fs::read(&replaced.evidence).unwrap(), damaged);
    assert_eq!(
        fs::read(fixture.side("state.sqlite3.damaged-1700000000000-journal")).unwrap(),
        journal
    );
    assert!(!fixture.side("state.sqlite3-journal").exists());
    assert_eq!(
        fixture.entries(),
        [
            "state.sqlite3",
            "state.sqlite3.damaged-1700000000000",
            "state.sqlite3.damaged-1700000000000-journal"
        ]
    );
}

#[test]
fn reserved_counters_reach_the_new_base() {
    let fixture = Fixture::new();
    let counters = populated(&fixture.db());
    let peeked = peek_counters(&fixture.db()).unwrap();
    let mut registry = IdentityRegistry::new(0xfeed).unwrap();
    registry.reserve_counters(peeked.counters);
    let candidate = Configuration::prepare_new_reserving(&registry).unwrap();
    replace_damaged(&fixture.db(), 5, |temp| Store::create(temp, &candidate)).unwrap();

    let loaded = Store::open(&fixture.db()).unwrap().load().unwrap();
    assert_eq!(loaded.registry.installation(), 0xfeed);
    assert!(loaded.registry.counters().contains(counters));
    let principal = loaded.configuration.data().folders[0].binding;
    assert_eq!(principal.short().number(), counters.folders + 1);
    assert_eq!(principal.identity().serial(), counters.serial + 1);
    assert!(fixture.side("state.sqlite3.damaged-5").exists());
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(fixture.db()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_failing_create_leaves_the_damaged_base_and_no_temporary() {
    let fixture = Fixture::new();
    fs::write(fixture.db(), b"damaged").unwrap();
    let result = replace_damaged(&fixture.db(), 9, |_| Err(StoreError::Capacity));
    assert_eq!(result, Err(ReplaceError::NotApplied(StoreError::Capacity)));
    assert_eq!(fs::read(fixture.db()).unwrap(), b"damaged");
    assert_eq!(fixture.entries(), ["state.sqlite3"]);
}

#[test]
fn a_missing_base_is_not_replaced() {
    let fixture = Fixture::new();
    let candidate = initial();
    let result = replace_damaged(&fixture.db(), 9, |temp| Store::create(temp, &candidate));
    assert_eq!(result, Err(ReplaceError::NotApplied(StoreError::Io)));
    assert!(fixture.entries().is_empty());
}

#[test]
fn existing_evidence_blocks_the_replacement_untouched() {
    let candidate = initial();
    for taken in [
        "state.sqlite3.damaged-9",
        "state.sqlite3.damaged-9-journal",
        "state.sqlite3.damaged-9-wal",
        "state.sqlite3.damaged-9-shm",
        "state.sqlite3.new-9",
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.db(), b"damaged").unwrap();
        fs::write(fixture.side(taken), b"earlier").unwrap();
        let result = replace_damaged(&fixture.db(), 9, |temp| Store::create(temp, &candidate));
        assert_eq!(
            result,
            Err(ReplaceError::NotApplied(StoreError::Io)),
            "{taken}"
        );
        assert_eq!(fs::read(fixture.db()).unwrap(), b"damaged");
        assert_eq!(fs::read(fixture.side(taken)).unwrap(), b"earlier");
        assert_eq!(fixture.entries().len(), 2, "{taken}");
    }
}
