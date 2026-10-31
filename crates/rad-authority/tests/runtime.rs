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
                brightness_percent: i + 10,
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
fn physical_sleep_preserves_folder_and_brightness_and_consumes_the_whole_wake_press() {
    for (gesture, threshold) in [(Gesture::Press, 0), (Gesture::LongPress, 25)] {
        let c = fixture();
        let root = c.configuration().data().principal;
        let c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::CreateFolder {
                    parent: root,
                    position: 1,
                    name: "Tools".into(),
                },
            )
            .unwrap();
        let folder = c.configuration().data().folders[1].binding.identity();
        let c = c
            .configuration()
            .prepare_edits(
                c.registry(),
                Origin::Editor,
                &[
                    Edit::CreateButton {
                        folder,
                        position: 1,
                        name: "Sleep".into(),
                    },
                    Edit::CreateButton {
                        folder,
                        position: 2,
                        name: "Launch".into(),
                    },
                    Edit::SetSettings(SettingsData {
                        brightness_percent: 37,
                        sleep_ms: 0,
                        long_press_threshold_ms: 25,
                        ..Default::default()
                    }),
                ],
            )
            .unwrap();
        let sleep = c
            .configuration()
            .data()
            .buttons
            .iter()
            .find(|b| b.name == "Sleep")
            .unwrap()
            .binding
            .identity();
        let launch = c
            .configuration()
            .data()
            .buttons
            .iter()
            .find(|b| b.name == "Launch")
            .unwrap()
            .binding
            .identity();
        let c = c
            .configuration()
            .prepare_edits(
                c.registry(),
                Origin::Editor,
                &[
                    Edit::AddAction {
                        button: sleep,
                        kind: ActionKind::Sleep,
                    },
                    Edit::SetGesture {
                        button: sleep,
                        gesture,
                    },
                    Edit::AddAction {
                        button: launch,
                        kind: ActionKind::Launcher(
                            DesktopEntry::new("wake.desktop".into()).unwrap(),
                        ),
                    },
                ],
            )
            .unwrap();
        let mut a = actor(c);
        a.navigate(Origin::Editor, folder).unwrap();
        a.input_report(0, [false; 32]).unwrap();
        let mut down = [false; 32];
        down[1] = true;
        let mut slept = a.input_report(1, down).unwrap();
        slept.extend(a.advance(1 + threshold).unwrap());
        assert!(
            slept
                .iter()
                .any(|e| matches!(e, Effect::HardwareAccepted { .. }))
        );
        assert!(slept.iter().any(|e| matches!(
            e,
            Effect::Display {
                brightness: 37,
                asleep: true
            }
        )));
        assert!(!slept.iter().any(|e| matches!(
            e,
            Effect::Dispatch(_) | Effect::SelectionChanged { .. } | Effect::Persist { .. }
        )));
        // Releasing the key that put the deck to sleep does not wake it.
        let release = a.input_report(30, [false; 32]).unwrap();
        assert!(
            !release
                .iter()
                .any(|e| matches!(e, Effect::Wake | Effect::Display { asleep: false, .. }))
        );
        // Any key wakes: even an assigned launcher is consumed through its release.
        down[1] = false;
        down[2] = true;
        let wake = a.input_report(31, down).unwrap();
        assert!(wake.iter().any(|e| matches!(e, Effect::Wake)));
        assert!(wake.iter().any(|e| matches!(
            e,
            Effect::Display {
                brightness: 37,
                asleep: false
            }
        )));
        assert!(
            !wake
                .iter()
                .any(|e| matches!(e, Effect::HardwareAccepted { .. } | Effect::Dispatch(_)))
        );
        assert!(a.input_report(32, down).unwrap().is_empty());
        assert!(
            !a.input_report(33, [false; 32])
                .unwrap()
                .iter()
                .any(|e| matches!(e, Effect::Dispatch(_)))
        );
        assert_eq!(a.status(Origin::Editor).active.unwrap().identity(), folder);
        assert_eq!(a.editor_view(Origin::Editor).unwrap().revision, 1);
        let next = a.input_report(34, down).unwrap();
        assert_eq!(
            dispatch(&next).operation,
            Operation::Launcher(DesktopEntry::new("wake.desktop".into()).unwrap())
        );
        // Timeout 0 never disables the explicit action and remains unchanged.
        assert_eq!(a.settings(Origin::Editor).unwrap().sleep_ms, 0);
    }
}

#[test]
fn sleep_is_an_ordered_native_plan_step_and_does_not_stop_the_remaining_plan() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let action = c.configuration().data().buttons[0].actions[0]
        .binding
        .identity();
    let c = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                Edit::ReplaceActionType {
                    action,
                    kind: ActionKind::Wait(Wait::new(5).unwrap()),
                },
                Edit::AddAction {
                    button: b,
                    kind: ActionKind::Sleep,
                },
                Edit::AddAction {
                    button: b,
                    kind: ActionKind::Launcher(DesktopEntry::new("after.desktop".into()).unwrap()),
                },
            ],
        )
        .unwrap();
    let mut a = actor(c);
    let start = submit(
        &mut a,
        Command::Run {
            target: Target::Button(b),
        },
    );
    let execution = start
        .iter()
        .find_map(|e| {
            if let Effect::Accepted { execution, .. } = e {
                Some(*execution)
            } else {
                None
            }
        })
        .unwrap();
    assert!(!start.iter().any(|e| matches!(
        e,
        Effect::Display { asleep: true, .. } | Effect::Dispatch(_)
    )));
    let due = a.advance(5).unwrap();
    assert!(
        due.iter()
            .any(|e| matches!(e, Effect::Display { asleep: true, .. }))
    );
    let job = dispatch(&due);
    assert_eq!(
        job.operation,
        Operation::Launcher(DesktopEntry::new("after.desktop".into()).unwrap())
    );
    a.job_complete(job.id, Completion::Succeeded).unwrap();
    assert_eq!(
        a.result(Origin::Editor, execution).unwrap().status,
        ExecutionStatus::Succeeded
    );
}
