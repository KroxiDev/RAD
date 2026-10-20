use rad_authority::{Authority, Command, Effect, Outcome};
use rad_core::{configuration::*, execution::*, identity::*};
fn fixture() -> Candidate {
    let c = Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "run".into(),
            },
        )
        .unwrap();
    let b = c.configuration().data().buttons[0].binding.identity();
    c.configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::AddAction {
                button: b,
                kind: ActionKind::Launcher(DesktopEntry::new("old.desktop".into()).unwrap()),
            },
        )
        .unwrap()
}
fn actor(c: Candidate) -> Authority {
    Authority::new(c, 1, std::num::NonZeroU128::new(11).unwrap())
}
fn submit(a: &mut Authority, c: impl Into<Command>) -> Vec<Effect> {
    let t = a.reserve(1, Origin::Editor).unwrap();
    a.submit(t, c)
}
fn dispatch(e: &[Effect]) -> DispatchIntent {
    e.iter()
        .find_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d.clone())
            } else {
                None
            }
        })
        .expect("dispatch")
}
#[test]
fn admission_during_commit_uses_old_snapshot_then_new_after_publication() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let action = c.configuration().data().buttons[0].actions[0]
        .binding
        .identity();
    let mut a = actor(c);
    let p = submit(
        &mut a,
        vec![Edit::SetParameters {
            action,
            parameters: ActionParameters::Launcher(
                DesktopEntry::new("new.desktop".into()).unwrap(),
            ),
        }],
    );
    let Effect::Persist { id, .. } = p[0] else {
        panic!()
    };
    let run = submit(
        &mut a,
        Command::Run {
            target: Target::Button(b),
        },
    );
    let old = dispatch(&run);
    assert_eq!(
        old.operation,
        Operation::Launcher(DesktopEntry::new("old.desktop".into()).unwrap())
    );
    a.complete(
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert!(matches!(
        submit(
            &mut a,
            Command::Run {
                target: Target::Action(action)
            }
        )
        .as_slice(),
        [Effect::Rejected { .. }]
    ));
    a.job_complete(old.id, Completion::Succeeded).unwrap();
    // Consume delivered responses, otherwise per-connection credit deliberately remains held.
    for e in run {
        if let Effect::Accepted { request, .. } = e {
            a.finish(request);
        }
    }
    let new = dispatch(&submit(
        &mut a,
        Command::Run {
            target: Target::Button(b),
        },
    ));
    assert_eq!(
        new.operation,
        Operation::Launcher(DesktopEntry::new("new.desktop".into()).unwrap())
    );
}
#[test]
fn timeout_ttl_and_history_eviction_do_not_release_residual_revision_until_real_termination() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let mut a = actor(c);
    let e = submit(
        &mut a,
        Command::Run {
            target: Target::Button(b),
        },
    );
    let d = dispatch(&e);
    for e in e {
        if let Effect::Accepted { request, .. } = e {
            a.finish(request);
        }
    }
    assert_eq!(a.next_deadline(), Some(5000));
    a.advance(5000).unwrap();
    for i in 0..30 {
        let e = submit(
            &mut a,
            vec![Edit::SetSettings(SettingsData {
                brightness_percent: i,
                ..SettingsData::default()
            })],
        );
        let Effect::Persist { id, .. } = e[0] else {
            panic!()
        };
        for e in a.complete(
            id,
            Outcome::Applied {
                revision: i64::from(i) + 2,
                changed: true,
            },
        ) {
            if let Effect::Reply { request, .. } = e {
                a.finish(request);
            }
        }
    }
    a.advance(605000).unwrap();
    assert!(a.retained_revisions().contains(&1));
    a.job_complete(d.id, Completion::Succeeded).unwrap();
    assert!(!a.retained_revisions().contains(&1));
    assert!(a.job_complete(d.id, Completion::Succeeded).is_err());
}
#[test]
fn installed_type_is_admission_gate_but_visual_and_dynamic_service_are_not() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let action = c.configuration().data().buttons[0].actions[0]
        .binding
        .identity();
    let kind = ExternalType::new("p".into(), "act".into()).unwrap();
    let visual = ExternalType::new("missing".into(), "visual".into()).unwrap();
    let c = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                Edit::ReplaceActionType {
                    action,
                    kind: ActionKind::External {
                        type_key: kind.clone(),
                        parameters: br#"{"schema_version":1,"values":{}}"#.to_vec(),
                    },
                },
                Edit::SetVisual {
                    button: b,
                    visual: VisualSource::External {
                        type_key: visual,
                        configuration: vec![],
                    },
                },
            ],
        )
        .unwrap();
    let mut a = actor(c);
    let e = submit(
        &mut a,
        Command::Run {
            target: Target::Button(b),
        },
    );
    assert!(matches!(e.as_slice(), [Effect::Rejected { .. }]));
    a.install_plugin(
        rad_plugin_contract::Manifest {
            lifecycle: Default::default(),
            contract: 1,
            id: "p".into(),
            version: rad_plugin_contract::Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            host_min: 1,
            host_max: 1,
            targets: vec!["x86_64-unknown-linux-gnu".into()],
            actions: vec![rad_plugin_contract::Definition {
                response_timeout: None,
                id: "act".into(),
                label: "Action".into(),
                schema: rad_plugin_contract::Schema {
                    version: 1,
                    fields: vec![],
                },
                credential_slots: vec![],
            }],
            visuals: vec![],
        },
        1,
        "x86_64-unknown-linux-gnu",
    )
    .unwrap();
    let e = submit(
        &mut a,
        Command::Run {
            target: Target::Button(b),
        },
    );
    let d = dispatch(&e);
    assert!(matches!(d.operation, Operation::External { .. }));
    assert!(
        a.job_complete(d.id, Completion::Failed(Failure::ServiceUnavailable))
            .unwrap()
            .is_empty()
    );
    assert!(a.job_complete(d.id, Completion::Succeeded).is_err());
}
#[test]
fn physical_input_captures_threshold_but_parameters_only_at_admission() {
    let c = fixture();
    let action = c.configuration().data().buttons[0].actions[0]
        .binding
        .identity();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                press_threshold_ms: 100,
                ..SettingsData::default()
            }),
        )
        .unwrap();
    let mut a = actor(c);
    a.input_report(0, [false; 32]).unwrap();
    let mut down = [false; 32];
    down[0] = true;
    assert!(a.input_report(10, down).unwrap().is_empty());
    a.advance(50).unwrap();
    let e = submit(
        &mut a,
        vec![
            Edit::SetParameters {
                action,
                parameters: ActionParameters::Launcher(
                    DesktopEntry::new("edited.desktop".into()).unwrap(),
                ),
            },
            Edit::SetSettings(SettingsData {
                press_threshold_ms: 500,
                ..SettingsData::default()
            }),
        ],
    );
    let Effect::Persist { id, .. } = e[0] else {
        panic!()
    };
    a.complete(
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert!(a.advance(109).unwrap().is_empty());
    let d = dispatch(&a.advance(110).unwrap());
    assert_eq!(
        d.operation,
        Operation::Launcher(DesktopEntry::new("edited.desktop".into()).unwrap())
    );
    assert!(a.input_report(111, down).unwrap().is_empty());
}
#[test]
fn presses_during_uncertain_are_consumed_not_replayed_after_resolution() {
    let c = fixture();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                press_threshold_ms: 100,
                ..SettingsData::default()
            }),
        )
        .unwrap();
    let mut a = actor(c);
    a.input_report(0, [false; 32]).unwrap();
    let e = submit(
        &mut a,
        vec![Edit::SetSettings(SettingsData {
            brightness_percent: 42,
            press_threshold_ms: 100,
            ..SettingsData::default()
        })],
    );
    let Effect::Persist { id, .. } = e[0] else {
        panic!()
    };
    a.complete(id, Outcome::Uncertain);
    let mut down = [false; 32];
    down[0] = true;
    assert!(a.input_report(10, down).unwrap().is_empty());
    let e = a.reconcile();
    let Effect::Reconcile { id } = e[0] else {
        panic!()
    };
    a.complete(id, Outcome::NotApplied);
    assert!(a.advance(110).unwrap().is_empty());
    assert!(a.input_report(120, down).unwrap().is_empty());
    a.input_report(121, [false; 32]).unwrap();
    a.input_report(122, down).unwrap();
    assert!(
        a.advance(222)
            .unwrap()
            .iter()
            .any(|e| matches!(e, Effect::Dispatch(_)))
    );
}
#[test]
fn layout_round_trip_invalidates_candidate() {
    let c = fixture();
    let root = c.configuration().data().principal;
    let c = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                Edit::CreateFolder {
                    parent: root,
                    position: 1,
                    name: "folder".into(),
                },
                Edit::SetSettings(SettingsData {
                    press_threshold_ms: 100,
                    ..SettingsData::default()
                }),
            ],
        )
        .unwrap();
    let folder = c.configuration().data().folders[1].binding.identity();
    let mut a = actor(c);
    a.input_report(0, [false; 32]).unwrap();
    let mut down = [false; 32];
    down[0] = true;
    a.input_report(10, down).unwrap();
    a.advance(20).unwrap();
    a.navigate(Origin::Editor, folder).unwrap();
    a.navigate(Origin::Editor, root).unwrap();
    assert!(a.advance(110).unwrap().is_empty());
}
#[test]
fn collection_serializes_with_commit_and_uncertain_gc_never_rewrites_prior_applied() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let mut a = actor(c);
    let gc = a.collect().unwrap();
    let Effect::Collect { id, ref roots } = gc[0] else {
        panic!("collect effect")
    };
    assert_eq!(roots, &vec![1]);
    assert!(
        submit(
            &mut a,
            vec![Edit::SetSettings(SettingsData {
                brightness_percent: 42,
                ..SettingsData::default()
            })]
        )
        .is_empty()
    );
    let run = submit(
        &mut a,
        Command::Run {
            target: Target::Button(b),
        },
    );
    assert_eq!(
        dispatch(&run).operation,
        Operation::Launcher(DesktopEntry::new("old.desktop".into()).unwrap())
    );
    let e = a.complete(id, Outcome::Uncertain);
    assert!(e.iter().any(|e| matches!(e, Effect::Rejected { .. })));
    assert!(a.collect().is_err());
    assert_eq!(a.editor_view(Origin::Editor).unwrap().brightness, 100);
    let e = a.reconcile();
    let Effect::Reconcile { id } = e[0] else {
        panic!()
    };
    a.complete(
        id,
        Outcome::Applied {
            revision: 1,
            changed: false,
        },
    );
    assert_eq!(a.storage_state(), rad_authority::StorageState::Ready);
    assert_eq!(a.editor_view(Origin::Editor).unwrap().history_operations, 0);
    a.job_complete(dispatch(&run).id, Completion::Succeeded)
        .unwrap();
}
#[test]
fn input_overflow_keeps_due_dispatch_and_consumes_failed_press() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let c = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                Edit::AddAction {
                    button: b,
                    kind: ActionKind::Wait(Wait::new(10).unwrap()),
                },
                Edit::AddAction {
                    button: b,
                    kind: ActionKind::Launcher(DesktopEntry::new("second.desktop".into()).unwrap()),
                },
                Edit::SetSettings(SettingsData {
                    press_threshold_ms: u64::MAX,
                    ..SettingsData::default()
                }),
            ],
        )
        .unwrap();
    let mut a = actor(c);
    a.input_report(0, [false; 32]).unwrap();
    let first = dispatch(&submit(
        &mut a,
        Command::Run {
            target: Target::Button(b),
        },
    ));
    a.job_complete(first.id, Completion::Succeeded).unwrap();
    let mut down = [false; 32];
    down[0] = true;
    let e = a
        .input_report(10, down)
        .expect("Input failure must not discard Engine effects");
    assert_eq!(
        dispatch(&e).operation,
        Operation::Launcher(DesktopEntry::new("second.desktop".into()).unwrap())
    );
    assert!(e.iter().any(|e| matches!(e, Effect::InputRejected { .. })));
    assert!(a.input_report(11, down).unwrap().is_empty());
}
#[test]
fn impossible_settings_deadline_rejected_before_persist() {
    let mut a = actor(fixture());
    let mut down = [false; 32];
    down[31] = true;
    a.input_report(1, down).unwrap();
    for settings in [
        SettingsData {
            sleep_ms: u64::MAX,
            ..SettingsData::default()
        },
        SettingsData {
            press_threshold_ms: u64::MAX,
            ..SettingsData::default()
        },
    ] {
        let e = submit(&mut a, vec![Edit::SetSettings(settings)]);
        assert!(matches!(
            e.as_slice(),
            [Effect::Rejected {
                reason: rad_authority::Rejection::Invalid,
                ..
            }]
        ));
    }
    assert_eq!(a.editor_view(Origin::Editor).unwrap().revision, 1);
}
#[test]
fn sleep_publication_projects_brightness_and_wake_consumes_press() {
    let c = fixture();
    let mut a = actor(c);
    a.input_report(0, [false; 32]).unwrap();
    a.before_event(100).unwrap();
    let e = submit(
        &mut a,
        vec![Edit::SetSettings(SettingsData {
            sleep_ms: 50,
            brightness_percent: 40,
            ..SettingsData::default()
        })],
    );
    let Effect::Persist { id, .. } = e[0] else {
        panic!()
    };
    assert!(!e.iter().any(|e| matches!(e, Effect::Display { .. })));
    let e = a.complete(
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert!(e.iter().any(|e| matches!(
        e,
        Effect::Display {
            brightness: 40,
            asleep: true
        }
    )));
    let mut down = [false; 32];
    down[0] = true;
    let e = a.input_report(101, down).unwrap();
    assert!(e.iter().any(|e| matches!(e, Effect::Wake)));
    assert!(!e.iter().any(|e| matches!(e, Effect::Dispatch(_))));
    assert_eq!(a.next_deadline(), Some(151));
    a.input_report(102, [false; 32]).unwrap();
    assert!(
        a.input_report(103, down)
            .unwrap()
            .iter()
            .any(|e| matches!(e, Effect::Dispatch(_)))
    );
}
#[test]
fn zero_actions_release_immediately_and_residual_capacity_survives_timeout() {
    let c = fixture();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::RemoveAction {
                action: c.configuration().data().buttons[0].actions[0]
                    .binding
                    .identity(),
            },
        )
        .unwrap();
    let b = c.configuration().data().buttons[0].binding.identity();
    let mut a = actor(c);
    for _ in 0..40 {
        let e = submit(
            &mut a,
            Command::Run {
                target: Target::Button(b),
            },
        );
        assert!(!e.iter().any(|e| matches!(e, Effect::Dispatch(_))));
        let request = e
            .iter()
            .find_map(|e| {
                if let Effect::Accepted { request, .. } = e {
                    Some(*request)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(a.finish(request));
    }
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let mut a = actor(c);
    let mut jobs = vec![];
    for i in 0..32 {
        a.before_event(i * 5000).unwrap();
        let e = submit(
            &mut a,
            Command::Run {
                target: Target::Button(b),
            },
        );
        jobs.push(dispatch(&e));
        for e in e {
            if let Effect::Accepted { request, .. } = e {
                a.finish(request);
            }
        }
    }
    a.before_event(160000).unwrap();
    assert!(matches!(
        submit(
            &mut a,
            Command::Run {
                target: Target::Button(b)
            }
        )
        .as_slice(),
        [Effect::Rejected { .. }]
    ));
    // Completion remains processable even with all 32 client credits held.
    let mut held = vec![];
    for connection in 10..18 {
        for _ in 0..4 {
            if let Ok(t) = a.reserve(connection, Origin::Editor) {
                held.push(t);
            }
        }
    }
    a.job_complete(jobs[0].id, Completion::Succeeded).unwrap();
    assert!(a.job_complete(jobs[0].id, Completion::Succeeded).is_err());
    for connection in 10..18 {
        a.disconnect(connection);
    }
    assert!(
        submit(
            &mut a,
            Command::Run {
                target: Target::Button(b)
            }
        )
        .iter()
        .any(|e| matches!(e, Effect::Dispatch(_)))
    );
}
#[test]
fn activity_during_pending_can_overflow_projection_but_never_rolls_back_applied() {
    let mut a = actor(fixture());
    let e = submit(
        &mut a,
        vec![Edit::SetSettings(SettingsData {
            sleep_ms: u64::MAX,
            ..SettingsData::default()
        })],
    );
    let Effect::Persist { id, .. } = e[0] else {
        panic!()
    };
    let mut down = [false; 32];
    down[31] = true;
    a.input_report(1, down).unwrap();
    let e = a.complete(
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert!(e.iter().any(|e| matches!(e, Effect::DisplayClockOverflow)));
    assert!(
        e.iter()
            .any(|e| matches!(e, Effect::Reply { changed: true, .. }))
    );
    assert_eq!(a.editor_view(Origin::Editor).unwrap().revision, 2);
    assert_eq!(a.next_deadline(), None);
    assert!(a.advance(2).unwrap().is_empty());
}
#[test]
fn run_rejection_preserves_typed_busy_instead_of_invalid_command() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let mut a = actor(c);
    submit(
        &mut a,
        Command::Run {
            target: Target::Button(b),
        },
    );
    assert!(matches!(
        submit(
            &mut a,
            Command::Run {
                target: Target::Button(b)
            }
        )
        .as_slice(),
        [Effect::Rejected {
            reason: rad_authority::Rejection::Execution(ExecutionError::Busy),
            ..
        }]
    ));
}
#[test]
fn input_disconnect_invalidates_pending_and_requires_d9_rearm() {
    let c = fixture();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                press_threshold_ms: 100,
                ..SettingsData::default()
            }),
        )
        .unwrap();
    let mut a = actor(c);
    a.input_report(0, [false; 32]).unwrap();
    let mut down = [false; 32];
    down[0] = true;
    a.input_report(1, down).unwrap();
    a.input_disconnect(50).unwrap();
    assert!(a.advance(101).unwrap().is_empty());
    down[1] = true;
    assert!(a.input_report(102, down).unwrap().is_empty());
    assert!(a.advance(202).unwrap().is_empty());
    a.input_report(203, [false; 32]).unwrap();
    down[1] = false;
    a.input_report(204, down).unwrap();
    assert!(
        a.advance(304)
            .unwrap()
            .iter()
            .any(|e| matches!(e, Effect::Dispatch(_)))
    );
}
#[test]
fn gc_completion_with_changed_revision_requires_reconciliation() {
    let mut a = actor(fixture());
    let e = a.collect().unwrap();
    let Effect::Collect { id, .. } = e[0] else {
        panic!()
    };
    a.complete(
        id,
        Outcome::Applied {
            revision: 99,
            changed: true,
        },
    );
    assert_eq!(a.storage_state(), rad_authority::StorageState::Uncertain);
    assert_eq!(a.editor_view(Origin::Editor).unwrap().revision, 1);
    assert!(matches!(
        a.reconcile().as_slice(),
        [Effect::Reconcile { .. }]
    ));
}
#[test]
fn codec_scalar_boundary_is_enforced_before_store_and_timeout_wins_exact_completion() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let mut a = actor(c);
    let e = submit(
        &mut a,
        vec![Edit::SetName {
            button: b,
            name: "x".repeat(8190),
        }],
    );
    let Effect::Persist { id, .. } = e[0] else {
        panic!("8192 serialized bytes must fit")
    };
    for e in a.complete(id, Outcome::NotApplied) {
        if let Effect::Rejected { request, .. } = e {
            a.finish(request);
        }
    }
    assert!(matches!(
        submit(
            &mut a,
            vec![Edit::SetName {
                button: b,
                name: "x".repeat(8191)
            }]
        )
        .as_slice(),
        [Effect::Rejected {
            reason: rad_authority::Rejection::Invalid,
            ..
        }]
    ));
    let e = submit(
        &mut a,
        Command::Run {
            target: Target::Button(b),
        },
    );
    let d = dispatch(&e);
    a.before_event(5000).unwrap();
    a.job_complete(d.id, Completion::Succeeded).unwrap();
    assert_eq!(
        a.result(Origin::Editor, d.id.execution()).unwrap().status,
        ExecutionStatus::TimedOut
    );
}
#[test]
fn sleep_deadline_is_based_on_last_activity_not_commit_preparation_time() {
    let mut a = actor(fixture());
    a.before_event(100).unwrap();
    let e = submit(
        &mut a,
        vec![Edit::SetSettings(SettingsData {
            sleep_ms: u64::MAX,
            ..SettingsData::default()
        })],
    );
    assert!(matches!(e.as_slice(), [Effect::Persist { .. }]));
}
#[test]
fn delete_and_revoke_after_admission_do_not_replace_snapshot_but_hide_results() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::AddAction {
                button: b,
                kind: ActionKind::Launcher(DesktopEntry::new("snapshot.desktop".into()).unwrap()),
            },
        )
        .unwrap();
    let mut a = actor(c);
    let first = dispatch(&submit(
        &mut a,
        Command::Run {
            target: Target::Button(b),
        },
    ));
    let e = submit(
        &mut a,
        vec![
            Edit::DeleteButton { button: b },
            Edit::SetGlobalPermissions(
                rad_core::permissions::Permissions::new(false, false, false).unwrap(),
            ),
        ],
    );
    let Effect::Persist { id, .. } = e[0] else {
        panic!()
    };
    a.complete(
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert_eq!(a.result(Origin::Automation, first.id.execution()), None);
    assert_eq!(a.result(Origin::Editor, first.id.execution()), None);
    let second = dispatch(&a.job_complete(first.id, Completion::Succeeded).unwrap());
    assert_eq!(
        second.operation,
        Operation::Launcher(DesktopEntry::new("snapshot.desktop".into()).unwrap())
    );
    a.job_complete(second.id, Completion::Succeeded).unwrap();
    assert!(a.job_complete(first.id, Completion::Succeeded).is_err());
}
#[test]
fn sixteen_active_executions_share_capacity_with_editor_and_hardware_admissions() {
    let mut c = fixture();
    let folder = c.configuration().data().principal;
    for position in 1..17 {
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::CreateButton {
                    folder,
                    position,
                    name: format!("b{position}"),
                },
            )
            .unwrap();
        let b = c
            .configuration()
            .data()
            .buttons
            .last()
            .unwrap()
            .binding
            .identity();
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::AddAction {
                    button: b,
                    kind: ActionKind::Launcher(DesktopEntry::new("app.desktop".into()).unwrap()),
                },
            )
            .unwrap();
    }
    let buttons: Vec<_> = c
        .configuration()
        .data()
        .buttons
        .iter()
        .map(|b| b.binding.identity())
        .collect();
    let mut a = actor(c);
    a.input_report(0, [false; 32]).unwrap();
    for button in &buttons[..16] {
        let e = submit(
            &mut a,
            Command::Run {
                target: Target::Button(*button),
            },
        );
        assert!(e.iter().any(|e| matches!(e, Effect::Dispatch(_))));
        for e in e {
            if let Effect::Accepted { request, .. } = e {
                a.finish(request);
            }
        }
    }
    let mut down = [false; 32];
    down[16] = true;
    assert!(matches!(
        a.input_report(1, down).unwrap().as_slice(),
        [Effect::InputRejected {
            reason: ExecutionError::Capacity
        }]
    ));
}
#[test]
fn short_id_admission_resolves_child_without_requiring_parent_consulta() {
    let c = fixture();
    let mut data = c.configuration().data().clone();
    let child = data.buttons[0].actions[0].binding.short();
    data.global_permissions = rad_core::permissions::Permissions::new(false, false, false).unwrap();
    data.buttons[0].actions[0].permissions = rad_core::permissions::LocalPermissions::own(
        rad_core::permissions::Permissions::new(true, false, true).unwrap(),
    );
    let c = c
        .configuration()
        .prepare_restore(c.registry(), Origin::Editor, true, data)
        .unwrap();
    let mut a = actor(c);
    let t = a.reserve(1, Origin::Automation).unwrap();
    let e = a.submit(t, Command::RunId { id: child });
    assert!(e.iter().any(|e| matches!(e, Effect::Dispatch(_))));
}
