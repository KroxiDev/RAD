use rad_authority::{Authority, Command, Effect, RequestId};
use rad_core::{configuration::*, identity::IdentityRegistry};
use rad_runtime::persistence::Persistence;
use rad_store::Store;
use std::{
    num::NonZeroU128,
    sync::Arc,
    time::{Duration, Instant},
};
struct Fixture(std::path::PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn settle(
    actor: &mut Authority,
    persistence: &mut Persistence,
    poll: &mut mio::Poll,
    request: RequestId,
) {
    let end = Instant::now() + Duration::from_secs(3);
    let mut events = mio::Events::with_capacity(4);
    loop {
        for e in persistence.poll(actor) {
            if let Effect::Reply {
                request: id,
                changed,
            } = e
            {
                assert_eq!(id, request);
                assert!(changed);
                actor.finish(id);
                return;
            }
            assert!(!matches!(
                e,
                Effect::Rejected { .. } | Effect::Unknown { .. }
            ));
        }
        assert!(
            Instant::now() < end,
            "real Store completion must produce Applied"
        );
        poll.poll(&mut events, Some(Duration::from_millis(100)))
            .unwrap();
    }
}
#[test]
fn edit_publishes_only_on_real_store_completion_and_undo_redo_use_retained_revisions() {
    let file = Fixture(
        std::env::temp_dir().join(format!("rad-publication-{}.sqlite3", std::process::id())),
    );
    let initial = Configuration::prepare_new(&IdentityRegistry::new(544).unwrap()).unwrap();
    let store = Store::create(&file.0, &initial).unwrap();
    let mut actor = Authority::new(initial, 1, NonZeroU128::new(99).unwrap());
    let mut poll = mio::Poll::new().unwrap();
    let wake = Arc::new(mio::Waker::new(poll.registry(), mio::Token(9)).unwrap());
    let mut persistence = Persistence::new(store, wake).unwrap();
    let ticket = actor.reserve(1, Origin::Editor).unwrap();
    let request = ticket.request();
    let effects = actor.submit(
        ticket,
        Command::Edits(vec![Edit::SetSettings(SettingsData {
            brightness_percent: 23,
            ..SettingsData::default()
        })]),
    );
    assert_eq!(actor.editor_view(Origin::Editor).unwrap().revision, 1);
    let effects = persistence.route(&mut actor, effects);
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::Persist { .. } | Effect::Reply { .. })),
        "Store effect must be submitted, never returned as an Applied response"
    );
    assert_eq!(actor.editor_view(Origin::Editor).unwrap().revision, 1);
    settle(&mut actor, &mut persistence, &mut poll, request);
    assert_eq!(actor.editor_view(Origin::Editor).unwrap().brightness, 23);
    assert_eq!(
        Store::open(&file.0)
            .unwrap()
            .load()
            .unwrap()
            .configuration
            .settings()
            .brightness_percent(),
        23
    );
    for (command, expected) in [
        (Command::Undo, SettingsData::default().brightness_percent),
        (Command::Redo, 23),
    ] {
        let ticket = actor.reserve(2, Origin::Editor).unwrap();
        let request = ticket.request();
        let effects = actor.submit(ticket, command);
        let effects = persistence.route(&mut actor, effects);
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::ReadRevision { .. } | Effect::Reply { .. }))
        );
        settle(&mut actor, &mut persistence, &mut poll, request);
        assert_eq!(
            actor.editor_view(Origin::Editor).unwrap().brightness,
            expected
        );
        assert_eq!(
            Store::open(&file.0)
                .unwrap()
                .load()
                .unwrap()
                .configuration
                .settings()
                .brightness_percent(),
            expected
        );
    }
}
#[test]
fn started_persist_waits_for_active_transfer_without_rejection_or_second_queue() {
    use rad_runtime::{storage::WorkerError, transfers::*};
    let path = std::env::temp_dir().join(format!(
        "rad-persist-transfer-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    let base = Configuration::prepare_new(&IdentityRegistry::new(655).unwrap()).unwrap();
    let store = Store::create(&path.join("db"), &base).unwrap();
    let mut actor = Authority::new(base, 1, NonZeroU128::new(55).unwrap());
    let transfers = Transfers::create(&path.join("transfers"), 55).unwrap();
    let mut poll = mio::Poll::new().unwrap();
    let wake = Arc::new(mio::Waker::new(poll.registry(), mio::Token(1)).unwrap());
    let mut p = Persistence::with_transfers(store, transfers, wake).unwrap();
    let job = p
        .submit_transfer(
            Context {
                connection: 1,
                origin: Origin::Editor,
                now_ms: 0,
            },
            Operation::Begin {
                kind: Kind::ImageImport,
                size: Some(1),
                export_revision: None,
            },
        )
        .unwrap();
    let ticket = actor.reserve(1, Origin::Editor).unwrap();
    let request = ticket.request();
    let effects = actor.submit(
        ticket,
        Command::Edits(vec![Edit::SetSettings(SettingsData {
            brightness_percent: 19,
            ..Default::default()
        })]),
    );
    let out = p.route(&mut actor, effects);
    assert!(
        !out.iter()
            .any(|e| matches!(e, Effect::Rejected { .. } | Effect::Unknown { .. }))
    );
    assert_eq!(
        p.submit_transfer(
            Context {
                connection: 2,
                origin: Origin::Editor,
                now_ms: 1
            },
            Operation::Reap
        ),
        Err(WorkerError::Busy)
    );
    settle(&mut actor, &mut p, &mut poll, request);
    let (got, result) = p.take_transfer().unwrap();
    assert_eq!(got, job);
    assert!(matches!(result, Ok(Output::Begun { .. })));
    assert_eq!(actor.editor_view(Origin::Editor).unwrap().brightness, 19);
    drop(p);
    std::fs::remove_dir_all(path).unwrap();
}

const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240, 31, 0,
    5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
fn transfer(
    p: &mut Persistence,
    a: &mut Authority,
    poll: &mut mio::Poll,
    connection: u64,
    op: rad_runtime::transfers::Operation,
) -> rad_runtime::transfers::Output {
    use rad_runtime::transfers::Context;
    let id = p
        .submit_transfer(
            Context {
                connection,
                origin: Origin::Editor,
                now_ms: 0,
            },
            op,
        )
        .unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        let effects = p.poll(a);
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::Rejected { .. } | Effect::Unknown { .. }))
        );
        if let Some((got, result)) = p.take_transfer() {
            assert_eq!(got, id);
            return result.unwrap();
        }
        assert!(Instant::now() < end);
        poll.poll(
            &mut mio::Events::with_capacity(4),
            Some(Duration::from_millis(30)),
        )
        .unwrap();
    }
}
fn import(
    p: &mut Persistence,
    a: &mut Authority,
    poll: &mut mio::Poll,
    connection: u64,
) -> (String, String) {
    use rad_runtime::transfers::{Kind, Operation as Op, Output};
    let id = match transfer(
        p,
        a,
        poll,
        connection,
        Op::Begin {
            kind: Kind::ImageImport,
            size: Some(PNG.len() as u64),
            export_revision: None,
        },
    ) {
        Output::Begun { transfer_id } => transfer_id,
        _ => panic!(),
    };
    transfer(
        p,
        a,
        poll,
        connection,
        Op::Write {
            transfer_id: id.clone(),
            offset: 0,
            bytes: PNG.to_vec(),
        },
    );
    let key = match transfer(
        p,
        a,
        poll,
        connection,
        Op::Finish {
            transfer_id: id.clone(),
        },
    ) {
        Output::ImagePrepared { media_ref } => media_ref,
        _ => panic!(),
    };
    assert!(matches!(
        transfer(
            p,
            a,
            poll,
            connection,
            Op::RetainImage {
                media_ref: key.clone()
            }
        ),
        Output::Retained { .. }
    ));
    (id, key)
}
#[test]
fn retained_image_binds_to_authority_commit_and_rejected_candidate_cleans_on_worker() {
    use rad_runtime::transfers::{Operation as Op, Output, Transfers};
    let path = std::env::temp_dir().join(format!(
        "rad-persist-image-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    let initial = Configuration::prepare_new(&IdentityRegistry::new(655).unwrap()).unwrap();
    let base = initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: initial.configuration().data().principal,
                position: 0,
                name: "Image".into(),
            },
        )
        .unwrap();
    let button = base.configuration().data().buttons[0].binding.identity();
    let store = Store::create(&path.join("db"), &base).unwrap();
    let mut actor = Authority::new(base, 1, NonZeroU128::new(55).unwrap());
    let area = Transfers::create(&path.join("transfers"), 55).unwrap();
    let mut poll = mio::Poll::new().unwrap();
    let wake = Arc::new(mio::Waker::new(poll.registry(), mio::Token(1)).unwrap());
    let mut p = Persistence::with_transfers(store, area, wake).unwrap();
    let (id, key) = import(&mut p, &mut actor, &mut poll, 1);
    let ticket = actor.reserve(1, Origin::Editor).unwrap();
    let request = ticket.request();
    p.bind_media(request, 1, id.clone()).unwrap();
    let effects = actor.submit(
        ticket,
        Command::Edits(vec![Edit::SetAppearance {
            button,
            appearance: Appearance {
                text: "Image".into(),
                image: Some(key.clone()),
                text_position: None,
            },
        }]),
    );
    p.route(&mut actor, effects);
    settle(&mut actor, &mut p, &mut poll, request);
    assert!(p.transfer_metadata().unwrap().handles.is_empty());
    assert_eq!(p.take_released(), vec![id]);
    match transfer(
        &mut p,
        &mut actor,
        &mut poll,
        1,
        Op::LoadVisual { resolved_key: key },
    ) {
        Output::Visual { image } => assert_eq!(image, rad_media::normalize(PNG).unwrap()),
        _ => panic!(),
    }
    let (id, key) = import(&mut p, &mut actor, &mut poll, 2);
    let ticket = actor.reserve(2, Origin::Editor).unwrap();
    let request = ticket.request();
    p.bind_media(request, 2, id.clone()).unwrap();
    let effects = actor.submit(
        ticket,
        Command::Edits(vec![Edit::SetAppearance {
            button: rad_core::identity::EntityIdentity::new(999, 1).unwrap(),
            appearance: Appearance {
                text: "bad".into(),
                image: Some(key),
                text_position: None,
            },
        }]),
    );
    let output = p.route(&mut actor, effects);
    assert!(
        output
            .iter()
            .any(|e| matches!(e,Effect::Rejected{request:r,..} if *r==request))
    );
    actor.finish(request);
    let end = Instant::now() + Duration::from_secs(3);
    while !p.transfer_metadata().unwrap().handles.is_empty() {
        p.poll(&mut actor);
        assert!(Instant::now() < end);
        poll.poll(
            &mut mio::Events::with_capacity(4),
            Some(Duration::from_millis(10)),
        )
        .unwrap();
    }
    assert_eq!(p.take_released(), vec![id]);
    drop(p);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn pinned_export_prepare_restore_publishes_only_after_same_worker_commit() {
    use rad_runtime::transfers::{Kind, Operation as Op, Output, Transfers};
    let path = std::env::temp_dir().join(format!(
        "rad-persist-restore-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    let base = Configuration::prepare_new(&IdentityRegistry::new(955).unwrap()).unwrap();
    let store = Store::create(&path.join("db"), &base).unwrap();
    let mut actor = Authority::new(base, 1, NonZeroU128::new(45).unwrap());
    let area = Transfers::create(&path.join("transfers"), 45).unwrap();
    let mut poll = mio::Poll::new().unwrap();
    let wake = Arc::new(mio::Waker::new(poll.registry(), mio::Token(1)).unwrap());
    let mut p = Persistence::with_transfers(store, area, wake).unwrap();
    let lease = actor.pin_export(1, Origin::Editor).unwrap();
    let id = match transfer(
        &mut p,
        &mut actor,
        &mut poll,
        1,
        Op::Begin {
            kind: Kind::Export,
            size: None,
            export_revision: Some(lease.revision()),
        },
    ) {
        Output::Begun { transfer_id } => transfer_id,
        _ => panic!(),
    };
    let mut bytes = vec![];
    loop {
        match transfer(
            &mut p,
            &mut actor,
            &mut poll,
            1,
            Op::Read {
                transfer_id: id.clone(),
                offset: bytes.len() as u64,
            },
        ) {
            Output::Chunk { bytes: part, eof } => {
                bytes.extend(part);
                if eof {
                    break;
                }
            }
            _ => panic!(),
        }
    }
    assert_eq!(p.take_released(), vec![id]);
    assert!(actor.release_export(lease));
    let id = match transfer(
        &mut p,
        &mut actor,
        &mut poll,
        1,
        Op::Begin {
            kind: Kind::Restore,
            size: Some(bytes.len() as u64),
            export_revision: None,
        },
    ) {
        Output::Begun { transfer_id } => transfer_id,
        _ => panic!(),
    };
    for (i, part) in bytes.chunks(8192).enumerate() {
        transfer(
            &mut p,
            &mut actor,
            &mut poll,
            1,
            Op::Write {
                transfer_id: id.clone(),
                offset: (i * 8192) as u64,
                bytes: part.to_vec(),
            },
        );
    }
    assert!(matches!(
        transfer(
            &mut p,
            &mut actor,
            &mut poll,
            1,
            Op::Finish {
                transfer_id: id.clone()
            }
        ),
        Output::RestorePrepared { .. }
    ));
    let ticket = actor.reserve(2, Origin::Editor).unwrap();
    let request = ticket.request();
    let effects = actor.submit(
        ticket,
        vec![Edit::SetSettings(SettingsData {
            brightness_percent: 33,
            ..Default::default()
        })],
    );
    p.route(&mut actor, effects);
    settle(&mut actor, &mut p, &mut poll, request);
    let data = match transfer(
        &mut p,
        &mut actor,
        &mut poll,
        1,
        Op::RestoreData {
            transfer_id: id.clone(),
            confirmed: true,
        },
    ) {
        Output::RestoreData { data } => data,
        _ => panic!(),
    };
    let ticket = actor.reserve(1, Origin::Editor).unwrap();
    let request = ticket.request();
    p.bind_media(request, 1, id.clone()).unwrap();
    let effects = actor.submit(
        ticket,
        Command::Restore {
            data,
            confirmed: true,
        },
    );
    p.route(&mut actor, effects);
    assert_eq!(actor.editor_view(Origin::Editor).unwrap().brightness, 33);
    settle(&mut actor, &mut p, &mut poll, request);
    assert_eq!(
        actor.editor_view(Origin::Editor).unwrap().brightness,
        SettingsData::default().brightness_percent
    );
    assert_eq!(p.take_released(), vec![id]);
    drop(p);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn unbound_retained_preflight_releases_on_worker_and_readiness_is_not_busy_polling() {
    use rad_runtime::transfers::Transfers;
    let path = std::env::temp_dir().join(format!(
        "rad-persist-unbound-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    let base = Configuration::prepare_new(&IdentityRegistry::new(865).unwrap()).unwrap();
    let store = Store::create(&path.join("db"), &base).unwrap();
    let mut actor = Authority::new(base, 1, NonZeroU128::new(15).unwrap());
    let area = Transfers::create(&path.join("transfers"), 15).unwrap();
    let mut poll = mio::Poll::new().unwrap();
    let wake = Arc::new(mio::Waker::new(poll.registry(), mio::Token(1)).unwrap());
    let mut p = Persistence::with_transfers(store, area, wake).unwrap();
    assert!(p.can_submit_transfer());
    let (id, _) = import(&mut p, &mut actor, &mut poll, 1);
    p.release_unbound(id.clone()).unwrap();
    assert!(!p.can_submit_transfer());
    let end = Instant::now() + Duration::from_secs(3);
    while !p.transfer_metadata().unwrap().handles.is_empty() {
        p.poll(&mut actor);
        assert!(Instant::now() < end);
        poll.poll(
            &mut mio::Events::with_capacity(4),
            Some(Duration::from_millis(10)),
        )
        .unwrap();
    }
    assert_eq!(p.take_released(), vec![id]);
    assert!(p.can_submit_transfer());
    drop(p);
    std::fs::remove_dir_all(path).unwrap();
}
