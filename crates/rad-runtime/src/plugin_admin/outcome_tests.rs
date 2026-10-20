// Private message-boundary fault simulation, NOT a disk failure or C2 fault hook.
use super::*;
use rad_core::{configuration::Configuration, identity::IdentityRegistry};
use rad_plugin_manager::{Error, Manager, Operation};
use std::os::unix::fs::DirBuilderExt;
#[test]
fn unconfirmed_and_uncertain_keep_barrier_credit_but_confirmed_not_applied_releases() {
    for uncertain in [true, false] {
        let root =
            std::env::temp_dir().join(format!("rad-d1-outcome-{}-{uncertain}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let mut manager = Manager::create(&root, 1, rad_plugin_host::TARGET).unwrap();
        let prepared = manager
            .prepare(
                Operation::Install,
                Some(&super::lifecycle_tests::package(1)),
            )
            .unwrap();
        let key = prepared.key();
        let (tx, jobs) = mpsc::sync_channel(1);
        let (replies, rx) = mpsc::sync_channel(1);
        let mut admin = Admin {
            tx,
            rx,
            worker: None,
            unsent: None,
            state: AdminState::Preparing,
            error: None,
            busy: true,
            locked: false,
            stopping: false,
            stop_sent: false,
            cancel: rad_plugin_manager::Cancellation::default(),
            next: 1,
            operation: Some(1),
            installed: vec![],
            startup: VecDeque::new(),
            active: None,
            change_key: None,
            retained: None,
            demanded: Default::default(),
            attempted: Default::default(),
        };
        let config = Configuration::prepare_new(&IdentityRegistry::new(703).unwrap()).unwrap();
        let mut actor = Authority::new(config, 1, std::num::NonZeroU128::new(705).unwrap());
        let before = actor.editor_view(Origin::Editor).unwrap();
        let mut host = Host::default();
        replies
            .send(Reply::Prepared(Ok(Box::new(prepared)), true))
            .unwrap();
        admin.poll(&mut actor, &mut host, 0);
        let Job::Commit(ticket) = jobs.try_recv().unwrap() else {
            panic!("one commit continuation");
        };
        let mut held = Some(ticket);
        for _ in 0..3 {
            admin.poll(&mut actor, &mut host, 0);
        }
        assert!(admin.busy);
        assert_eq!(
            actor.plugin_change_allowed("org.rad.fixture"),
            Err(PluginError::PublicationPending)
        );
        let request = PluginAdmin {
            operation: rad_protocol::PluginOperation::Uninstall,
            plugin_id: Some("org.rad.fixture".into()),
            source: None,
        };
        assert_eq!(
            admin.submit(Origin::Editor, request.clone(), &actor),
            Err("resource_exhausted")
        );
        assert!(matches!(jobs.try_recv(), Err(mpsc::TryRecvError::Empty)));
        let outcome = if uncertain {
            CommitOutcome::Uncertain(Error::Io)
        } else {
            CommitOutcome::NotApplied(Error::Io)
        };
        replies
            .send(Reply::Committed(key, outcome, Ok(vec![])))
            .unwrap();
        for _ in 0..3 {
            admin.poll(&mut actor, &mut host, 0);
        }
        assert_eq!(actor.editor_view(Origin::Editor).unwrap(), before);
        assert_eq!(host.resources(), (0, 0));
        assert!(admin.installed.is_empty());
        assert!(
            matches!(jobs.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "no autoretry/abort/spawn"
        );
        if uncertain {
            assert!(admin.busy && admin.locked);
            assert!(admin.active.as_ref().unwrap().permit.is_some());
            assert_eq!(admin.status(&host)["state"], "uncertain");
            assert_eq!(admin.status(&host)["locked"], true);
            assert_eq!(
                actor.plugin_change_allowed("org.rad.fixture"),
                Err(PluginError::PublicationPending)
            );
            assert_eq!(
                admin.submit(Origin::Editor, request, &actor),
                Err("resource_exhausted")
            );
        } else {
            assert!(!admin.busy && !admin.locked);
            assert!(admin.active.is_none());
            assert_eq!(
                admin.status(&host)["state"],
                "failed",
                "NotApplied is not success"
            );
            assert!(actor.plugin_change_allowed("org.rad.fixture").is_ok());
        }
        admin.shutdown();
        admin.poll(&mut actor, &mut host, 0);
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Stop)),
            "uncertain publication must not trap shutdown"
        );
        // Test-only cleanup: simulated reply did not commit/change disk.
        manager.abort(&mut held).unwrap();
        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn startup_publication_wait_is_retained_and_retried_next_turn() {
    use rad_authority::{Command, Effect, Outcome};
    use rad_core::configuration::Edit;
    let config = Configuration::prepare_new(&IdentityRegistry::new(171).unwrap()).unwrap();
    let folder = config.configuration().data().principal;
    let mut actor = Authority::new(config, 1, std::num::NonZeroU128::new(172).unwrap());
    let ticket = actor.reserve(1, Origin::Editor).unwrap();
    let effects = actor.submit(
        ticket,
        Command::Edits(vec![Edit::CreateButton {
            folder,
            position: 0,
            name: "Pending".into(),
        }]),
    );
    let commit = effects
        .iter()
        .find_map(|e| {
            if let Effect::Persist { id, .. } = e {
                Some(*id)
            } else {
                None
            }
        })
        .unwrap();
    let mut manifest: Manifest = serde_json::from_slice(include_bytes!(
        "../../../rad-plugin-host/fixture/manifest-v1.json"
    ))
    .unwrap();
    manifest.lifecycle = rad_plugin_contract::PluginLifecycle::AlwaysLoaded;
    let (tx, jobs) = mpsc::sync_channel(1);
    let (replies, rx) = mpsc::sync_channel(1);
    let mut admin = Admin {
        tx,
        rx,
        worker: None,
        unsent: None,
        state: AdminState::Idle,
        error: None,
        busy: false,
        locked: false,
        stopping: false,
        stop_sent: false,
        cancel: Default::default(),
        next: 0,
        operation: None,
        installed: vec![],
        startup: VecDeque::from([manifest]),
        active: None,
        change_key: None,
        retained: None,
        demanded: Default::default(),
        attempted: Default::default(),
    };
    let mut host = Host::default();
    admin.poll(&mut actor, &mut host, 0);
    assert_eq!(admin.error, Some("publication_pending"));
    assert_eq!(admin.startup.len(), 1);
    assert!(jobs.try_recv().is_err());
    actor.complete(
        commit,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    admin.poll(&mut actor, &mut host, 0);
    let Ok(Job::Spawn { ticket, .. }) = jobs.try_recv() else {
        panic!("startup retries admission");
    };
    let binary = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("rad-plugin-fixture-v1");
    let prepared = ticket.spawn(std::process::Command::new(binary));
    let ticket = actor.reserve(2, Origin::Editor).unwrap();
    let effects = actor.submit(
        ticket,
        Command::Edits(vec![Edit::CreateButton {
            folder,
            position: 1,
            name: "Concurrent publication".into(),
        }]),
    );
    let commit = effects
        .iter()
        .find_map(|e| {
            if let Effect::Persist { id, .. } = e {
                Some(*id)
            } else {
                None
            }
        })
        .unwrap();
    replies.send(Reply::Spawned(Box::new(prepared))).unwrap();
    admin.poll(&mut actor, &mut host, 0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while host.state("org.rad.fixture") == Some(State::Starting) {
        host.pump(0).unwrap();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    admin.poll(&mut actor, &mut host, 0);
    let preserved_ready = host.state("org.rad.fixture") == Some(State::Ready);
    let pending_error = admin.error;
    actor.complete(
        commit,
        Outcome::Applied {
            revision: 3,
            changed: true,
        },
    );
    admin.poll(&mut actor, &mut host, 0);
    let activated = actor.plugin_manifests().count() == 1 && !admin.busy;
    host.shutdown();
    while host.resources().0 > 0 {
        host.pump(0).unwrap();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        preserved_ready,
        "publication must not kill handshaken process"
    );
    assert_eq!(
        pending_error, None,
        "waking selected metadata need not wait for unrelated persistence"
    );
    assert!(
        activated,
        "same Ready generation registers next turn after publication"
    );
}

#[test]
fn startup_registers_plugin_and_rejects_only_buttons_with_incompatible_configuration() {
    use rad_authority::{Command, Effect, Rejection};
    use rad_core::{
        configuration::{ActionKind, Edit, ExternalType},
        execution::{ExecutionError, Target},
    };
    // Reachable by restoring a backup or undoing across a version change: the second
    // button keeps parameters that the installed version does not understand.
    let mut config = Configuration::prepare_new(&IdentityRegistry::new(181).unwrap()).unwrap();
    let folder = config.configuration().data().principal;
    for (position, parameters) in [
        r#"{"schema_version":1,"values":{"mode":"echo"}}"#,
        r#"{"schema_version":1,"values":{"mode":"echo","new":true}}"#,
    ]
    .into_iter()
    .enumerate()
    {
        config = config
            .configuration()
            .prepare_edit(
                config.registry(),
                Origin::Editor,
                Edit::CreateButton {
                    folder,
                    position,
                    name: format!("Button {position}"),
                },
            )
            .unwrap();
        let button = config.configuration().data().buttons[position]
            .binding
            .identity();
        config = config
            .configuration()
            .prepare_edit(
                config.registry(),
                Origin::Editor,
                Edit::AddAction {
                    button,
                    kind: ActionKind::External {
                        type_key: ExternalType::new("org.rad.fixture".into(), "run".into())
                            .unwrap(),
                        parameters: parameters.as_bytes().to_vec(),
                    },
                },
            )
            .unwrap();
    }
    let buttons: Vec<_> = config
        .configuration()
        .data()
        .buttons
        .iter()
        .map(|b| b.binding.identity())
        .collect();
    let mut actor = Authority::new(config, 1, std::num::NonZeroU128::new(182).unwrap());
    let manifest: Manifest = serde_json::from_slice(include_bytes!(
        "../../../rad-plugin-host/fixture/manifest-v1.json"
    ))
    .unwrap();
    let (tx, jobs) = mpsc::sync_channel(1);
    let (replies, rx) = mpsc::sync_channel(1);
    let mut admin = Admin {
        tx,
        rx,
        worker: None,
        unsent: None,
        state: AdminState::Idle,
        error: None,
        busy: false,
        locked: false,
        stopping: false,
        stop_sent: false,
        cancel: Default::default(),
        next: 0,
        operation: None,
        installed: vec![],
        startup: VecDeque::from([manifest]),
        active: None,
        change_key: None,
        retained: None,
        demanded: Default::default(),
        attempted: Default::default(),
    };
    let mut host = Host::default();
    admin.poll(&mut actor, &mut host, 0);
    let Ok(Job::Spawn { ticket, .. }) = jobs.try_recv() else {
        panic!(
            "one incompatible button must not keep the plugin from starting: {:?}",
            admin.error
        );
    };
    let binary = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("rad-plugin-fixture-v1");
    let prepared = ticket.spawn(std::process::Command::new(binary));
    replies.send(Reply::Spawned(Box::new(prepared))).unwrap();
    admin.poll(&mut actor, &mut host, 0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while host.state("org.rad.fixture") == Some(State::Starting) {
        host.pump(0).unwrap();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    admin.poll(&mut actor, &mut host, 0);
    let registered = actor.plugin_manifests().count() == 1 && !admin.busy;
    let error = admin.error;
    let mut run = |button| {
        let ticket = actor.reserve(1, Origin::Editor).unwrap();
        actor.submit(
            ticket,
            Command::Run {
                target: Target::Button(button),
            },
        )
    };
    let incompatible = run(buttons[1]);
    let compatible = run(buttons[0]);
    host.shutdown();
    while host.resources().0 > 0 {
        host.pump(0).unwrap();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(registered, "startup registers the selected version");
    assert_eq!(error, None);
    assert!(
        matches!(
            incompatible.as_slice(),
            [Effect::Rejected {
                reason: Rejection::Execution(ExecutionError::ExecutorUnavailable),
                ..
            }]
        ),
        "the incompatible button stays rejected"
    );
    assert!(
        compatible
            .iter()
            .any(|e| matches!(e, Effect::Accepted { .. })),
        "the compatible button keeps working"
    );
}
