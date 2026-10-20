use super as transfers;
use crate::server::*;
use rad_authority::{Outcome as CommitOutcome, StorageState};
use rad_core::configuration::{Edit, SettingsData};
use std::os::unix::fs::PermissionsExt;

struct Directory(std::path::PathBuf);
impl Directory {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("rad-server-{label}-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn uncertain_completion_routes_one_reconciliation_to_the_existing_store_worker() {
    let directory = Directory::new("reconcile");
    let mut server = Server::new(
        &directory.0,
        &directory.0.join("data"),
        NonZeroU128::new(735).unwrap(),
    )
    .unwrap();
    let ticket = server.actor.reserve(7, Origin::Editor).unwrap();
    let request = ticket.request();
    let effects = server.actor.submit(
        ticket,
        vec![Edit::SetSettings(SettingsData {
            brightness_percent: 33,
            ..SettingsData::default()
        })],
    );
    let id = effects
        .iter()
        .find_map(|e| match e {
            Effect::Persist { id, .. } => Some(*id),
            _ => None,
        })
        .unwrap();
    // Controlled completion at the existing Authority seam: no write occurred.
    // Real Store.reconcile therefore reports NotApplied. This is not a VFS fault test.
    let unknown = server.actor.complete(id, CommitOutcome::Uncertain);
    server.effects(unknown).unwrap();
    assert_eq!(server.actor.storage_state(), StorageState::Uncertain);
    server.turn().unwrap();
    assert!(
        !server.persistence.can_submit_transfer(),
        "uncertainty must schedule reconciliation instead of requiring a restart"
    );
    let end = std::time::Instant::now() + Duration::from_secs(2);
    while server.actor.storage_state() == StorageState::Uncertain {
        server.turn().unwrap();
        assert!(
            std::time::Instant::now() < end,
            "worker must resolve the pending commit"
        );
        server.wait(Duration::from_millis(5)).unwrap();
    }
    assert_eq!(server.actor.storage_state(), StorageState::Ready);
    assert_ne!(
        server.actor.editor_view(Origin::Editor).unwrap().brightness,
        33
    );
    assert!(
        !server.actor.finish(request),
        "terminal reconciliation released the original credit once"
    );
    assert!(server.actor.reserve(7, Origin::Editor).is_ok());
}

#[test]
fn busy_transfer_queries_have_no_mutation_outcome_and_mutations_keep_not_applied() {
    use rad_client::connection::{Client, Endpoint};
    use std::sync::mpsc;
    let directory = Directory::new("busy-read");
    let mut server = Server::new(
        &directory.0,
        &directory.0.join("data"),
        NonZeroU128::new(736).unwrap(),
    )
    .unwrap();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (go_tx, go_rx) = mpsc::sync_channel(1);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let root = directory.0.clone();
    let client = std::thread::spawn(move || {
        let mut client = Client::connect(&root, Endpoint::Editor).unwrap();
        ready_tx.send(()).unwrap();
        go_rx.recv().unwrap();
        for request in [
            rad_api::Request::TransferRead {
                transfer_id: "held".into(),
                offset: "0".into(),
            },
            rad_api::Request::MediaRead {
                id: "b1".into(),
                offset: "0".into(),
            },
            rad_api::Request::TransferWrite {
                transfer_id: "held".into(),
                offset: "0".into(),
                base64: String::new(),
            },
        ] {
            let operation = request.operation();
            let response = client.call(request).unwrap();
            let error = response.error.unwrap();
            assert_eq!(error.code, "resource_exhausted");
            assert_eq!(
                error.outcome,
                (operation == rad_api::Operation::TransferWrite).then_some(Outcome::NotApplied),
                "Busy reads are not mutations"
            );
        }
        done_tx.send(()).unwrap();
    });
    let end = std::time::Instant::now() + Duration::from_secs(3);
    while ready_rx.try_recv().is_err() {
        server.turn().unwrap();
        assert!(std::time::Instant::now() < end);
        server.wait(Duration::from_millis(5)).unwrap();
    }
    // Hold the one worker credit until this test consumes it, independent of IO speed.
    let job = server
        .persistence
        .submit_transfer(
            crate::transfers::Context {
                connection: 0,
                origin: Origin::Editor,
                now_ms: monotonic_ms().unwrap(),
            },
            crate::transfers::Operation::Reap,
        )
        .unwrap();
    server.transfer = Some(transfers::Flight {
        job,
        target: None,
        kind: transfers::FlightKind::Internal,
    });
    go_tx.send(()).unwrap();
    loop {
        server.transport.drive(monotonic_ms().unwrap()).unwrap();
        if let Some(incoming) = server.transport.take_next() {
            let context = IncomingContext {
                request_id: incoming.key.id(),
                connection: incoming.key.connection(),
                origin: Origin::Editor,
            };
            // Typed External seam: no authorization or Store read is being tested here.
            let external = match incoming.request.operation() {
                "transfer_read" => External::TransferRead {
                    transfer_id: "held".into(),
                    offset: 0,
                },
                "media_read" => External::MediaRead {
                    id: rad_core::identity::ShortId::new(rad_core::identity::EntityKind::Button, 1)
                        .unwrap(),
                    media_ref: "held".into(),
                    offset: 0,
                },
                "transfer_write" => External::TransferWrite {
                    transfer_id: "held".into(),
                    offset: 0,
                    bytes: vec![],
                },
                _ => panic!("unexpected operation"),
            };
            server.external(incoming.key, context, external).unwrap();
        }
        if done_rx.try_recv().is_ok() {
            break;
        }
        assert!(
            std::time::Instant::now() < end,
            "busy response must complete"
        );
        server
            .transport
            .wait_readiness(Duration::from_millis(5))
            .unwrap();
    }
    client.join().unwrap();
}

#[test]
fn reconciliation_completions_publish_once_or_keep_uncertainty_without_retrying() {
    for (index, outcome) in [
        CommitOutcome::Applied {
            revision: 2,
            changed: true,
        },
        CommitOutcome::Uncertain,
        CommitOutcome::RecoveryRequired,
    ]
    .into_iter()
    .enumerate()
    {
        let directory = Directory::new(&format!("reconcile-outcome-{index}"));
        let mut server = Server::new(
            &directory.0,
            &directory.0.join("data"),
            NonZeroU128::new(737).unwrap(),
        )
        .unwrap();
        let ticket = server.actor.reserve(7, Origin::Editor).unwrap();
        let effects = server.actor.submit(
            ticket,
            vec![Edit::SetSettings(SettingsData {
                brightness_percent: 33,
                ..SettingsData::default()
            })],
        );
        let id = effects
            .iter()
            .find_map(|e| match e {
                Effect::Persist { id, .. } => Some(*id),
                _ => None,
            })
            .unwrap();
        let effects = server.actor.complete(id, CommitOutcome::Uncertain);
        server.effects(effects).unwrap();
        let effects = server.reconcile_once();
        let id = effects
            .iter()
            .find_map(|e| match e {
                Effect::Reconcile { id } => Some(*id),
                _ => None,
            })
            .expect("one scheduled reconcile");
        assert!(server.reconcile_once().is_empty());
        // Controlled completions exercise Server/Authority routing only, not Store durability.
        let effects = server.actor.complete(id, outcome);
        server.effects(effects).unwrap();
        assert!(server.reconcile_once().is_empty());
        assert!(
            server.actor.complete(id, outcome).is_empty(),
            "duplicate completion cannot publish twice"
        );
        let expected = match outcome {
            CommitOutcome::Applied { .. } => StorageState::Ready,
            CommitOutcome::Uncertain => StorageState::Uncertain,
            _ => StorageState::RecoveryRequired,
        };
        assert_eq!(server.actor.storage_state(), expected);
        assert_eq!(
            server.actor.editor_view(Origin::Editor).unwrap().brightness == 33,
            expected == StorageState::Ready
        );
        assert_eq!(
            server.actor.commit_request(id).is_some(),
            expected != StorageState::Ready,
            "uncertain original commit stays retained"
        );
    }
}
