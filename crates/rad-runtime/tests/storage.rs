use rad_core::{configuration::*, identity::IdentityRegistry};
use rad_runtime::storage::*;
use rad_store::{CommitOutcome, Store};
use std::{sync::Arc, time::Duration};
#[test]
fn real_store_commit_is_single_flight_until_completion_consumed_and_wakes_io_owner() {
    let path =
        std::env::temp_dir().join(format!("rad-runtime-store-{}.sqlite3", std::process::id()));
    let base = Configuration::prepare_new(&IdentityRegistry::new(765).unwrap()).unwrap();
    let store = Store::create(&path, &base).unwrap();
    let mut poll = mio::Poll::new().unwrap();
    let wake = Arc::new(mio::Waker::new(poll.registry(), mio::Token(77)).unwrap());
    let mut worker = StorageWorker::spawn(store, wake).unwrap();
    let next = base
        .configuration()
        .prepare_edit(
            base.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                brightness_percent: 23,
                ..SettingsData::default()
            }),
        )
        .unwrap();
    let id = worker.submit(Job::Commit(next)).unwrap();
    assert_eq!(worker.submit(Job::Commit(base)), Err(WorkerError::Busy));
    let mut events = mio::Events::with_capacity(4);
    poll.poll(&mut events, Some(Duration::from_secs(2)))
        .unwrap();
    assert!(events.iter().any(|e| e.token() == mio::Token(77)));
    let (got, result) = worker.take().unwrap().unwrap();
    assert_eq!(got, id);
    let ResultValue::Commit(result) = result else {
        panic!("expected commit");
    };
    assert_eq!(
        result,
        CommitOutcome::Applied {
            revision: 2,
            changed: true
        }
    );
    assert_eq!(
        Store::open(&path)
            .unwrap()
            .load()
            .unwrap()
            .configuration
            .settings()
            .brightness_percent(),
        23
    );
    drop(worker);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn transfer_runs_on_existing_store_worker_and_is_single_flight() {
    use rad_runtime::transfers::*;
    let path = std::env::temp_dir().join(format!(
        "rad-runtime-transfer-worker-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    let base = Configuration::prepare_new(&IdentityRegistry::new(765).unwrap()).unwrap();
    let store = Store::create(&path.join("db"), &base).unwrap();
    let transfers = Transfers::create(&path.join("transfers"), 76).unwrap();
    let mut poll = mio::Poll::new().unwrap();
    let wake = Arc::new(mio::Waker::new(poll.registry(), mio::Token(7)).unwrap());
    let mut worker = StorageWorker::with_transfers(store, transfers, wake).unwrap();
    let id = worker
        .submit(Job::Transfer {
            context: Context {
                connection: 1,
                origin: Origin::Editor,
                now_ms: 0,
            },
            operation: Operation::Begin {
                kind: Kind::ImageImport,
                size: Some(1),
                export_revision: None,
            },
        })
        .unwrap();
    assert_eq!(worker.submit(Job::ReadRevision(1)), Err(WorkerError::Busy));
    let end = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if let Some((got, value)) = worker.take().unwrap() {
            assert_eq!(got, id);
            assert!(matches!(
                value,
                ResultValue::Transfer(Ok(Output::Begun { .. }))
            ));
            break;
        }
        assert!(std::time::Instant::now() < end);
        poll.poll(
            &mut mio::Events::with_capacity(4),
            Some(Duration::from_millis(50)),
        )
        .unwrap();
    }
    drop(worker);
    std::fs::remove_dir_all(path).unwrap();
}
