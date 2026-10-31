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
