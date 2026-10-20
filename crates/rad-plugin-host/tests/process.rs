use rad_plugin_contract::*;
use rad_plugin_host::{Host, State};
use std::{
    process::Command,
    time::{Duration, Instant},
};
fn manifest(major: u32) -> Manifest {
    Manifest {
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
    }
}
fn definition(id: &str) -> Definition {
    Definition {
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
                    ]
                    .map(String::from)
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
fn one_real_process_handshakes_and_shares_correlated_invocations() {
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
    let a = host
        .invoke("org.rad.fixture", g, "run", parameters("echo"), 5000)
        .unwrap();
    let b = host
        .invoke("org.rad.fixture", g, "run", parameters("fail"), 5000)
        .unwrap();
    assert_eq!(host.resources(), (1, 2));
    assert!(b.request > a.request);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut replies = vec![];
    while replies.len() < 2 {
        assert!(Instant::now() < deadline);
        host.pump(0).unwrap();
        while let Some(r) = host.take_completion() {
            replies.push(r);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(replies[0], (a, Outcome::Success));
    assert_eq!(
        replies[1],
        (
            b,
            Outcome::Failure {
                code: FailureCode::Unavailable
            }
        )
    );
    assert_eq!(host.resources(), (1, 0));
    assert_eq!(host.state("org.rad.fixture"), Some(State::Ready));
    stop(&mut host, 0);
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
    stop(&mut host, 10);
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
fn cross_host_prepared_cannot_consume_or_release_another_setup_reservation() {
    for spawn_fails in [true, false] {
        let mut a = Host::default();
        let mut b = Host::default();
        let ticket_a = a.reserve_launch(manifest(1), 0).unwrap();
        let ticket_b = b.reserve_launch(manifest(2), 0).unwrap();
        assert_eq!(a.bindings().next().unwrap().1, 1);
        assert_eq!(b.bindings().next().unwrap().1, 1);
        // Empty executable deterministically fails without searching PATH.
        let command = if spawn_fails {
            Command::new("")
        } else {
            Command::new(env!("CARGO_BIN_EXE_rad-plugin-fixture-v1"))
        };
        let mut foreign = Some(ticket_a.spawn(command));
        assert_eq!(
            b.attach(&mut foreign, 1),
            Err(rad_plugin_host::Error::Invalid)
        );
        assert!(foreign.is_some());
        assert_eq!(b.resources(), (1, 0));
        assert_eq!(b.state("org.rad.fixture"), Some(State::Starting));
        assert_eq!(b.bindings().next().unwrap().0, &manifest(2));
        assert_eq!(
            b.remove("org.rad.fixture"),
            Err(rad_plugin_host::Error::Residual)
        );
        // Rejection did not advance B's clock or consume either legitimate ticket.
        let mut own = Some(ticket_b.spawn(Command::new("")));
        assert_eq!(b.attach(&mut own, 0), Ok(1));
        assert!(own.is_none());
        b.remove("org.rad.fixture").unwrap();
        assert_eq!(a.attach(&mut foreign, 0), Ok(1));
        assert!(foreign.is_none());
        stop(&mut a, 0);
        a.remove("org.rad.fixture").unwrap();
    }
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
    assert_eq!(host.take_completion().unwrap().0, old);
    assert_eq!(host.resources(), (1, 1));
    assert!(
        !host.idle("org.rad.fixture"),
        "timed-out action retains demand until late reply"
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

#[test]
fn readiness_wait_uses_actual_expiry_not_periodic_polling() {
    let mut h = Host::default();
    h.activate(
        Command::new(env!("CARGO_BIN_EXE_rad-plugin-fixture-v1")),
        manifest(1),
        0,
    )
    .unwrap();
    until(&mut h, |h| h.state("org.rad.fixture") == Some(State::Ready));
    assert_eq!(h.next_poll(), None);
    let c = h
        .subscribe("org.rad.fixture", "status", parameters("echo"), 1000)
        .unwrap();
    until(&mut h, |h| {
        matches!(
            h.visual("org.rad.fixture", c),
            Some(VisualState::Current { .. })
        )
    });
    assert_eq!(h.next_poll(), Some(1000));
    h.pump(1000).unwrap();
    assert_eq!(h.next_poll(), None);
    stop(&mut h, 1000);
}

#[test]
fn ordinary_retirement_reaps_before_releasing_without_crash_diagnostic() {
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
    host.retire("org.rad.fixture");
    assert_eq!(host.resources().0, 1);
    assert_eq!(
        host.remove("org.rad.fixture"),
        Err(rad_plugin_host::Error::Residual)
    );
    until(&mut host, |h| {
        h.state("org.rad.fixture") == Some(State::Reaped)
    });
    assert!(
        host.diagnostics("org.rad.fixture").is_empty(),
        "ordinary demand removal is not a crash"
    );
    host.remove("org.rad.fixture").unwrap();
}
