use rad_authority::{Authority, Command, Effect};
use rad_core::{
    configuration::*,
    execution::{ExecutionStatus, Target},
    identity::IdentityRegistry,
};
use rad_launcher::{Launcher, monotonic_ms};
use rad_runtime::native::Native;
use rad_transport::Transport;
use std::{
    num::NonZeroU128,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};
struct Directory(std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn admitted_actor_run_reaches_native_callback_and_helper_exit_wakes_the_same_wait() {
    let directory = Directory(
        std::env::temp_dir().join(format!("rad-native-integration-{}", std::process::id())),
    );
    for sub in [
        "",
        "home",
        "data/applications",
        "system",
        "config",
        "cache",
        "runtime",
    ] {
        std::fs::create_dir_all(directory.0.join(sub)).unwrap();
    }
    std::fs::set_permissions(
        directory.0.join("runtime"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let helper =
        std::env::var("RAD_TEST_LAUNCHER_HELPER").expect("runner builds private helper fixture");
    let fixture =
        std::env::var("RAD_TEST_LAUNCHER_FIXTURE").expect("runner builds benign Rust fixture app");
    let output = directory.0.join("invoked.json");
    std::fs::write(directory.0.join("data/applications/test.desktop"), format!("[Desktop Entry]\nType=Application\nName=Private fixture\nExec=\"{fixture}\" literal\nTerminal=false\n")).unwrap();
    let mut command = std::process::Command::new(helper);
    command
        .env_clear()
        .env("HOME", directory.0.join("home"))
        .env("XDG_DATA_HOME", directory.0.join("data"))
        .env("XDG_DATA_DIRS", directory.0.join("system"))
        .env("XDG_CONFIG_HOME", directory.0.join("config"))
        .env("XDG_CACHE_HOME", directory.0.join("cache"))
        .env("XDG_RUNTIME_DIR", directory.0.join("runtime"))
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent-rad-private-bus",
        )
        .env("RAD_FIXTURE_OUTPUT", &output)
        .env("PATH", "/nonexistent-rad-private-bin");
    let initial = Configuration::prepare_new(&IdentityRegistry::new(754).unwrap()).unwrap();
    let configured = initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: initial.configuration().data().principal,
                position: 0,
                name: "fixture".into(),
            },
        )
        .unwrap();
    let button = configured.configuration().data().buttons[0]
        .binding
        .identity();
    let configured = configured
        .configuration()
        .prepare_edit(
            configured.registry(),
            Origin::Editor,
            Edit::AddAction {
                button,
                kind: ActionKind::Launcher(DesktopEntry::new("test.desktop".into()).unwrap()),
            },
        )
        .unwrap();
    let mut actor = Authority::new_at(
        configured,
        1,
        NonZeroU128::new(121).unwrap(),
        monotonic_ms().unwrap(),
    );
    let mut transport = Transport::bind(&directory.0.join("runtime"), 121).unwrap();
    let mut native = Native::attach(Launcher::spawn(command).unwrap(), &transport).unwrap();
    // Catalogue IO is a worker concern; readiness, not a fixed sleep loop, drives it.
    let end = Instant::now() + Duration::from_secs(3);
    while !native.launcher.is_available() {
        native.pump(&mut actor, &transport).unwrap();
        assert!(Instant::now() < end, "native catalogue must complete");
        if !native.launcher.is_available() {
            transport
                .wait_readiness(Duration::from_millis(100))
                .unwrap();
        }
    }
    let ticket = actor.reserve(1, Origin::Editor).unwrap();
    let request = ticket.request();
    let effects = actor.submit(
        ticket,
        Command::Run {
            target: Target::Button(button),
        },
    );
    let mut execution = None;
    for effect in native.route(&mut actor, effects).unwrap() {
        match effect {
            Effect::Accepted { execution: id, .. } => execution = Some(id),
            Effect::Dispatch(_) => panic!("native dispatch must cross the worker port"),
            _ => {}
        }
    }
    actor.finish(request);
    let execution = execution.unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        native.pump(&mut actor, &transport).unwrap();
        if actor.result(Origin::Editor, execution).unwrap().status == ExecutionStatus::Succeeded {
            break;
        }
        assert!(
            Instant::now() < end,
            "native callback must complete original execution"
        );
        transport
            .wait_readiness(Duration::from_millis(100))
            .unwrap();
    }
    let end = Instant::now() + Duration::from_secs(2);
    // Native callback is acceptance, not application readiness or file-write completion.
    while std::fs::read_to_string(&output).ok().as_deref() != Some("[\"literal\"]") {
        assert!(
            Instant::now() < end,
            "private fixture must finish its output"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    native.launcher.shutdown().unwrap();
    let start = Instant::now();
    transport.wait_readiness(Duration::from_secs(2)).unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "own helper exit must wake without SIGCHLD polling"
    );
    native.pump(&mut actor, &transport).unwrap();
    assert_eq!(native.launcher.process_id(), None);
    let idle = Instant::now();
    transport.wait_readiness(Duration::from_millis(60)).unwrap();
    assert!(
        idle.elapsed() >= Duration::from_millis(40),
        "reaped helper must not leave a permanently readable pidfd busy loop"
    );
}
