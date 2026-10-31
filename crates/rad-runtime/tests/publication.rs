use rad_authority::{Authority, Command, Effect, RequestId};
use rad_core::{
    configuration::{
        ActionKind, Appearance, BrightnessDirection, Configuration, Edit, Origin, SettingsData,
    },
    identity::IdentityRegistry,
};
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
            export_snapshot: None,
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
fn portable_restore_virgin_has_one_undo_and_exact_redo_after_real_commit() {
    let file = Fixture(std::env::temp_dir().join(format!(
        "rad-portable-publication-{}.sqlite3",
        std::process::id()
    )));
    let initial = Configuration::prepare_new(&IdentityRegistry::new(544).unwrap()).unwrap();
    let source = Configuration::prepare_new(&IdentityRegistry::new(966).unwrap()).unwrap();
    let source = source
        .configuration()
        .prepare_edit(
            source.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: source.configuration().data().principal,
                position: 0,
                name: "portable".into(),
            },
        )
        .unwrap();
    let store = Store::create(&file.0, &initial).unwrap();
    let mut actor = Authority::new(initial, 1, NonZeroU128::new(99).unwrap());
    let mut poll = mio::Poll::new().unwrap();
    let wake = Arc::new(mio::Waker::new(poll.registry(), mio::Token(9)).unwrap());
    let mut persistence = Persistence::new(store, wake).unwrap();
    for (command, count) in [
        (
            Command::RestorePortable {
                data: source.configuration().data().clone(),
                installation: 966,
                counters: source.registry().counters(),
                confirmed: true,
            },
            1,
        ),
        (Command::Undo, 0),
        (Command::Redo, 1),
    ] {
        let ticket = actor.reserve(1, Origin::Editor).unwrap();
        let request = ticket.request();
        let effects = actor.submit(ticket, command);
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Rejected { .. })),
            "{effects:?}"
        );
        persistence.route(&mut actor, effects);
        settle(&mut actor, &mut persistence, &mut poll, request);
        let loaded = Store::open(&file.0).unwrap().load().unwrap();
        assert_eq!(loaded.configuration.data().buttons.len(), count);
        if count == 1 {
            assert_eq!(loaded.configuration, *source.configuration());
        }
    }
}

#[test]
fn collection_prunes_actor_registry_and_next_commit_does_not_resurrect_retired_ids() {
    let file = Fixture(
        std::env::temp_dir().join(format!("rad-portable-gc-{}.sqlite3", std::process::id())),
    );
    let initial = Configuration::prepare_new(&IdentityRegistry::new(544).unwrap()).unwrap();
    let root = initial.configuration().data().principal;
    let store = Store::create(&file.0, &initial).unwrap();
    let mut actor = Authority::new(initial, 1, NonZeroU128::new(99).unwrap());
    let mut poll = mio::Poll::new().unwrap();
    let wake = Arc::new(mio::Waker::new(poll.registry(), mio::Token(9)).unwrap());
    let mut persistence = Persistence::new(store, wake).unwrap();
    for i in 0..30 {
        let edit = if i == 0 {
            Edit::CreateButton {
                folder: root,
                position: 0,
                name: "retired".into(),
            }
        } else if i == 1 {
            let loaded = Store::open(&file.0).unwrap().load().unwrap();
            Edit::DeleteButton {
                button: loaded.configuration.data().buttons[0].binding.identity(),
            }
        } else {
            Edit::SetSettings(SettingsData {
                brightness_percent: i + 10,
                ..Default::default()
            })
        };
        let ticket = actor.reserve(1, Origin::Editor).unwrap();
        let request = ticket.request();
        let effects = actor.submit(ticket, vec![edit]);
        persistence.route(&mut actor, effects);
        settle(&mut actor, &mut persistence, &mut poll, request);
    }
    let effects = actor.collect().unwrap();
    persistence.route(&mut actor, effects);
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        if persistence.poll(&mut actor).iter().any(|e| {
            matches!(
                e,
                Effect::CollectionFinished {
                    outcome: rad_authority::Outcome::Applied { .. }
                }
            )
        }) {
            break;
        }
        assert!(Instant::now() < end);
        poll.poll(
            &mut mio::Events::with_capacity(4),
            Some(Duration::from_millis(30)),
        )
        .unwrap();
    }
    let lease = actor.pin_export(1, Origin::Editor).unwrap();
    assert_eq!(
        lease.snapshot().registry().records().count(),
        1,
        "actor must release retired identities after confirmed collection"
    );
    actor.release_export(lease);
    let ticket = actor.reserve(1, Origin::Editor).unwrap();
    let request = ticket.request();
    let effects = actor.submit(
        ticket,
        vec![Edit::CreateButton {
            folder: root,
            position: 0,
            name: "next".into(),
        }],
    );
    persistence.route(&mut actor, effects);
    settle(&mut actor, &mut persistence, &mut poll, request);
    let loaded = Store::open(&file.0).unwrap().load().unwrap();
    assert_eq!(loaded.registry.records().count(), 2);
    assert_eq!(
        loaded.configuration.data().buttons[0]
            .binding
            .short()
            .number(),
        2
    );
}

#[test]
fn counters_only_restore_survives_worker_commit_reopen_and_next_allocation() {
    let file = Fixture(std::env::temp_dir().join(format!(
        "rad-counter-restore-{}.sqlite3",
        std::process::id()
    )));
    let initial = Configuration::prepare_new(&IdentityRegistry::new(544).unwrap()).unwrap();
    let mut counters = initial.registry().counters();
    counters.buttons = 7;
    counters.serial = 8;
    let data = initial.configuration().data().clone();
    let root = data.principal;
    let store = Store::create(&file.0, &initial).unwrap();
    let mut actor = Authority::new(initial, 1, NonZeroU128::new(99).unwrap());
    let mut poll = mio::Poll::new().unwrap();
    let wake = Arc::new(mio::Waker::new(poll.registry(), mio::Token(9)).unwrap());
    let mut persistence = Persistence::new(store, wake).unwrap();
    let ticket = actor.reserve(1, Origin::Editor).unwrap();
    let request = ticket.request();
    let effects = actor.submit(
        ticket,
        Command::RestorePortable {
            data,
            installation: 544,
            counters,
            confirmed: true,
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Persist { .. })));
    persistence.route(&mut actor, effects);
    settle(&mut actor, &mut persistence, &mut poll, request);
    let loaded = Store::open(&file.0).unwrap().load().unwrap();
    assert_eq!(loaded.registry.counters(), counters);
    let next = loaded
        .configuration
        .prepare_edit(
            &loaded.registry,
            Origin::Editor,
            Edit::CreateButton {
                folder: root,
                position: 0,
                name: "after reserve".into(),
            },
        )
        .unwrap();
    assert_eq!(
        next.configuration().data().buttons[0]
            .binding
            .short()
            .number(),
        8
    );
}

#[test]
fn original_empty_backup_after_adoption_has_exact_undo_redo_and_reopens() {
    let file = Fixture(std::env::temp_dir().join(format!(
        "rad-initial-root-publication-{}.sqlite3",
        std::process::id()
    )));
    let initial = Configuration::prepare_new(&IdentityRegistry::new(544).unwrap()).unwrap();
    let backup = initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                brightness_percent: 30,
                ..Default::default()
            }),
        )
        .unwrap();
    let foreign = Configuration::prepare_new(&IdentityRegistry::new(966).unwrap()).unwrap();
    let foreign = foreign
        .configuration()
        .prepare_edit(
            foreign.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: foreign.configuration().data().principal,
                position: 0,
                name: "foreign".into(),
            },
        )
        .unwrap();
    let store = Store::create(&file.0, &backup).unwrap();
    let mut actor = Authority::new(backup.clone(), 1, NonZeroU128::new(99).unwrap());
    let mut poll = mio::Poll::new().unwrap();
    let wake = Arc::new(mio::Waker::new(poll.registry(), mio::Token(9)).unwrap());
    let mut persistence = Persistence::new(store, wake).unwrap();
    let mut restored = None;
    for (step, command) in [
        Command::RestorePortable {
            data: foreign.configuration().data().clone(),
            installation: 966,
            counters: foreign.registry().counters(),
            confirmed: true,
        },
        Command::RestorePortable {
            data: backup.configuration().data().clone(),
            installation: 544,
            counters: backup.registry().counters(),
            confirmed: true,
        },
        Command::Undo,
        Command::Redo,
    ]
    .into_iter()
    .enumerate()
    {
        let ticket = actor.reserve(1, Origin::Editor).unwrap();
        let request = ticket.request();
        let effects = actor.submit(ticket, command);
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Rejected { .. })),
            "{effects:?}"
        );
        persistence.route(&mut actor, effects);
        settle(&mut actor, &mut persistence, &mut poll, request);
        let loaded = Store::open(&file.0).unwrap().load().unwrap();
        match step {
            0 | 2 => assert_eq!(loaded.configuration, *foreign.configuration()),
            1 => {
                let root = loaded.configuration.data().folders[0].binding;
                assert!(root.short().number() > 2);
                assert_eq!(root.identity().installation(), 544);
                let mut expected = backup.configuration().data().clone();
                expected.principal = root.identity();
                expected.folders[0].binding = root;
                assert_eq!(loaded.configuration.data(), &expected);
                restored = Some(loaded.configuration);
            }
            3 => assert_eq!(Some(loaded.configuration), restored),
            _ => unreachable!(),
        }
    }
    drop(persistence);
    assert_eq!(
        Some(Store::open(&file.0).unwrap().load().unwrap().configuration),
        restored
    );
}

#[test]
fn native_brightness_without_client_publication_ticket_is_durable_through_the_real_store_worker() {
    use rad_core::execution::{ExecutionStatus, Target};
    let file = Fixture(std::env::temp_dir().join(format!(
        "rad-native-brightness-{}.sqlite3",
        std::process::id()
    )));
    let c = Configuration::prepare_new(&IdentityRegistry::new(731).unwrap()).unwrap();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "Brightness".into(),
            },
        )
        .unwrap();
    let button = c.configuration().data().buttons[0].binding.identity();
    let initial = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                Edit::AddAction {
                    button,
                    kind: ActionKind::Brightness(BrightnessDirection::Increase),
                },
                Edit::SetSettings(SettingsData {
                    brightness_percent: 55,
                    ..SettingsData::default()
                }),
            ],
        )
        .unwrap();
    let store = Store::create(&file.0, &initial).unwrap();
    let mut actor = Authority::new(initial, 1, NonZeroU128::new(732).unwrap());
    let mut poll = mio::Poll::new().unwrap();
    let wake = Arc::new(mio::Waker::new(poll.registry(), mio::Token(9)).unwrap());
    let mut persistence = Persistence::new(store, wake).unwrap();
    let ticket = actor.reserve(1, Origin::Editor).unwrap();
    let request = ticket.request();
    let effects = actor.submit(
        ticket,
        Command::Run {
            target: Target::Button(button),
        },
    );
    let execution = effects
        .iter()
        .find_map(|e| {
            if let Effect::Accepted { execution, .. } = e {
                Some(*execution)
            } else {
                None
            }
        })
        .unwrap();
    assert!(actor.finish(request));
    persistence.route(&mut actor, effects);
    assert_eq!(
        actor.settings(Origin::Editor).unwrap().brightness_percent,
        55
    );
    let end = Instant::now() + Duration::from_secs(3);
    let mut events = mio::Events::with_capacity(4);
    loop {
        let effects = persistence.poll(&mut actor);
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::Rejected { .. } | Effect::Unknown { .. }))
        );
        if actor.result(Origin::Editor, execution).unwrap().status == ExecutionStatus::Succeeded {
            break;
        }
        assert!(
            Instant::now() < end,
            "native publication must finish without a client request binding"
        );
        poll.poll(&mut events, Some(Duration::from_millis(50)))
            .unwrap();
    }
    assert_eq!(
        actor.settings(Origin::Editor).unwrap().brightness_percent,
        65
    );
    assert_eq!(actor.editor_view(Origin::Editor).unwrap().revision, 2);
    drop(persistence);
    let store = Store::open(&file.0).unwrap();
    assert_eq!(
        store
            .load()
            .unwrap()
            .configuration
            .settings()
            .brightness_percent(),
        65
    );
}
