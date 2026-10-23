mod fault_probe {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/performance/fault_probe.rs"
    ));
}
mod vfs;
use super::*;
use rad_core::configuration::{Edit, Origin};
fn path() -> std::path::PathBuf {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "rad-store-fault-{}-{t}.sqlite3",
        std::process::id()
    ))
}
fn next(initial: &Candidate) -> Candidate {
    initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: initial.configuration().data().principal,
                position: 0,
                name: "added".into(),
            },
        )
        .unwrap()
}
#[test]
fn vfs_observes_real_sync_and_extra_directory_sync_but_not_identical_update() {
    let path = path();
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    drop(Store::create(&path, &initial).unwrap());
    let vfs = vfs::Vfs::new();
    let connection =
        Connection::open_with_flags_and_vfs(&path, OpenFlags::SQLITE_OPEN_READ_WRITE, vfs.name())
            .unwrap();
    database::configure(&connection).unwrap();
    let mut store = Store {
        connection,
        uncertain: false,
        pending: None,
        fail_return: false,
    };
    let before = vfs.state.count(vfs::Event::Sync);
    store
        .connection
        .execute("UPDATE meta SET barrier=barrier WHERE singleton=1", [])
        .unwrap();
    assert_eq!(vfs.state.count(vfs::Event::Sync), before);
    assert!(matches!(
        store.commit(&next(&initial)),
        CommitOutcome::Applied { .. }
    ));
    assert!(vfs.state.count(vfs::Event::Sync) > before);
    assert!(
        vfs.state
            .directory_syncs
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0
    );
    drop(store);
    drop(vfs);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn failed_return_after_real_commit_reconciles_by_new_synced_write_without_rotation() {
    let mut profile =
        fault_probe::Probe::begin("uncertain_store", "real_sqlite_vfs_injected_return_failure");
    let path = profile
        .as_ref()
        .map(|p| p.root().join("state.sqlite3"))
        .unwrap_or_else(path);
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    drop(Store::create(&path, &initial).unwrap());
    let vfs = vfs::Vfs::new();
    let connection =
        Connection::open_with_flags_and_vfs(&path, OpenFlags::SQLITE_OPEN_READ_WRITE, vfs.name())
            .unwrap();
    database::configure(&connection).unwrap();
    let mut store = Store {
        connection,
        uncertain: false,
        pending: None,
        fail_return: true,
    };
    let candidate = next(&initial);
    if let Some(p) = &mut profile {
        p.snapshot("before");
    }
    let profile_commit = std::time::Instant::now();
    assert!(matches!(
        store.commit(&candidate),
        CommitOutcome::Uncertain { revision: 2, .. }
    ));
    if let Some(p) = &mut profile {
        p.sample("commit_to_uncertain", profile_commit.elapsed().as_nanos());
        p.snapshot("during");
    }
    let profile_reject = std::time::Instant::now();
    assert_eq!(
        store.commit(&candidate),
        CommitOutcome::NotApplied(StoreError::Uncertain)
    );
    if let Some(p) = &mut profile {
        p.sample(
            "write_rejected_while_uncertain",
            profile_reject.elapsed().as_nanos(),
        );
    }
    let before = vfs.state.count(vfs::Event::Sync);
    let profile_reconcile = std::time::Instant::now();
    assert_eq!(
        store.reconcile(),
        CommitOutcome::Applied {
            revision: 2,
            changed: true
        }
    );
    if let Some(p) = &mut profile {
        p.sample(
            "reconcile_uncertain",
            profile_reconcile.elapsed().as_nanos(),
        );
        p.snapshot("after");
    }
    assert!(vfs.state.count(vfs::Event::Sync) > before);
    let previous: i64 = store
        .connection
        .query_row("SELECT previous FROM heads", [], |r| r.get(0))
        .unwrap();
    assert_eq!(previous, 1);
    assert_eq!(store.load().unwrap().registry.records().count(), 2);
    drop(store);
    drop(vfs);
    if profile.is_none() {
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn vfs_write_and_sync_failures_reconcile_old_or_new_without_partial_ledger() {
    for (event, count) in sweep_counts() {
        for nth in 1..=count {
            for persistent in [false, true] {
                for after in [false, true] {
                    let path = path();
                    let initial =
                        Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
                    drop(Store::create(&path, &initial).unwrap());
                    let vfs = vfs::Vfs::new();
                    let connection = Connection::open_with_flags_and_vfs(
                        &path,
                        OpenFlags::SQLITE_OPEN_READ_WRITE,
                        vfs.name(),
                    )
                    .unwrap();
                    database::configure(&connection).unwrap();
                    let mut store = Store {
                        connection,
                        uncertain: false,
                        pending: None,
                        fail_return: false,
                    };
                    let candidate = next(&initial);
                    vfs.state.arm(event, nth, persistent, after);
                    let outcome = store.commit(&candidate);
                    if matches!(outcome, CommitOutcome::Uncertain { .. }) {
                        assert!(store.load().is_err());
                        assert!(store.collect(&[]).is_err());
                    }
                    vfs.state.disarm();
                    let outcome = if matches!(outcome, CommitOutcome::Uncertain { .. }) {
                        store.reconcile()
                    } else {
                        outcome
                    };
                    let loaded = store.load().unwrap();
                    match outcome {
                        CommitOutcome::Applied { .. } => {
                            assert_eq!(loaded.configuration, *candidate.configuration());
                            assert_eq!(loaded.registry.records().count(), 2);
                        }
                        CommitOutcome::NotApplied(_) => {
                            assert_eq!(loaded.configuration, *initial.configuration());
                            assert_eq!(loaded.registry.records().count(), 1);
                        }
                        CommitOutcome::Uncertain { .. } => {
                            panic!("failed to reconcile cleared real I/O fault")
                        }
                    }
                    drop(store);
                    drop(vfs);
                    let reopened = Store::open(&path).unwrap();
                    assert_eq!(reopened.load().unwrap().configuration, loaded.configuration);
                    drop(reopened);
                    std::fs::remove_file(path).unwrap();
                }
            }
        }
    }
}

#[test]
fn persistent_reaffirmation_failure_stays_uncertain_then_recovers_without_replay() {
    let path = path();
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    drop(Store::create(&path, &initial).unwrap());
    let vfs = vfs::Vfs::new();
    let connection =
        Connection::open_with_flags_and_vfs(&path, OpenFlags::SQLITE_OPEN_READ_WRITE, vfs.name())
            .unwrap();
    database::configure(&connection).unwrap();
    let mut store = Store {
        connection,
        uncertain: false,
        pending: None,
        fail_return: true,
    };
    let candidate = next(&initial);
    assert!(matches!(
        store.commit(&candidate),
        CommitOutcome::Uncertain { .. }
    ));
    vfs.state.arm(vfs::Event::Sync, 1, true, false);
    assert!(matches!(store.reconcile(), CommitOutcome::Uncertain { .. }));
    assert_eq!(
        store.commit(&candidate),
        CommitOutcome::NotApplied(StoreError::Uncertain)
    );
    vfs.state.disarm();
    // Now let reaffirmation take effect but fail the directory-sync return.
    vfs.state.arm(vfs::Event::Delete, 1, false, true);
    assert!(matches!(store.reconcile(), CommitOutcome::Uncertain { .. }));
    vfs.state.disarm();
    assert_eq!(
        store.reconcile(),
        CommitOutcome::Applied {
            revision: 2,
            changed: true
        }
    );
    assert_eq!(store.load().unwrap().registry.records().count(), 2);
    assert_eq!(
        store
            .connection
            .query_row::<i64, _, _>("SELECT count(*) FROM revisions", [], |r| r.get(0))
            .unwrap(),
        2
    );
    drop(store);
    drop(vfs);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn readonly_full_and_busy_errors_preserve_published_tuple() {
    let path = path();
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    drop(Store::create(&path, &initial).unwrap());
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    database::configure(&connection).unwrap();
    let mut readonly = Store {
        connection,
        uncertain: false,
        pending: None,
        fail_return: false,
    };
    assert!(matches!(
        readonly.commit(&next(&initial)),
        CommitOutcome::NotApplied(_)
    ));
    assert_eq!(
        readonly.load().unwrap().configuration,
        *initial.configuration()
    );
    drop(readonly);
    let mut store = Store::open(&path).unwrap();
    let blocker = Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(matches!(
        store.commit(&next(&initial)),
        CommitOutcome::NotApplied(_)
    ));
    blocker.execute_batch("ROLLBACK").unwrap();
    drop(blocker);
    let pages: i64 = store
        .connection
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    store
        .connection
        .pragma_update(None, "max_page_count", pages)
        .unwrap();
    let (candidate, image) = pictured(&initial);
    let outcome = store.commit_with_media(&candidate, &[image]);
    assert!(!matches!(outcome, CommitOutcome::Applied { .. }));
    store
        .connection
        .pragma_update(None, "max_page_count", 262144)
        .unwrap();
    if matches!(outcome, CommitOutcome::Uncertain { .. }) {
        assert!(matches!(store.reconcile(), CommitOutcome::NotApplied(_)));
    }
    assert_eq!(
        store.load().unwrap().configuration,
        *initial.configuration()
    );
    assert_eq!(store.load().unwrap().registry.records().count(), 1);
    drop(store);
    std::fs::remove_file(path).unwrap();
}
fn pictured(initial: &Candidate) -> (Candidate, rad_media::NormalizedImage) {
    use image::ImageEncoder;
    use rad_core::configuration::Appearance;
    let mut pixels = vec![0u8; 256 * 256 * 4];
    let mut seed = 123u32;
    for byte in &mut pixels {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        *byte = seed as u8;
    }
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&pixels, 256, 256, image::ExtendedColorType::Rgba8)
        .unwrap();
    let image = rad_media::normalize(&bytes).unwrap();
    let button = next(initial);
    let candidate = button
        .configuration()
        .prepare_edit(
            button.registry(),
            Origin::Editor,
            Edit::SetAppearance {
                button: button.configuration().data().buttons[0].binding.identity(),
                appearance: Appearance {
                    text: String::new(),
                    image: Some(media_key(&image)),
                    text_position: None,
                },
            },
        )
        .unwrap();
    (candidate, image)
}
pub(super) fn checkpoint(stage: &str) {
    if std::env::var("RAD_STORE_CRASH_STAGE").ok().as_deref() == Some(stage) {
        std::process::exit(77);
    }
}
#[test]
fn crash_commit_child() {
    let Ok(path) = std::env::var("RAD_STORE_CRASH_PATH") else {
        return;
    };
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let mut store = Store::open(Path::new(&path)).unwrap();
    let (candidate, image) = pictured(&initial);
    store.commit_with_media(&candidate, &[image]);
    panic!("requested checkpoint not hit");
}
#[test]
fn process_exit_at_transaction_boundaries_reopens_only_complete_old_or_new_tuple() {
    for stage in ["media", "ledger", "revision", "heads", "committed"] {
        let path = path();
        let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
        drop(Store::create(&path, &initial).unwrap());
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "fault_tests::crash_commit_child", "--nocapture"])
            .env("RAD_STORE_CRASH_PATH", &path)
            .env("RAD_STORE_CRASH_STAGE", stage)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(77));
        let store = Store::open(&path).unwrap();
        let loaded = store.load().unwrap();
        let applied = stage == "committed";
        assert_eq!(
            loaded.registry.records().count(),
            if applied { 2 } else { 1 }
        );
        assert_eq!(
            loaded.configuration.data().buttons.len(),
            if applied { 1 } else { 0 }
        );
        assert_eq!(
            store
                .connection
                .query_row::<i64, _, _>("SELECT count(*) FROM media", [], |r| r.get(0))
                .unwrap(),
            if applied { 1 } else { 0 }
        );
        let previous: Option<i64> = store
            .connection
            .query_row("SELECT previous FROM heads", [], |r| r.get(0))
            .unwrap();
        assert_eq!(previous, if applied { Some(1) } else { None });
        drop(store);
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn gc_rollback_failure_is_uncertain_and_cannot_erase_published_roots() {
    let path = path();
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    drop(Store::create(&path, &initial).unwrap());
    let vfs = vfs::Vfs::new();
    let connection =
        Connection::open_with_flags_and_vfs(&path, OpenFlags::SQLITE_OPEN_READ_WRITE, vfs.name())
            .unwrap();
    database::configure(&connection).unwrap();
    let mut store = Store {
        connection,
        uncertain: false,
        pending: None,
        fail_return: false,
    };
    let (pictured, image) = pictured(&initial);
    assert!(matches!(
        store.commit_with_media(&pictured, &[image]),
        CommitOutcome::Applied { .. }
    ));
    let restored = pictured
        .configuration()
        .prepare_restore(
            pictured.registry(),
            Origin::Editor,
            true,
            initial.configuration().data().clone(),
        )
        .unwrap();
    assert!(matches!(
        store.commit(&restored),
        CommitOutcome::Applied { .. }
    ));
    let next = next(&restored);
    assert!(matches!(store.commit(&next), CommitOutcome::Applied { .. }));
    store
        .connection
        .execute_batch("PRAGMA cache_size=1")
        .unwrap();
    vfs.state.arm(vfs::Event::Write, 1, true, false);
    assert_eq!(store.collect(&[]), Err(StoreError::Uncertain));
    assert!(store.load().is_err());
    vfs.state.disarm();
    assert_eq!(
        store.reconcile(),
        CommitOutcome::NotApplied(StoreError::Sql)
    );
    assert_eq!(store.load().unwrap().configuration, *next.configuration());
    store.collect(&[]).unwrap();
    drop(store);
    drop(vfs);
    std::fs::remove_file(path).unwrap();
}
thread_local! { static FAIL_BEFORE_COMMIT:std::cell::Cell<bool>=const {std::cell::Cell::new(false)}; }
pub(super) fn take_rollback_fault() -> bool {
    FAIL_BEFORE_COMMIT.with(|flag| flag.replace(false))
}
#[test]
fn real_truncate_failure_during_spilled_transaction_rollback_remains_uncertain() {
    let path = path();
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    drop(Store::create(&path, &initial).unwrap());
    let vfs = vfs::Vfs::new();
    let connection =
        Connection::open_with_flags_and_vfs(&path, OpenFlags::SQLITE_OPEN_READ_WRITE, vfs.name())
            .unwrap();
    database::configure(&connection).unwrap();
    let mut store = Store {
        connection,
        uncertain: false,
        pending: None,
        fail_return: false,
    };
    store
        .connection
        .execute_batch("PRAGMA cache_size=1")
        .unwrap();
    let (candidate, image) = pictured(&initial);
    FAIL_BEFORE_COMMIT.with(|f| f.set(true));
    vfs.state.arm(vfs::Event::Truncate, 1, true, false);
    let outcome = store.commit_with_media(&candidate, &[image]);
    assert!(vfs.state.count(vfs::Event::Truncate) > 0);
    assert!(matches!(outcome, CommitOutcome::Uncertain { .. }));
    vfs.state.disarm();
    assert!(matches!(store.reconcile(), CommitOutcome::NotApplied(_)));
    assert_eq!(
        store.load().unwrap().configuration,
        *initial.configuration()
    );
    drop(store);
    drop(vfs);
    std::fs::remove_file(path).unwrap();
}

fn sweep_counts() -> Vec<(vfs::Event, usize)> {
    let path = path();
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    drop(Store::create(&path, &initial).unwrap());
    let vfs = vfs::Vfs::new();
    let connection =
        Connection::open_with_flags_and_vfs(&path, OpenFlags::SQLITE_OPEN_READ_WRITE, vfs.name())
            .unwrap();
    database::configure(&connection).unwrap();
    let mut store = Store {
        connection,
        uncertain: false,
        pending: None,
        fail_return: false,
    };
    vfs.state.arm(vfs::Event::Write, usize::MAX, false, false);
    assert!(matches!(
        store.commit(&next(&initial)),
        CommitOutcome::Applied { .. }
    ));
    let counts = [
        vfs::Event::Write,
        vfs::Event::Sync,
        vfs::Event::Delete,
        vfs::Event::Open,
    ]
    .into_iter()
    .map(|e| (e, vfs.state.count(e)))
    .collect::<Vec<_>>();
    println!(
        "baseline VFS write/sync/delete/open counts: {:?}; each swept N with before/after and once/persistent",
        counts.iter().map(|(_, n)| n).collect::<Vec<_>>()
    );
    drop(store);
    drop(vfs);
    std::fs::remove_file(path).unwrap();
    counts
}
#[test]
fn transient_read_io_error_does_not_masquerade_as_corruption_recovery() {
    let path = path();
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let mut initial_store = Store::create(&path, &initial).unwrap();
    let (candidate, image) = pictured(&initial);
    assert!(matches!(
        initial_store.commit_with_media(&candidate, &[image]),
        CommitOutcome::Applied { .. }
    ));
    drop(initial_store);
    for nth in 1..=80 {
        let vfs = vfs::Vfs::new();
        let connection = Connection::open_with_flags_and_vfs(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            vfs.name(),
        )
        .unwrap();
        database::configure(&connection).unwrap();
        connection.execute_batch("PRAGMA shrink_memory").unwrap();
        let store = Store {
            connection,
            uncertain: false,
            pending: None,
            fail_return: false,
        };
        vfs.state.arm(vfs::Event::Read, nth, false, false);
        if let Ok(loaded) = store.load() {
            assert!(
                !loaded.recovered_previous,
                "read failure {nth} became recovery"
            );
            assert_eq!(loaded.configuration, *candidate.configuration());
        }
        drop(store);
        drop(vfs);
    }
    std::fs::remove_file(path).unwrap();
}
#[test]
fn failed_directory_sync_after_unlink_requires_reaffirmation() {
    let path = path();
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    drop(Store::create(&path, &initial).unwrap());
    let vfs = vfs::Vfs::new();
    let connection =
        Connection::open_with_flags_and_vfs(&path, OpenFlags::SQLITE_OPEN_READ_WRITE, vfs.name())
            .unwrap();
    database::configure(&connection).unwrap();
    let mut store = Store {
        connection,
        uncertain: false,
        pending: None,
        fail_return: false,
    };
    vfs.state.arm(vfs::Event::Delete, 1, false, true);
    assert!(matches!(
        store.commit(&next(&initial)),
        CommitOutcome::Uncertain { .. }
    ));
    let before = vfs
        .state
        .directory_syncs
        .load(std::sync::atomic::Ordering::SeqCst);
    vfs.state.disarm();
    assert_eq!(
        store.reconcile(),
        CommitOutcome::Applied {
            revision: 2,
            changed: true
        }
    );
    assert!(
        vfs.state
            .directory_syncs
            .load(std::sync::atomic::Ordering::SeqCst)
            > before
    );
    drop(store);
    drop(vfs);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn crash_create_child() {
    let Ok(path) = std::env::var("RAD_STORE_CREATE_PATH") else {
        return;
    };
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    Store::create(Path::new(&path), &initial).unwrap();
    panic!("requested create checkpoint not hit");
}
#[test]
fn interrupted_first_installation_never_becomes_silent_empty_replacement() {
    for stage in [
        "create-file",
        "create-schema",
        "create-tuple",
        "create-committed",
    ] {
        let path = path();
        let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "fault_tests::crash_create_child", "--nocapture"])
            .env("RAD_STORE_CREATE_PATH", &path)
            .env("RAD_STORE_CRASH_STAGE", stage)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(77));
        assert!(path.exists());
        if stage == "create-committed" {
            assert_eq!(
                Store::open(&path).unwrap().load().unwrap().configuration,
                *initial.configuration()
            );
        } else {
            assert!(Store::open(&path).is_err());
        }
        assert!(Store::create(&path, &initial).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn oversized_real_normalized_batch_is_rejected_before_any_sql_write() {
    let path = path();
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    drop(Store::create(&path, &initial).unwrap());
    let vfs = vfs::Vfs::new();
    let connection =
        Connection::open_with_flags_and_vfs(&path, OpenFlags::SQLITE_OPEN_READ_WRITE, vfs.name())
            .unwrap();
    database::configure(&connection).unwrap();
    let mut store = Store {
        connection,
        uncertain: false,
        pending: None,
        fail_return: false,
    };
    let (candidate, image) = pictured(&initial);
    assert!(image.png_bytes().len() * 512 > 128 * 1024 * 1024);
    let batch = vec![image; 512]; // Genuine canonical PNGs; not fabricated 512KiB outputs.
    vfs.state.arm(vfs::Event::Write, usize::MAX, false, false);
    assert_eq!(
        store.commit_with_media(&candidate, &batch),
        CommitOutcome::NotApplied(StoreError::Capacity)
    );
    assert_eq!(vfs.state.count(vfs::Event::Write), 0);
    assert_eq!(store.load().unwrap().revision, 1);
    drop(store);
    drop(vfs);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn recovery_discard_sync_failure_blocks_gc_and_reconciles_without_replay() {
    for after in [false, true] {
        let path = path();
        let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
        let mut store = Store::create(&path, &initial).unwrap();
        assert!(matches!(
            store.commit(&next(&initial)),
            CommitOutcome::Applied { .. }
        ));
        store
            .connection
            .execute("UPDATE revisions SET document=x'00' WHERE id=2", [])
            .unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        let loaded = store.load().unwrap();
        let fixed = loaded
            .configuration
            .prepare_edits(&loaded.registry, Origin::Editor, &[])
            .unwrap();
        assert!(matches!(
            store.commit(&fixed),
            CommitOutcome::Applied { .. }
        ));
        drop(store);
        let vfs = vfs::Vfs::new();
        let connection = Connection::open_with_flags_and_vfs(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            vfs.name(),
        )
        .unwrap();
        database::configure(&connection).unwrap();
        let mut store = Store {
            connection,
            uncertain: false,
            pending: None,
            fail_return: false,
        };
        let hold = store.recovery_hold().unwrap().unwrap();
        vfs.state.arm(
            if after {
                vfs::Event::Delete
            } else {
                vfs::Event::Sync
            },
            1,
            true,
            after,
        );
        assert!(matches!(
            store.discard_recovery(hold),
            CommitOutcome::Uncertain { .. }
        ));
        assert_eq!(store.collect(&[]), Err(StoreError::Uncertain));
        assert!(matches!(store.reconcile(), CommitOutcome::Uncertain { .. }));
        vfs.state.disarm();
        let result = store.reconcile();
        assert!(matches!(
            result,
            CommitOutcome::Applied { changed: false, .. } | CommitOutcome::NotApplied(_)
        ));
        let held = store.recovery_hold().unwrap().is_some();
        assert_eq!(held, matches!(result, CommitOutcome::NotApplied(_)));
        store.collect(&[]).unwrap();
        let retained: bool = store
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM revisions WHERE id=2)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(retained, held);
        let loaded = store.load().unwrap();
        assert_eq!(loaded.registry.records().count(), if held { 2 } else { 1 });
        assert_eq!(loaded.registry.counters().buttons, 1);
        assert_eq!(loaded.registry.counters().serial, 2);
        drop(store);
        drop(vfs);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.recovery_hold().unwrap().is_some(), held);
        drop(store);
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn streaming_preflight_error_in_recovery_performs_no_dml() {
    struct Missing;
    impl crate::MediaSource for Missing {
        fn catalog(&self) -> &[crate::MediaEntry] {
            &[]
        }
        fn read(
            &mut self,
            _: &[u8; 32],
        ) -> Result<rad_media::NormalizedImage, crate::MediaReadError> {
            panic!("empty source must never be read")
        }
    }
    let path = path();
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let mut store = Store::create(&path, &initial).unwrap();
    assert!(matches!(
        store.commit(&next(&initial)),
        CommitOutcome::Applied { .. }
    ));
    store
        .connection
        .execute("UPDATE revisions SET document=x'00' WHERE id=2", [])
        .unwrap();
    let loaded = store.load().unwrap();
    let button = loaded
        .configuration
        .prepare_edit(
            &loaded.registry,
            Origin::Editor,
            Edit::CreateButton {
                folder: loaded.configuration.data().principal,
                position: 0,
                name: "media".into(),
            },
        )
        .unwrap();
    let candidate = button
        .configuration()
        .prepare_edit(
            button.registry(),
            Origin::Editor,
            Edit::SetAppearance {
                button: button.configuration().data().buttons[0].binding.identity(),
                appearance: rad_core::configuration::Appearance {
                    text: "".into(),
                    image: Some("00".repeat(32)),
                    text_position: None,
                },
            },
        )
        .unwrap();
    let before: i64 = store
        .connection
        .query_row("SELECT total_changes()", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        store.commit_from_media(&candidate, &mut Missing),
        CommitOutcome::NotApplied(StoreError::Invalid)
    );
    let after: i64 = store
        .connection
        .query_row("SELECT total_changes()", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after, "No DML even if rollback would undo it");
    assert!(store.load().unwrap().recovered_previous);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn crash_migration_child() {
    let Ok(path) = std::env::var("RAD_STORE_MIGRATION_PATH") else {
        return;
    };
    let _ = Store::open(Path::new(&path));
    panic!("requested migration checkpoint not hit");
}
#[test]
fn migration_crash_retries_preserve_original_and_all_revisions() {
    let original = include_bytes!("../../../docs/fixtures/v1/profile.sqlite3");
    for stage in [
        "migration-backup-staged",
        "migration-backup-published",
        "migration-tuple",
        "migration-committed",
    ] {
        let path = path();
        std::fs::write(&path, original).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "fault_tests::crash_migration_child",
                "--nocapture",
            ])
            .env("RAD_STORE_MIGRATION_PATH", &path)
            .env("RAD_STORE_CRASH_STAGE", stage)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(77), "{stage}");
        let store = Store::open(&path).unwrap_or_else(|error| panic!("{stage}: {error:?}"));
        assert_eq!(store.load().unwrap().revision, 5);
        for revision in 1..=5 {
            assert!(
                store.revision(revision).is_ok(),
                "{stage}: revision {revision}"
            );
        }
        let backup = std::path::PathBuf::from(format!("{}.v1-backup", path.display()));
        assert_eq!(std::fs::read(backup).unwrap(), original, "{stage}");
    }
}

#[test]
fn adoption_reconciles_complete_before_or_after_history_without_losing_counters() {
    for rollback in [true, false] {
        let path = path();
        let target = Configuration::prepare_new(&IdentityRegistry::new(88).unwrap()).unwrap();
        let mut store = Store::create(&path, &target).unwrap();
        let source = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
        let source = next(&source);
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
        if rollback {
            FAIL_BEFORE_COMMIT.with(|flag| flag.set(true));
        } else {
            store.fail_return = true;
        }
        assert!(matches!(
            store.commit(&portable.candidate),
            CommitOutcome::Uncertain { .. }
        ));
        let outcome = store.reconcile();
        if rollback {
            assert!(matches!(outcome, CommitOutcome::NotApplied(_)));
            assert_eq!(store.load().unwrap().configuration, *target.configuration());
            assert_eq!(
                store.revision(1).unwrap().configuration,
                *target.configuration()
            );
            assert_eq!(
                store.load().unwrap().registry.counters(),
                target.registry().counters()
            );
        } else {
            assert!(matches!(outcome, CommitOutcome::Applied { .. }));
            assert_eq!(store.load().unwrap().configuration, *source.configuration());
            assert_eq!(
                store.revision(1).unwrap().configuration.data(),
                portable.history_base.as_ref().unwrap()
            );
            assert_eq!(
                store.load().unwrap().registry.counters(),
                portable.candidate.registry().counters()
            );
        }
    }
}
