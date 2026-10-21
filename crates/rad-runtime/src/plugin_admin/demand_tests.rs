use super::*;
use rad_core::{configuration::Configuration, identity::IdentityRegistry};

fn controlled() -> (Admin, mpsc::Receiver<Job>, Authority, Host) {
    let (tx, jobs) = mpsc::sync_channel(1);
    let (_, rx) = mpsc::sync_channel(1);
    let admin = Admin {
        tx,
        rx,
        worker_ended: crate::thread_end::Ended::already(),
        fetcher: fetch::Fetcher::new(Arc::new(|| {}), Box::new(|_, _| panic!("no download"))),
        unsent: None,
        state: AdminState::Idle,
        error: None,
        busy: false,
        locked: false,
        stopping: false,
        stop_sent: false,
        cancel: rad_plugin_manager::Cancellation::default(),
        next: 0,
        operation: None,
        installed: vec![],
        startup: VecDeque::new(),
        active: None,
        change_key: None,
        retained: None,
        demanded: Default::default(),
        attempted: Default::default(),
    };
    let config = Configuration::prepare_new(&IdentityRegistry::new(998).unwrap()).unwrap();
    (
        admin,
        jobs,
        Authority::new(config, 1, std::num::NonZeroU128::new(999).unwrap()),
        Host::default(),
    )
}
#[test]
fn installed_unused_metadata_is_published_without_launching() {
    let (mut admin, jobs, mut actor, mut host) = controlled();
    let manifest: Manifest = serde_json::from_slice(include_bytes!(
        "../../../rad-plugin-host/fixture/manifest-v1.json"
    ))
    .unwrap();
    admin.startup.push_back(manifest);
    admin.poll(&mut actor, &mut host, 0);
    assert_eq!(
        host.resources(),
        (0, 0),
        "unused plugin must not reserve a process"
    );
    assert!(
        jobs.try_recv().is_err(),
        "metadata publication must not spawn"
    );
    assert_eq!(actor.plugin_manifests().count(), 1);
}

fn tick(admin: &mut Admin, jobs: &mpsc::Receiver<Job>, actor: &mut Authority, host: &mut Host) {
    host.pump(0).unwrap();
    admin.poll(actor, host, 0);
    if let Ok(Job::Spawn { ticket, .. }) = jobs.try_recv() {
        let binary = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("rad-plugin-fixture-v1");
        admin.handle_reply(
            Reply::Spawned(Box::new(ticket.spawn(std::process::Command::new(binary)))),
            actor,
            host,
            0,
        );
    }
}
fn until(
    admin: &mut Admin,
    jobs: &mpsc::Receiver<Job>,
    actor: &mut Authority,
    host: &mut Host,
    predicate: impl Fn(&Authority, &Host) -> bool,
) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !predicate(actor, host) {
        tick(admin, jobs, actor, host);
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
#[test]
fn demand_reuses_one_process_and_reappearing_during_stop_waits_for_reap() {
    let (mut admin, jobs, mut actor, mut host) = controlled();
    let manifest: Manifest = serde_json::from_slice(include_bytes!(
        "../../../rad-plugin-host/fixture/manifest-v1.json"
    ))
    .unwrap();
    admin.startup.push_back(manifest);
    admin.poll(&mut actor, &mut host, 0);
    let selection = actor.plugin_manifests().next().unwrap().1;
    admin.set_extra_demand(["org.rad.fixture".into()].into());
    until(&mut admin, &jobs, &mut actor, &mut host, |a, _| {
        a.plugin_generation("org.rad.fixture").is_some()
    });
    let first = actor.plugin_generation("org.rad.fixture").unwrap();
    for _ in 0..10 {
        tick(&mut admin, &jobs, &mut actor, &mut host);
    }
    assert_eq!(host.resources().0, 1);
    assert_eq!(actor.plugin_generation("org.rad.fixture"), Some(first));
    admin.set_extra_demand(Default::default());
    admin.poll(&mut actor, &mut host, 0);
    assert!(host.stopped_normally("org.rad.fixture"));
    assert_eq!(host.resources().0, 1, "kill is not proof of reap");
    admin.set_extra_demand(["org.rad.fixture".into()].into());
    admin.poll(&mut actor, &mut host, 0);
    assert_eq!(host.resources().0, 1);
    until(&mut admin, &jobs, &mut actor, &mut host, |a, _| {
        a.plugin_generation("org.rad.fixture")
            .is_some_and(|g| g > first)
    });
    assert_eq!(actor.plugin_manifests().next().unwrap().1, selection);
    assert!(host.diagnostics("org.rad.fixture").is_empty());
    admin.set_extra_demand(Default::default());
    until(&mut admin, &jobs, &mut actor, &mut host, |_, h| {
        h.state("org.rad.fixture").is_none()
    });
    assert_eq!(host.resources(), (0, 0));
}

#[test]
fn failed_continuous_demand_is_latched_until_an_explicit_new_selection() {
    let (mut admin, jobs, mut actor, mut host) = controlled();
    let manifest: Manifest = serde_json::from_slice(include_bytes!(
        "../../../rad-plugin-host/fixture/manifest-v1.json"
    ))
    .unwrap();
    admin.set_extra_demand(["org.rad.fixture".into()].into());
    admin.startup.push_back(manifest.clone());
    until(&mut admin, &jobs, &mut actor, &mut host, |a, _| {
        a.plugin_generation("org.rad.fixture").is_some()
    });
    let first = actor.plugin_generation("org.rad.fixture").unwrap();
    let selection = actor.plugin_manifests().next().unwrap().1;
    let key = host
        .invoke(
            "org.rad.fixture",
            first,
            "run",
            serde_json::from_value(json!({"schema_version":1,"values":{"mode":"crash"}})).unwrap(),
            1000,
        )
        .unwrap();
    until(&mut admin, &jobs, &mut actor, &mut host, |_, h| {
        h.state("org.rad.fixture") == Some(State::Reaped)
    });
    actor.plugin_failed("org.rad.fixture", first);
    assert_eq!(host.take_completion().unwrap().0, key);
    for _ in 0..20 {
        tick(&mut admin, &jobs, &mut actor, &mut host);
    }
    assert_eq!(host.resources().0, 0);
    assert_eq!(actor.plugin_generation("org.rad.fixture"), Some(first));
    admin.set_extra_demand(Default::default());
    tick(&mut admin, &jobs, &mut actor, &mut host);
    admin.set_extra_demand(["org.rad.fixture".into()].into());
    for _ in 0..10 {
        tick(&mut admin, &jobs, &mut actor, &mut host);
    }
    assert_eq!(
        host.resources().0,
        0,
        "returning demand must not restart a real failure"
    );
    assert!(!actor.plugin_available("org.rad.fixture", selection));
    // The same metadata selected explicitly is a new repair attempt; wake is not a selection.
    actor
        .register_plugin(manifest, rad_plugin_host::TARGET)
        .unwrap();
    assert_ne!(actor.plugin_manifests().next().unwrap().1, selection);
    until(&mut admin, &jobs, &mut actor, &mut host, |a, _| {
        a.plugin_generation("org.rad.fixture")
            .is_some_and(|g| g > first)
    });
    host.shutdown();
    until(&mut admin, &jobs, &mut actor, &mut host, |_, h| {
        h.resources().0 == 0
    });
}

#[test]
fn action_admitted_during_normal_stop_waits_for_reap_and_invokes_once() {
    use rad_authority::{Command, Effect};
    use rad_core::configuration::{ActionKind, Edit, ExternalType};
    use rad_core::execution::{Completion, Target};
    let (mut admin, jobs, _, host) = controlled();
    let mut config = Configuration::prepare_new(&IdentityRegistry::new(992).unwrap()).unwrap();
    let root = config.configuration().data().principal;
    for edit in [
        Edit::CreateButton {
            folder: root,
            position: 0,
            name: "Run".into(),
        },
        Edit::CreateFolder {
            parent: root,
            position: 1,
            name: "Empty".into(),
        },
    ] {
        config = config
            .configuration()
            .prepare_edit(config.registry(), Origin::Editor, edit)
            .unwrap();
    }
    let button = config.configuration().data().buttons[0].binding.identity();
    let empty = config
        .configuration()
        .data()
        .folders
        .iter()
        .find(|f| f.binding.identity() != root)
        .unwrap()
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
                    type_key: ExternalType::new("org.rad.fixture".into(), "run".into()).unwrap(),
                    parameters: br#"{"schema_version":1,"values":{"mode":"echo"}}"#.to_vec(),
                },
            },
        )
        .unwrap();
    let mut actor = Authority::new(config, 1, std::num::NonZeroU128::new(993).unwrap());
    let mut ports = crate::plugin_dispatch::Plugins::default();
    ports.host = host;
    admin.startup.push_back(
        serde_json::from_slice(include_bytes!(
            "../../../rad-plugin-host/fixture/manifest-v1.json"
        ))
        .unwrap(),
    );
    until(&mut admin, &jobs, &mut actor, &mut ports.host, |a, _| {
        a.plugin_generation("org.rad.fixture").is_some()
    });
    let first = actor.plugin_generation("org.rad.fixture").unwrap();
    actor.navigate(Origin::Editor, empty).unwrap();
    admin.poll(&mut actor, &mut ports.host, 0);
    assert!(ports.host.stopped_normally("org.rad.fixture"));
    let ticket = actor.reserve(1, Origin::Editor).unwrap();
    let effects = actor.submit(
        ticket,
        Command::Run {
            target: Target::Button(button),
        },
    );
    let intent = effects
        .into_iter()
        .find_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d)
            } else {
                None
            }
        })
        .expect("admitted outside active folder");
    ports
        .submit(&actor, &intent)
        .expect("new action waits rather than dispatching to retiring generation");
    assert_eq!(ports.host.resources().0, 1, "retiring child still owned");
    let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let completed = loop {
        tick(&mut admin, &jobs, &mut actor, &mut ports.host);
        let completed = ports.pump(&mut actor, 0).unwrap();
        if !completed.is_empty() {
            break completed;
        }
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    assert_eq!(completed, [(intent.id, Completion::Succeeded)]);
    assert!(actor.plugin_generation("org.rad.fixture").unwrap() > first);
    actor
        .job_complete(intent.id, Completion::Succeeded)
        .unwrap();
    assert!(ports.pump(&mut actor, 0).unwrap().is_empty());
    until(&mut admin, &jobs, &mut actor, &mut ports.host, |_, h| {
        h.resources().0 == 0
    });
}
