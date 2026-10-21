use rad_core::{configuration::*, identity::IdentityRegistry};
use rad_store::{CommitOutcome, Store, StoreError};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "rad-settings-persistence-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&base).unwrap();
        Self(base)
    }
    fn db(&self) -> PathBuf {
        self.0.join("rad.sqlite3")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn initial() -> Candidate {
    Configuration::prepare_new(&IdentityRegistry::new(37).unwrap()).unwrap()
}
fn changed(base: &Candidate) -> Candidate {
    base.configuration()
        .prepare_edits(
            base.registry(),
            Origin::Editor,
            &[
                Edit::CreateFolder {
                    parent: base.configuration().data().principal,
                    position: 0,
                    name: "settings and tree".into(),
                },
                Edit::SetSettings(SettingsData {
                    brightness_percent: 33,
                    sleep_ms: 0,
                    press_threshold_ms: u64::MAX,
                    long_press_threshold_ms: 500,
                }),
            ],
        )
        .unwrap()
}
#[test]
fn settings_survive_reopen_and_previous_restore_uses_current_ledger() {
    let fixture = Fixture::new();
    let base = initial();
    let edit = changed(&base);
    let mut store = Store::create(&fixture.db(), &base).unwrap();
    assert_eq!(
        store.commit(&edit),
        CommitOutcome::Applied {
            revision: 2,
            changed: true
        }
    );
    drop(store);
    let mut store = Store::open(&fixture.db()).unwrap();
    let current = store.load().unwrap();
    assert_eq!(current.configuration, *edit.configuration());
    assert_eq!(
        store.revision(1).unwrap().configuration.settings(),
        base.configuration().settings()
    );
    let restored = current
        .configuration
        .prepare_restore(
            &current.registry,
            Origin::Editor,
            true,
            store.revision(1).unwrap().configuration.data().clone(),
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
    let store = Store::open(&fixture.db()).unwrap();
    assert_eq!(store.load().unwrap().configuration, *base.configuration());
    let registry = store.load().unwrap().registry;
    assert_eq!(registry, *restored.registry());
    assert_eq!(
        registry.records().map(|r| r.binding()).collect::<Vec<_>>(),
        edit.registry()
            .records()
            .map(|r| r.binding())
            .collect::<Vec<_>>()
    );
    assert_eq!(registry.records().filter(|r| r.retired()).count(), 5);
    assert_eq!(
        store.revision(2).unwrap().configuration.settings(),
        edit.configuration().settings()
    );
}
#[test]
fn busy_write_keeps_entire_settings_document_until_an_explicit_new_commit() {
    let fixture = Fixture::new();
    let base = initial();
    let edit = changed(&base);
    let mut store = Store::create(&fixture.db(), &base).unwrap();
    let lock = rusqlite::Connection::open(fixture.db()).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert_eq!(
        store.commit(&edit),
        CommitOutcome::NotApplied(StoreError::Busy)
    );
    assert_eq!(store.load().unwrap().configuration, *base.configuration());
    assert_eq!(store.load().unwrap().revision, 1);
    lock.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        store.commit(&edit),
        CommitOutcome::Applied {
            revision: 2,
            changed: true
        }
    );
    assert_eq!(store.load().unwrap().configuration, *edit.configuration());
}
#[test]
fn valid_digest_cannot_make_a_document_without_required_settings_recoverable() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let base = initial();
    drop(Store::create(&fixture.db(), &base).unwrap());
    let original = rad_codec::encode(base.configuration()).unwrap();
    // Remove precisely the Settings object from the canonical document, without
    // needing a second JSON implementation in this integration test.
    let text = String::from_utf8(original).unwrap();
    let start = text.find("\"settings\":{").unwrap();
    let end = start + text[start..].find('}').unwrap() + 1;
    let stripped = if text.as_bytes().get(end) == Some(&b',') {
        format!("{}{}", &text[..start], &text[end + 1..])
    } else {
        assert_eq!(text.as_bytes()[start - 1], b',');
        format!("{}{}", &text[..start - 1], &text[end..])
    };
    let sql = rusqlite::Connection::open(fixture.db()).unwrap();
    assert_eq!(
        sql.query_row("SELECT json_valid(?1)", [&stripped], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    sql.execute(
        "UPDATE revisions SET document=?1,digest=?2 WHERE id=1",
        rusqlite::params![
            stripped.as_bytes(),
            Sha256::digest(stripped.as_bytes()).as_slice()
        ],
    )
    .unwrap();
    drop(sql);
    let before = std::fs::read(fixture.db()).unwrap();
    assert!(Store::open(&fixture.db()).is_err());
    assert_eq!(std::fs::read(fixture.db()).unwrap(), before);
}

#[test]
fn native_sleep_and_its_duplicate_survive_store_reopen_without_changing_settings() {
    let fixture = Fixture::new();
    let base = initial();
    let c = base
        .configuration()
        .prepare_edit(
            base.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: base.configuration().data().principal,
                position: 0,
                name: "Sleep".into(),
            },
        )
        .unwrap();
    let button = c.configuration().data().buttons[0].binding.identity();
    let c = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                Edit::AddAction {
                    button,
                    kind: ActionKind::Sleep,
                },
                Edit::DuplicateButton {
                    button,
                    folder: c.configuration().data().principal,
                    position: 1,
                },
            ],
        )
        .unwrap();
    let mut store = Store::create(&fixture.db(), &base).unwrap();
    assert!(matches!(
        store.commit(&c),
        CommitOutcome::Applied { changed: true, .. }
    ));
    drop(store);
    let store = Store::open(&fixture.db()).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.configuration, *c.configuration());
    assert_eq!(
        loaded.configuration.settings(),
        base.configuration().settings()
    );
    assert_eq!(loaded.configuration.data().buttons.len(), 2);
    let buttons = &loaded.configuration.data().buttons;
    assert_eq!(buttons[0].actions[0].kind, ActionKind::Sleep);
    assert_eq!(buttons[1].actions[0].kind, ActionKind::Sleep);
    assert_ne!(buttons[0].actions[0].binding, buttons[1].actions[0].binding);
}
