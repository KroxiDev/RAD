use rad_plugin_contract::*;
use rad_plugin_host::{Host, State};
use std::{
    process::Command,
    time::{Duration, Instant},
};
fn manifest(major: u32) -> Manifest {
    Manifest {
        resources: vec![],
        capabilities: rad_plugin_contract::SUPPORTED_CAPABILITIES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        settings: None,
        lifecycle: Default::default(),
        contract: 1,
        id: "org.rad.fixture".into(),
        version: Version {
            major,
            minor: 0,
            patch: 0,
        },
        host_min: 1,
        host_max: 1,
        targets: vec!["x86_64-unknown-linux-gnu".into()],
        actions: vec![definition("run")],
        visuals: vec![definition("status")],
        translations: Default::default(),
    }
}
fn definition(id: &str) -> Definition {
    Definition {
        gestures: vec![rad_plugin_contract::Gesture::Press],
        response_timeout: None,
        id: id.into(),
        label: id.into(),
        schema: Schema {
            version: 1,
            fields: vec![Field {
                id: "mode".into(),
                label: "Mode".into(),
                required: true,
                kind: FieldKind::Choice {
                    options: [
                        "echo",
                        "fail",
                        "slow",
                        "crash",
                        "late",
                        "flood",
                        "environment",
                        "child",
                        "child_exit",
                    ]
                    .map(Into::into)
                    .to_vec(),
                },
            }],
        },
        credential_slots: vec![],
    }
}
fn parameters(mode: &str) -> Parameters {
    serde_json::from_value(serde_json::json!({"schema_version":1,"values":{"mode":mode}})).unwrap()
}
fn until(host: &mut Host, condition: impl Fn(&Host) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition(host) {
        assert!(Instant::now() < deadline, "fixture watchdog");
        host.pump(0).unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn running_timeout_cycles_keep_slots_until_reap_and_never_replay() {
    let mut host = Host::default();
    let mut last = 0;
    for cycle in 0..6 {
        let now = cycle as u64 * 10;
        let g = host
            .activate(
                Command::new(env!("CARGO_BIN_EXE_rad-plugin-fixture-v1")),
                manifest(1),
                now,
            )
            .unwrap();
        assert!(g > last);
        last = g;
        let deadline = Instant::now() + Duration::from_secs(3);
        while host.state("org.rad.fixture") != Some(State::Ready) {
            assert!(Instant::now() < deadline);
            host.pump(now).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        let visual = host
            .subscribe("org.rad.fixture", "status", parameters("echo"), 1000)
            .unwrap();
        let key = host
            .invoke("org.rad.fixture", g, "run", parameters("slow"), now + 10)
            .unwrap();
        // Deadline changes status, not ownership. No pump/reap is needed to observe admission cap.
        for _ in 1..rad_plugin_host::MAX_PLUGIN_JOBS {
            host.invoke("org.rad.fixture", g, "run", parameters("slow"), now + 10)
                .unwrap();
        }
        assert_eq!(
            host.invoke("org.rad.fixture", g, "run", parameters("echo"), now + 10),
            Err(rad_plugin_host::Error::Capacity)
        );
        assert_eq!(
            host.remove("org.rad.fixture"),
            Err(rad_plugin_host::Error::Residual)
        );
        // A real worker-side marker proves the first action reached plugin code;
        // do not mistake an unsent queued request for a running residual.
        while !matches!(host.visual("org.rad.fixture",visual),Some(VisualState::Current{content}) if content.text.as_deref()==Some("running"))
        {
            assert!(
                Instant::now() < deadline,
                "worker did not announce action entry"
            );
            host.pump(now).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        host.pump(now + 10).unwrap();
        assert_eq!(host.resources().1, rad_plugin_host::MAX_PLUGIN_JOBS);
        assert!(
            host.invoke("org.rad.fixture", g, "run", parameters("echo"), now + 20)
                .is_err()
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while host.resources().0 != 0 {
            assert!(Instant::now() < deadline);
            host.pump(now + 10).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut n = 0;
        while let Some((c, outcome)) = host.take_completion() {
            assert_eq!(c.generation, key.generation);
            assert!(matches!(outcome, Outcome::Failure { .. }));
            n += 1;
        }
        assert_eq!(n, rad_plugin_host::MAX_PLUGIN_JOBS);
        assert_eq!(host.resources(), (0, 0));
        host.remove("org.rad.fixture").unwrap();
    }
}
#[test]
fn actual_bad_identity_and_unsolicited_late_results_fail_closed() {
    let mut host = Host::default();
    host.activate(
        Command::new(env!("CARGO_BIN_EXE_rad-plugin-fixture-v2")),
        manifest(1),
        0,
    )
    .unwrap();
    until(&mut host, |h| {
        h.state("org.rad.fixture") == Some(State::Reaped)
    });
    assert!(
        host.diagnostics("org.rad.fixture")
            .contains(&rad_plugin_host::Diagnostic::Protocol)
    );
    host.remove("org.rad.fixture").unwrap();
    let g = host
        .activate(
            Command::new(env!("CARGO_BIN_EXE_rad-plugin-fixture-v2")),
            manifest(2),
            0,
        )
        .unwrap();
    until(&mut host, |h| {
        h.state("org.rad.fixture") == Some(State::Ready)
    });
    let c = host
        .invoke("org.rad.fixture", g, "run", parameters("late"), 5000)
        .unwrap();
    until(&mut host, |h| {
        h.state("org.rad.fixture") == Some(State::Reaped)
    });
    assert_eq!(
        host.take_completion(),
        Some((
            c,
            Outcome::Failure {
                code: FailureCode::Failed
            }
        ))
    );
    assert!(host.diagnostics("org.rad.fixture").len() <= 16);
}
#[test]
fn visual_expires_without_affecting_executor_and_unsubscribe_releases_slot() {
    let mut host = Host::default();
    let g = host
        .activate(
            Command::new(env!("CARGO_BIN_EXE_rad-plugin-fixture-v1")),
            manifest(1),
            0,
        )
        .unwrap();
    until(&mut host, |h| {
        h.state("org.rad.fixture") == Some(State::Ready)
    });
    let c = host
        .subscribe("org.rad.fixture", "status", parameters("echo"), 10)
        .unwrap();
    until(&mut host, |h| {
        matches!(
            h.visual("org.rad.fixture", c),
            Some(VisualState::Current { .. })
        )
    });
    host.pump(10).unwrap();
    assert!(
        matches!(
            host.visual("org.rad.fixture", c),
            Some(VisualState::Current { .. })
        ),
        "the plugin-declared validity overrides the old caller TTL"
    );
    host.pump(1000).unwrap();
    assert!(matches!(
        host.visual("org.rad.fixture", c),
        Some(VisualState::Stale { .. })
    ));
    assert_eq!(host.state("org.rad.fixture"), Some(State::Ready));
    assert!(
        host.invoke("org.rad.fixture", g, "run", parameters("echo"), 5010)
            .is_ok()
    );
    host.unsubscribe("org.rad.fixture", c).unwrap();
    assert_eq!(host.visual("org.rad.fixture", c), None);
    stop(&mut host, 1000);
}
#[test]
fn setup_ticket_reserves_capacity_before_spawn_and_timeout_cannot_free_it() {
    let mut host = Host::default();
    let ticket = host.reserve_launch(manifest(1), 0).unwrap();
    assert_eq!(host.resources().0, 1);
    assert!(
        !host.idle("org.rad.fixture"),
        "launch ticket is retained demand"
    );
    host.pump(5000).unwrap();
    assert_eq!(
        host.remove("org.rad.fixture"),
        Err(rad_plugin_host::Error::Residual)
    );
    let mut prepared =
        Some(ticket.spawn(Command::new(env!("CARGO_BIN_EXE_rad-plugin-fixture-v1"))));
    host.attach(&mut prepared, 5000).unwrap();
    assert!(prepared.is_none());
    let deadline = Instant::now() + Duration::from_secs(3);
    while host.state("org.rad.fixture") != Some(State::Reaped) {
        assert!(Instant::now() < deadline);
        host.pump(5000).unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    host.remove("org.rad.fixture").unwrap();
    assert_eq!(host.resources(), (0, 0));
}
#[test]
fn real_crash_and_flood_are_contained_and_synthetic_version_two_is_distinct() {
    for (mode, major, bin) in [
        ("crash", 1, env!("CARGO_BIN_EXE_rad-plugin-fixture-v1")),
        ("flood", 2, env!("CARGO_BIN_EXE_rad-plugin-fixture-v2")),
    ] {
        let mut h = Host::default();
        let g = h.activate(Command::new(bin), manifest(major), 0).unwrap();
        until(&mut h, |h| h.state("org.rad.fixture") == Some(State::Ready));
        let key = h
            .invoke("org.rad.fixture", g, "run", parameters(mode), 5000)
            .unwrap();
        until(&mut h, |h| {
            h.state("org.rad.fixture") == Some(State::Reaped)
        });
        let (actual, result) = h.take_completion().unwrap();
        assert_eq!(actual, key);
        if mode == "crash" {
            assert!(matches!(result, Outcome::Failure { .. }));
        }
        assert!(h.take_completion().is_none());
        assert_eq!(h.resources(), (0, 0));
        assert!(h.diagnostics("org.rad.fixture").len() <= 16);
    }
}

fn stop(host: &mut Host, now: u64) {
    host.shutdown();
    let deadline = Instant::now() + Duration::from_secs(3);
    while host.resources().0 > 0 {
        assert!(Instant::now() < deadline);
        host.pump(now).unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    while host.take_completion().is_some() {}
}

#[test]
fn timeout_retains_residual_until_late_reply_without_killing_healthy_plugin() {
    let mut host = Host::default();
    let g = host
        .activate(
            Command::new(env!("CARGO_BIN_EXE_rad-plugin-fixture-v1")),
            manifest(1),
            0,
        )
        .unwrap();
    until(&mut host, |h| {
        h.state("org.rad.fixture") == Some(State::Ready)
    });
    let old = host
        .invoke("org.rad.fixture", g, "run", parameters("slow"), 10)
        .unwrap();
    host.pump(0).unwrap();
    host.pump(10).unwrap();
    assert_eq!(host.state("org.rad.fixture"), Some(State::Ready));
    assert_eq!(
        host.last_diagnostic("org.rad.fixture"),
        Some(rad_plugin_host::Diagnostic::Deadline)
    );
    // The deadline is reported once, as a timeout and not as a failure.
    assert_eq!(
        host.take_completion(),
        Some((
            old,
            Outcome::Failure {
                code: FailureCode::TimedOut
            }
        ))
    );
    assert!(host.take_completion().is_none());
    assert_eq!(host.resources(), (1, 1));
    assert!(
        !host.idle("org.rad.fixture"),
        "timed-out action retains demand until late reply"
    );
    assert_eq!(
        host.remove("org.rad.fixture"),
        Err(rad_plugin_host::Error::Residual)
    );
    let new = host
        .invoke("org.rad.fixture", g, "run", parameters("echo"), 1000)
        .unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        host.pump(10).unwrap();
        if let Some(reply) = host.take_completion() {
            assert_eq!(reply, (new, Outcome::Success));
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(host.resources(), (1, 0));
    // The late reply was drained: no second report and nothing left to wait for.
    assert!(host.take_completion().is_none());
    assert!(host.idle("org.rad.fixture"));
    stop(&mut host, 10);
}

#[test]
fn unsubscribe_releases_local_slot_even_when_output_queue_is_full() {
    let mut host = Host::default();
    host.activate(
        Command::new(env!("CARGO_BIN_EXE_rad-plugin-fixture-v1")),
        manifest(1),
        0,
    )
    .unwrap();
    until(&mut host, |h| {
        h.state("org.rad.fixture") == Some(State::Ready)
    });
    let keys: Vec<_> = (0..32)
        .map(|_| {
            host.subscribe("org.rad.fixture", "status", parameters("echo"), 1000)
                .unwrap()
        })
        .collect();
    host.unsubscribe("org.rad.fixture", keys[0]).unwrap();
    assert_eq!(host.visual("org.rad.fixture", keys[0]), None);
    stop(&mut host, 0);
}

#[test]
fn plugin_capacity_accepts_32_and_returns_its_own_error() {
    let mut h = Host::default();
    for i in 0..32 {
        let mut m = manifest(1);
        m.id = format!("org.rad.plugin{i}");
        assert!(h.reserve_launch(m, 0).is_ok());
    }
    let mut m = manifest(1);
    m.id = "org.rad.extra".into();
    assert_eq!(
        format!("{:?}", h.reserve_launch(m, 0).err().unwrap()),
        "PluginLimit"
    );
}

#[test]
fn plugin_identity_and_allowlisted_environment_are_enforced_at_spawn() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rad-plugin-fixture-v1"));
    command
        .env_clear()
        .env("XDG_RUNTIME_DIR", "/synthetic-runtime")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/synthetic-bus")
        .env("RAD_SYNTHETIC_SECRET", "must-not-leak");
    let mut host = Host::default();
    let g = host.activate(command, manifest(1), 0).unwrap();
    until(&mut host, |h| {
        h.state("org.rad.fixture") == Some(State::Ready)
    });
    let key = host
        .invoke("org.rad.fixture", g, "run", parameters("environment"), 5000)
        .unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        host.pump(0).unwrap();
        if let Some(reply) = host.take_completion() {
            assert_eq!(reply, (key, Outcome::Success));
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    stop(&mut host, 0);
}

#[test]
fn plugin_started_under_blocked_shutdown_signals_still_terminates_on_sigterm() {
    // The runtime blocks both signals before spawning; a child inherits its spawning thread's mask.
    let mut blocked = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    let mut original = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    unsafe {
        libc::sigemptyset(&mut blocked);
        libc::sigaddset(&mut blocked, libc::SIGINT);
        libc::sigaddset(&mut blocked, libc::SIGTERM);
        assert_eq!(
            libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut original),
            0
        );
    }
    let mut host = Host::default();
    let activated = host.activate(
        Command::new(env!("CARGO_BIN_EXE_rad-plugin-fixture-v1")),
        manifest(1),
        0,
    );
    assert_eq!(
        unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &original, std::ptr::null_mut()) },
        0
    );
    activated.unwrap();
    until(&mut host, |h| {
        h.state("org.rad.fixture") == Some(State::Ready)
    });
    // Per-thread list: concurrent tests spawn their plugins from other threads.
    let pid: i32 = std::fs::read_to_string("/proc/thread-self/children")
        .unwrap()
        .trim()
        .parse()
        .expect("exactly the plugin spawned by this test");
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let end = Instant::now() + Duration::from_secs(3);
    while host.state("org.rad.fixture") != Some(State::Reaped) {
        assert!(
            Instant::now() < end,
            "plugin ignored SIGTERM: it inherited the blocked shutdown signals"
        );
        host.pump(0).unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(host.resources(), (0, 0));
}

/// Pid written by the fixture's descendant modes, killed on drop so a failing test leaks nothing.
struct Descendant {
    dir: std::path::PathBuf,
    pid: Option<i32>,
}
impl Descendant {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rad-host-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir, pid: None }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rad-plugin-fixture-v1"));
        command.env("XDG_RUNTIME_DIR", &self.dir);
        command
    }
    fn wait_for_pid(&mut self, host: &mut Host) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            // The fixture may still be writing the file: an unparsable read is retried.
            if let Some(pid) = std::fs::read_to_string(self.dir.join("descendant.pid"))
                .ok()
                .and_then(|text| text.parse().ok())
            {
                self.pid = Some(pid);
                return pid;
            }
            assert!(Instant::now() < deadline, "descendant never started");
            host.pump(0).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
impl Drop for Descendant {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            // SAFETY: plain signal to a pid this test started; failure only means it is gone.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
/// Dead means gone or an unreaped zombie (the test, not the Host, may be its parent's heir).
fn is_dead(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat
            .rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')),
    }
}
fn assert_dies(pid: i32, why: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !is_dead(pid) {
        assert!(
            Instant::now() < deadline,
            "{why}: descendant {pid} survived"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn start_with_descendant(name: &str, mode: &str) -> (Host, Descendant, i32) {
    let mut descendant = Descendant::new(name);
    let mut host = Host::default();
    let g = host.activate(descendant.command(), manifest(1), 0).unwrap();
    until(&mut host, |h| {
        h.state("org.rad.fixture") == Some(State::Ready)
    });
    host.invoke("org.rad.fixture", g, "run", parameters(mode), 5000)
        .unwrap();
    let pid = descendant.wait_for_pid(&mut host);
    (host, descendant, pid)
}
#[test]
fn retiring_a_plugin_kills_the_processes_it_started() {
    let (mut host, _descendant, pid) = start_with_descendant("retire", "child");
    assert!(!is_dead(pid));
    host.retire("org.rad.fixture");
    until(&mut host, |h| {
        h.state("org.rad.fixture") == Some(State::Reaped)
    });
    assert_dies(pid, "retire");
}
#[test]
fn a_plugin_exiting_by_itself_does_not_leave_its_descendants() {
    let (mut host, _descendant, pid) = start_with_descendant("exit", "child_exit");
    until(&mut host, |h| {
        h.state("org.rad.fixture") == Some(State::Reaped)
    });
    assert_dies(pid, "plugin exit");
}
#[test]
fn dropping_the_host_kills_the_processes_its_plugins_started() {
    let (host, _descendant, pid) = start_with_descendant("drop", "child");
    assert!(!is_dead(pid));
    drop(host);
    assert_dies(pid, "host drop");
}
