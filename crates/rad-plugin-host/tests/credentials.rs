use rad_plugin_contract::*;
use rad_plugin_host::{CredentialState, Host, State, TARGET};
use std::{
    process::Command,
    time::{Duration, Instant},
};
fn manifest() -> Manifest {
    serde_json::from_value(serde_json::json!({"contract":1,"capabilities":["credentials","dynamic_options","jpeg","response_timeout","visual_freshness"],"id":"org.rad.credentials-fixture","version":{"major":1,"minor":0,"patch":0},
    "host_min":1,"host_max":1,"targets":[TARGET],"actions":[{"id":"authenticate","label":"Authenticate","schema":{"version":1,"fields":[]},"credential_slots":["password"]}],"visuals":[]})).unwrap()
}
fn pump(host: &mut Host, now: &mut u64, done: impl Fn(&Host) -> bool) {
    let end = Instant::now() + Duration::from_secs(3);
    while !done(host) {
        *now += 1;
        host.pump(*now).unwrap();
        assert!(Instant::now() < end, "fixture timeout");
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn wrong_correct_absent_and_old_snapshots_reach_only_the_declared_plugin() {
    let mut host = Host::default();
    let m = manifest();
    let id = m.id.clone();
    let mut now = 0;
    let g = host
        .activate(
            Command::new(env!("CARGO_BIN_EXE_rad-plugin-credentials-fixture")),
            m,
            now,
        )
        .unwrap();
    pump(&mut host, &mut now, |h| h.state(&id) == Some(State::Ready));
    for (revision, value) in [(1, Some("wrong")), (2, Some("correct")), (3, None)] {
        let values = CredentialValues::new(
            value
                .into_iter()
                .map(|value| CredentialValue {
                    slot: "password".into(),
                    value: value.into(),
                })
                .collect(),
        )
        .unwrap();
        host.install_credentials(&id, g, revision, values).unwrap();
        pump(&mut host, &mut now, |h| {
            h.credentials_state(&id, g, revision) == Some(CredentialState::Applied)
        });
    }
    for revision in [1, 2, 3, 1, 2] {
        let c = host
            .invoke_with_credentials(
                &id,
                g,
                "authenticate",
                Parameters {
                    schema_version: 1,
                    values: Default::default(),
                },
                (now + 1000, Some(revision)),
            )
            .unwrap();
        assert_eq!(
            host.release_credentials(&id, g, revision),
            Err(rad_plugin_host::Error::Residual)
        );
        let end = Instant::now() + Duration::from_secs(3);
        let (got, outcome) = loop {
            now += 1;
            host.pump(now).unwrap();
            if let Some(v) = host.take_completion() {
                break v;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(got, c);
        assert_eq!(
            outcome,
            if revision == 2 {
                Outcome::Success
            } else {
                Outcome::Failure {
                    code: FailureCode::CredentialsUnavailable,
                }
            }
        );
    }
    assert!(
        host.install_credentials(
            &id,
            g,
            4,
            CredentialValues::new(vec![CredentialValue {
                slot: "another".into(),
                value: "canary".into()
            }])
            .unwrap()
        )
        .is_err()
    );
    for revision in 1..=3 {
        host.release_credentials(&id, g, revision).unwrap();
        pump(&mut host, &mut now, |h| {
            h.credentials_state(&id, g, revision) == Some(CredentialState::Released)
        });
        assert_eq!(
            host.take_credentials_released(),
            Some((id.clone(), g, revision))
        );
    }
    host.shutdown();
    pump(&mut host, &mut now, |h| h.state(&id) == Some(State::Reaped));
}

#[test]
fn fixture_options_and_visuals_identify_only_the_captured_revision() {
    let mut m = manifest();
    m.actions[0].schema.fields.push(Field {
        id: "choice".into(),
        label: "Choice".into(),
        required: false,
        kind: FieldKind::DynamicChoice { depends_on: vec![] },
    });
    let mut visual = m.actions[0].clone();
    visual.id = "status".into();
    m.visuals.push(visual);
    let id = m.id.clone();
    let mut host = Host::default();
    let mut now = 0;
    let g = host
        .activate(
            Command::new(env!("CARGO_BIN_EXE_rad-plugin-credentials-fixture")),
            m,
            now,
        )
        .unwrap();
    pump(&mut host, &mut now, |h| h.state(&id) == Some(State::Ready));
    for (revision, value) in [(1, Some("wrong")), (2, Some("correct")), (3, None)] {
        let values = CredentialValues::new(
            value
                .into_iter()
                .map(|value| CredentialValue {
                    slot: "password".into(),
                    value: value.into(),
                })
                .collect(),
        )
        .unwrap();
        host.install_credentials(&id, g, revision, values).unwrap();
        pump(&mut host, &mut now, |h| {
            h.credentials_state(&id, g, revision) == Some(CredentialState::Applied)
        });
    }
    for revision in [1, 2, 3, 1, 2] {
        let parameters = Parameters {
            schema_version: 1,
            values: Default::default(),
        };
        let visual = host
            .subscribe_with_credentials(&id, "status", parameters.clone(), 1000, Some(revision))
            .unwrap();
        let query = host
            .options_with_credentials(
                &id,
                g,
                rad_plugin_host::OptionsQuery {
                    scope: OptionsScope::Action,
                    definition: "authenticate".into(),
                    field: "choice".into(),
                    context: parameters,
                },
                now + 1000,
                Some(revision),
            )
            .unwrap();
        let end = Instant::now() + Duration::from_secs(3);
        let (correlation, outcome) = loop {
            now += 1;
            host.pump(now).unwrap();
            if let Some(result) = host.take_options() {
                break result;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(correlation, query);
        let marker = format!("rev{revision}");
        assert_eq!(
            outcome,
            rad_plugin_host::OptionsOutcome::Choices(ChoiceList::Available {
                options: vec![ChoiceOption {
                    value: marker.clone(),
                    label: marker.clone()
                }]
            })
        );
        let expected = if revision == 2 {
            VisualState::Current {
                content: VisualContent {
                    text: Some(marker),
                    png_hex: None,
                    jpeg_hex: None,
                    color: None,
                },
            }
        } else {
            VisualState::Unavailable
        };
        // The options reply is a FIFO barrier after the preceding visual answer.
        assert_eq!(host.visual(&id, visual), Some(&expected));
        host.unsubscribe(&id, visual).unwrap();
    }
    host.shutdown();
    pump(&mut host, &mut now, |h| h.state(&id) == Some(State::Reaped));
}

#[test]
fn fixture_ack_delays_are_monotonic_and_selected_only_by_exact_identity() {
    for (id, minimum) in [
        (
            "org.rad.test.credentials.delayed",
            Duration::from_millis(2500),
        ),
        (
            "org.rad.test.credentials.deferred",
            Duration::from_millis(100),
        ),
    ] {
        let mut m = manifest();
        m.id = id.into();
        let mut host = Host::default();
        let g = host
            .activate(
                Command::new(env!("CARGO_BIN_EXE_rad-plugin-credentials-fixture")),
                m,
                0,
            )
            .unwrap();
        let ready_deadline = Instant::now() + Duration::from_secs(3);
        while host.state(id) != Some(State::Ready) {
            host.pump(0).unwrap();
            assert!(Instant::now() < ready_deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let started = Instant::now();
        host.install_credentials(id, g, 1, CredentialValues::new(vec![]).unwrap())
            .unwrap();
        // Freeze only this test's host clock so the 2500ms peer ACK can be observed.
        // A real advancing host clock expires its 2000ms deadline instead.
        while host.credentials_state(id, g, 1) != Some(CredentialState::Applied) {
            host.pump(0).unwrap();
            assert!(started.elapsed() < Duration::from_secs(4));
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(started.elapsed() >= minimum, "fixture ACK was not delayed");
        host.shutdown();
        let mut now = 0;
        pump(&mut host, &mut now, |h| h.state(id) == Some(State::Reaped));
    }
}

#[test]
fn fixture_invocation_counter_is_a_non_secret_no_effect_oracle() {
    let mut m = manifest();
    m.actions[0].schema.fields.push(Field {
        id: "invocations".into(),
        label: "Invocations".into(),
        required: false,
        kind: FieldKind::DynamicChoice { depends_on: vec![] },
    });
    let id = m.id.clone();
    let mut host = Host::default();
    let mut now = 0;
    let g = host
        .activate(
            Command::new(env!("CARGO_BIN_EXE_rad-plugin-credentials-fixture")),
            m,
            now,
        )
        .unwrap();
    pump(&mut host, &mut now, |h| h.state(&id) == Some(State::Ready));
    host.install_credentials(&id, g, 1, CredentialValues::new(vec![]).unwrap())
        .unwrap();
    pump(&mut host, &mut now, |h| {
        h.credentials_state(&id, g, 1) == Some(CredentialState::Applied)
    });
    for expected in 0..=1 {
        let parameters = Parameters {
            schema_version: 1,
            values: Default::default(),
        };
        host.options_with_credentials(
            &id,
            g,
            rad_plugin_host::OptionsQuery {
                scope: OptionsScope::Action,
                definition: "authenticate".into(),
                field: "invocations".into(),
                context: parameters.clone(),
            },
            now + 1000,
            Some(1),
        )
        .unwrap();
        let end = Instant::now() + Duration::from_secs(3);
        let outcome = loop {
            now += 1;
            host.pump(now).unwrap();
            if let Some((_, outcome)) = host.take_options() {
                break outcome;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        };
        let marker = format!("count{expected}");
        assert_eq!(
            outcome,
            rad_plugin_host::OptionsOutcome::Choices(ChoiceList::Available {
                options: vec![ChoiceOption {
                    value: marker.clone(),
                    label: marker
                }]
            })
        );
        if expected == 0 {
            host.invoke_with_credentials(&id, g, "authenticate", parameters, (now + 1000, Some(1)))
                .unwrap();
            let end = Instant::now() + Duration::from_secs(3);
            loop {
                now += 1;
                host.pump(now).unwrap();
                if host.take_completion().is_some() {
                    break;
                }
                assert!(Instant::now() < end);
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    host.shutdown();
    pump(&mut host, &mut now, |h| h.state(&id) == Some(State::Reaped));
}
