//! Dynamic options against the real `org.rad.test.options` process (major 1 with dynamic_options).
use rad_plugin_contract::*;
use rad_plugin_host::{
    Diagnostic, Error, Host, MAX_PLUGIN_JOBS, OptionsOutcome, OptionsQuery, State, TARGET,
};
use serde_json::{Value, json};
use std::{
    process::Command,
    time::{Duration, Instant},
};

const ID: &str = "org.rad.test.options";

fn manifest() -> Manifest {
    Document::new(include_bytes!("../test-plugins/options-manifest.json"))
        .unwrap()
        .manifest(HOST_API, TARGET)
        .unwrap()
}
fn launch() -> (Host, u64) {
    let mut host = Host::default();
    let generation = host
        .activate(
            Command::new(env!("CARGO_BIN_EXE_rad-plugin-options-fixture")),
            manifest(),
            0,
        )
        .unwrap();
    (host, generation)
}
fn start() -> (Host, u64) {
    let (mut host, generation) = launch();
    wait(&mut host, 0, |h| h.state(ID) == Some(State::Ready));
    (host, generation)
}
/// Pump at virtual time `now` until `condition`, with a real-time watchdog.
fn wait(host: &mut Host, now: u64, condition: impl Fn(&Host) -> bool) {
    let end = Instant::now() + Duration::from_secs(3);
    while !condition(host) {
        assert!(Instant::now() < end, "options fixture watchdog");
        host.pump(now).unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn query(scope: OptionsScope, definition: &str, field: &str, values: Value) -> OptionsQuery {
    OptionsQuery {
        scope,
        definition: definition.into(),
        field: field.into(),
        context: serde_json::from_value(json!({"schema_version":1,"values":values})).unwrap(),
    }
}
fn action(field: &str, values: Value) -> OptionsQuery {
    query(OptionsScope::Action, "pick", field, values)
}
/// Pump until this query's result is ready; results are consumed exactly once.
fn resolve(host: &mut Host, now: u64, key: Correlation) -> OptionsOutcome {
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        host.pump(now).unwrap();
        if let Some((got, outcome)) = host.take_options() {
            assert_eq!(got, key, "result of another query");
            return outcome;
        }
        assert!(Instant::now() < end, "options fixture watchdog");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn ask(host: &mut Host, generation: u64, query: OptionsQuery) -> OptionsOutcome {
    let key = host.options(ID, generation, query, 5000).unwrap();
    resolve(host, 0, key)
}
fn listed(outcome: OptionsOutcome) -> Vec<(String, String)> {
    match outcome {
        OptionsOutcome::Choices(ChoiceList::Available { options }) => {
            options.into_iter().map(|o| (o.value, o.label)).collect()
        }
        other => panic!("expected an available list, got {other:?}"),
    }
}
fn stop(host: &mut Host, now: u64) {
    host.shutdown();
    wait(host, now, |h| h.resources().0 == 0);
    while host.take_completion().is_some() || host.take_options().is_some() {}
}

fn pick(scenario: &str, group: &str) -> Parameters {
    serde_json::from_value(json!({"schema_version":1,"values":{"scenario":scenario,"group":group}}))
        .unwrap()
}
/// A full action round trip: proves the plugin is alive and has handled every earlier frame.
fn ping(host: &mut Host, g: u64, now: u64) {
    let key = host
        .invoke(
            ID,
            g,
            "pick",
            pick("normal", "gone-from-every-list"),
            now + 60_000,
        )
        .unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        host.pump(now).unwrap();
        if let Some(done) = host.take_completion() {
            assert_eq!(done, (key, Outcome::Success));
            return;
        }
        assert!(Instant::now() < end, "plugin stopped answering actions");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_late_reply_is_drained_by_its_own_correlation_and_frees_the_slot() {
    let (mut host, g) = start();
    let late = host
        .options(ID, g, action("group", json!({"scenario":"late"})), 100)
        .unwrap();
    host.pump(100).unwrap();
    assert_eq!(host.take_options(), Some((late, OptionsOutcome::TimedOut)));
    // The expired query still holds one of the two slots until its reply is drained.
    let slow = host
        .options(ID, g, action("group", json!({"scenario":"slow"})), 60_000)
        .unwrap();
    let normal = || action("group", json!({"scenario":"normal"}));
    assert_eq!(host.options(ID, g, normal(), 60_000), Err(Error::Capacity));
    let end = Instant::now() + Duration::from_secs(3);
    let next = loop {
        host.pump(100).unwrap();
        match host.options(ID, g, normal(), 60_000) {
            Err(Error::Capacity) => assert!(Instant::now() < end, "late reply never drained"),
            other => break other.unwrap(),
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    // The drained reply completed neither the slow query nor the new one.
    assert_eq!(listed(resolve(&mut host, 100, next)).len(), 2);
    assert_eq!(host.take_options(), None);
    assert_eq!(host.state(ID), Some(State::Ready));
    host.pump(60_000).unwrap();
    assert_eq!(host.take_options(), Some((slow, OptionsOutcome::TimedOut)));
    assert_eq!(host.take_options(), None);
    stop(&mut host, 60000);
}

#[test]
fn two_queries_per_plugin_use_no_action_slot_and_a_refused_third_sends_nothing() {
    let (mut host, g) = start();
    // All 16 action slots are taken before any query: queries do not need them.
    for _ in 0..MAX_PLUGIN_JOBS {
        host.invoke(ID, g, "pick", pick("normal", "a"), 60_000)
            .unwrap();
    }
    assert_eq!(
        host.invoke(ID, g, "pick", pick("normal", "a"), 60_000),
        Err(Error::Capacity)
    );
    for _ in 0..2 {
        host.options(ID, g, action("group", json!({"scenario":"slow"})), 60_000)
            .unwrap();
    }
    assert_eq!(
        host.resources(),
        (1, MAX_PLUGIN_JOBS),
        "queries are not jobs"
    );
    // A `crash` query would kill the plugin if it reached it.
    let refused = action("group", json!({"scenario":"crash"}));
    assert_eq!(host.options(ID, g, refused, 60_000), Err(Error::Capacity));
    let mut done = 0;
    let end = Instant::now() + Duration::from_secs(3);
    while done < MAX_PLUGIN_JOBS {
        assert!(Instant::now() < end, "actions blocked by pending queries");
        host.pump(0).unwrap();
        while let Some((_, outcome)) = host.take_completion() {
            assert_eq!(outcome, Outcome::Success);
            done += 1;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    ping(&mut host, g, 0);
    assert_eq!(host.state(ID), Some(State::Ready));
    stop(&mut host, 0);
}

#[test]
fn a_plugin_that_dies_during_a_query_leaves_that_list_unavailable() {
    let (mut host, g) = start();
    // An expired query whose timeout was already reported must not leave a result behind.
    let slow = host
        .options(ID, g, action("group", json!({"scenario":"slow"})), 100)
        .unwrap();
    host.pump(100).unwrap();
    assert_eq!(host.take_options(), Some((slow, OptionsOutcome::TimedOut)));
    let key = host
        .options(ID, g, action("group", json!({"scenario":"crash"})), 60_000)
        .unwrap();
    assert_eq!(
        resolve(&mut host, 100, key),
        OptionsOutcome::Choices(ChoiceList::Unavailable)
    );
    wait(&mut host, 100, |h| h.state(ID) == Some(State::Reaped));
    assert_eq!(host.take_options(), None);
    assert_eq!(host.resources(), (0, 0));
    assert_eq!(host.remove(ID), Ok(()));
}

#[test]
fn a_list_beyond_the_contract_bounds_is_a_protocol_violation() {
    let (mut host, g) = start();
    let key = host
        .options(
            ID,
            g,
            action("group", json!({"scenario":"invalid"})),
            60_000,
        )
        .unwrap();
    assert_eq!(
        resolve(&mut host, 0, key),
        OptionsOutcome::Choices(ChoiceList::Unavailable)
    );
    wait(&mut host, 0, |h| h.state(ID) == Some(State::Reaped));
    assert!(host.diagnostics(ID).contains(&Diagnostic::Protocol));
    assert_eq!(host.take_options(), None);
    assert_eq!(host.remove(ID), Ok(()));
}

#[test]
fn queries_that_do_not_fit_the_manifest_fail_before_anything_is_sent() {
    let (mut host, g) = launch();
    let crash = || json!({"scenario":"crash"});
    // Not ready yet: nothing may be sent before the handshake.
    assert_eq!(
        host.options(ID, g, action("group", crash()), 60_000),
        Err(Error::Unavailable)
    );
    wait(&mut host, 0, |h| h.state(ID) == Some(State::Ready));
    let stale = serde_json::from_value(json!({"schema_version":2,"values":crash()})).unwrap();
    let cases = [
        (
            "other generation",
            g + 1,
            action("group", crash()),
            60_000,
            Error::Unavailable,
        ),
        (
            "deadline not ahead",
            g,
            action("group", crash()),
            0,
            Error::Clock,
        ),
        (
            "unknown definition",
            g,
            query(OptionsScope::Action, "nope", "group", crash()),
            60_000,
            Error::Invalid,
        ),
        (
            "action definition in visual scope",
            g,
            query(OptionsScope::Visual, "pick", "group", crash()),
            60_000,
            Error::Invalid,
        ),
        (
            "unknown field",
            g,
            action("nope", crash()),
            60_000,
            Error::Invalid,
        ),
        (
            "static field",
            g,
            action("scenario", crash()),
            60_000,
            Error::Invalid,
        ),
        (
            "self in its own context",
            g,
            action("group", json!({"scenario":"crash","group":"a"})),
            60_000,
            Error::Invalid,
        ),
        (
            "choice value outside its options",
            g,
            action("group", json!({"scenario":"bogus"})),
            60_000,
            Error::Invalid,
        ),
        (
            "context of another schema version",
            g,
            OptionsQuery {
                context: stale,
                ..action("group", crash())
            },
            60_000,
            Error::Invalid,
        ),
    ];
    for (case, generation, query, deadline, expected) in cases {
        assert_eq!(
            host.options(ID, generation, query, deadline),
            Err(expected),
            "{case}"
        );
    }
    assert_eq!(
        host.options("org.rad.unknown", g, action("group", crash()), 60_000),
        Err(Error::Unavailable)
    );
    // Every refused query above would have killed the plugin had it been sent.
    ping(&mut host, g, 0);
    assert_eq!(host.state(ID), Some(State::Ready));
    stop(&mut host, 0);
}

#[test]
fn querying_options_never_invokes_and_a_negotiated_jpeg_visual_is_accepted() {
    let (mut host, g) = start();
    for (field, values) in [
        ("group", json!({"scenario":"normal"})),
        ("member", json!({"scenario":"normal","group":"a"})),
        ("group", json!({"scenario":"empty"})),
    ] {
        ask(&mut host, g, action(field, values));
    }
    let probe = |host: &mut Host| {
        let params =
            serde_json::from_value(json!({"schema_version":1,"values":{"scenario":"normal"}}))
                .unwrap();
        let key = host.subscribe(ID, "probe", params, 1000).unwrap();
        wait(host, 0, |h| {
            h.visual(ID, key)
                .is_some_and(|v| matches!(v, VisualState::Current { .. }))
        });
        let Some(VisualState::Current { content }) = host.visual(ID, key) else {
            unreachable!("waited for a current state")
        };
        let jpeg = content.jpeg_hex.as_deref().expect("probe carries a JPEG");
        assert!(jpeg.starts_with("ffd8ff") && jpeg.len() <= 16384);
        assert!(content.png_hex.is_none());
        content.text.clone().expect("probe reports a count")
    };
    // The count is read when the plugin handles Subscribe, after every query above.
    assert_eq!(probe(&mut host), "invocations:0");
    ping(&mut host, g, 0);
    assert_eq!(probe(&mut host), "invocations:1");
    assert_eq!(host.state(ID), Some(State::Ready));
    stop(&mut host, 0);
}

#[test]
fn a_plugin_without_options_capability_refuses_queries_without_a_frame_and_keeps_working() {
    const FIXTURE: &str = "org.rad.fixture";
    let v1 = Document::new(include_bytes!("../fixture/manifest-v1.json"))
        .unwrap()
        .manifest(HOST_API, TARGET)
        .unwrap();
    assert_eq!(v1.negotiated_api(HOST_API), 1);
    let mut host = Host::default();
    let g = host
        .activate(
            Command::new(env!("CARGO_BIN_EXE_rad-plugin-fixture-v1")),
            v1,
            0,
        )
        .unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    while host.state(FIXTURE) != Some(State::Ready) {
        assert!(Instant::now() < end, "fixture watchdog");
        host.pump(0).unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    // `mode` is not a dynamic field: this query would be `Invalid` with dynamic_options, so
    // `Unavailable` shows the capability gate refuses before the manifest is consulted.
    let refused = query(OptionsScope::Action, "run", "mode", json!({}));
    assert_eq!(
        host.options(FIXTURE, g, refused, 5000),
        Err(Error::Unavailable)
    );
    // The v1 fixture exits on any message it does not know: a surviving echo round trip
    // proves that no frame was sent.
    let echo =
        serde_json::from_value(json!({"schema_version":1,"values":{"mode":"echo"}})).unwrap();
    let key = host.invoke(FIXTURE, g, "run", echo, 5000).unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        host.pump(0).unwrap();
        if let Some(done) = host.take_completion() {
            assert_eq!(done, (key, Outcome::Success));
            break;
        }
        assert!(Instant::now() < end, "fixture watchdog");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(host.state(FIXTURE), Some(State::Ready));
    host.shutdown();
    wait(&mut host, 0, |h| h.resources().0 == 0);
}

#[test]
fn a_late_reply_drained_before_the_timeout_is_taken_still_reports_it_once() {
    let (mut host, g) = start();
    let late = host
        .options(ID, g, action("group", json!({"scenario":"late"})), 100)
        .unwrap();
    host.pump(0).unwrap(); // `options` only queues: send the frame before the deadline
    host.pump(100).unwrap(); // expired; the caller has not taken the timeout yet
    // The plugin answers ~500 ms after the query. Keep pumping well past that, taking nothing.
    let end = Instant::now() + Duration::from_millis(900);
    while Instant::now() < end {
        host.pump(100).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(host.state(ID), Some(State::Ready));
    assert_eq!(host.take_options(), Some((late, OptionsOutcome::TimedOut)));
    assert_eq!(host.take_options(), None, "reported once, and nothing else");
    // Its slot is free again: both slots take new queries.
    for _ in 0..2 {
        host.options(ID, g, action("group", json!({"scenario":"slow"})), 60_000)
            .unwrap();
    }
    stop(&mut host, 100);
}
