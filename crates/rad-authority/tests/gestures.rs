use rad_authority::{Authority, Effect, plugins::PluginError};
use rad_core::{configuration::*, execution::*, identity::*, input::ControlGesture};
use rad_plugin_contract::{Definition, Manifest, Schema, Version};
fn setup(mode: Mode) -> Authority {
    let c = Configuration::prepare_new(&IdentityRegistry::new(717).unwrap()).unwrap();
    let root = c.configuration().data().principal;
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: root,
                position: 0,
                name: "Hold".into(),
            },
        )
        .unwrap();
    let button = c.configuration().data().buttons[0].binding.identity();
    let action = || Edit::AddAction {
        button,
        kind: ActionKind::External {
            type_key: ExternalType::new("fixture".into(), "run".into()).unwrap(),
            parameters: br#"{"schema_version":1,"values":{}}"#.to_vec(),
        },
    };
    let c = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                action(),
                action(),
                Edit::SetGesture {
                    button,
                    gesture: Gesture::Hold,
                },
                Edit::SetPluginSettings {
                    plugin: "fixture".into(),
                    settings: Some(
                        PluginSettings::new(br#"{"schema_version":1,"values":{}}"#.to_vec())
                            .unwrap(),
                    ),
                },
            ],
        )
        .unwrap();
    let order = c.configuration().data().buttons[0]
        .actions
        .iter()
        .map(|a| a.binding.identity())
        .collect();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetPlan {
                button,
                mode,
                order,
            },
        )
        .unwrap();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(718).unwrap());
    let manifest = Manifest {
        resources: vec![],
        capabilities: rad_plugin_contract::SUPPORTED_CAPABILITIES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        settings: Some(Schema {
            version: 1,
            fields: vec![],
        }),
        lifecycle: Default::default(),
        contract: 1,
        id: "fixture".into(),
        version: Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        host_min: 1,
        host_max: 1,
        targets: vec!["x86_64-unknown-linux-gnu".into()],
        actions: vec![Definition {
            id: "run".into(),
            label: "Run".into(),
            schema: Schema {
                version: 1,
                fields: vec![],
            },
            credential_slots: vec![],
            response_timeout: None,
            gestures: vec![rad_plugin_contract::Gesture::Hold],
        }],
        visuals: vec![],
        translations: Default::default(),
    };
    a.install_plugin(manifest, 1, "x86_64-unknown-linux-gnu")
        .unwrap();
    a
}
fn start(a: &mut Authority) -> Vec<DispatchIntent> {
    let mut pressed = [false; 32];
    pressed[0] = true;
    a.input_report(1, pressed)
        .unwrap()
        .into_iter()
        .filter_map(|e| {
            if let Effect::Dispatch(i) = e {
                Some(i)
            } else {
                None
            }
        })
        .collect()
}
#[test]
fn release_cancels_future_sequential_starts_and_retains_original_snapshot_until_end() {
    let mut a = setup(Mode::Sequential);
    let jobs = start(&mut a);
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.control.unwrap().gesture, ControlGesture::HoldStart);
    assert_eq!(
        a.job_settings(job.id).unwrap(),
        br#"{"schema_version":1,"values":{}}"#
    );
    let release = a.input_report(2, [false; 32]).unwrap();
    assert!(
        release
            .iter()
            .any(|e| matches!(e,Effect::ReleaseHold{execution} if *execution==job.id.execution()))
    );
    assert!(
        a.job_complete(job.id, Completion::Succeeded)
            .unwrap()
            .iter()
            .all(|e| !matches!(e, Effect::Dispatch(_)))
    );
    assert_eq!(a.plugin_change_allowed("fixture"), Err(PluginError::InUse));
    a.hold_released(job.id.execution());
    assert_eq!(a.plugin_change_allowed("fixture"), Ok(()));
}
#[test]
fn completed_parallel_starts_remain_pinned_until_disconnect_ends_them() {
    let mut a = setup(Mode::Parallel);
    let jobs = start(&mut a);
    assert_eq!(jobs.len(), 2);
    for job in &jobs {
        a.job_complete(job.id, Completion::Succeeded).unwrap();
    }
    assert_eq!(a.plugin_change_allowed("fixture"), Err(PluginError::InUse));
    let release = a.input_disconnect(2).unwrap();
    assert_eq!(
        release
            .iter()
            .filter(|e| matches!(e, Effect::ReleaseHold { .. }))
            .count(),
        1
    );
    a.hold_released(jobs[0].id.execution());
    assert_eq!(a.plugin_change_allowed("fixture"), Ok(()));
}

#[test]
fn release_does_not_fail_an_already_dispatched_parallel_plan() {
    let mut a = setup(Mode::Parallel);
    let jobs = start(&mut a);
    a.input_report(2, [false; 32]).unwrap();
    for job in &jobs {
        a.job_complete(job.id, Completion::Succeeded).unwrap();
    }
    assert_eq!(
        a.result(Origin::Editor, jobs[0].id.execution())
            .unwrap()
            .status,
        ExecutionStatus::Running
    );
    a.hold_released(jobs[0].id.execution());
    assert_eq!(
        a.result(Origin::Editor, jobs[0].id.execution())
            .unwrap()
            .status,
        ExecutionStatus::Succeeded
    );
}

#[test]
fn hold_end_failure_updates_original_result_and_survives_result_ttl() {
    let mut a = setup(Mode::Parallel);
    let jobs = start(&mut a);
    let id = jobs[0].id.execution();
    for job in &jobs {
        a.job_complete(job.id, Completion::Succeeded).unwrap();
    }
    a.before_event(RESULT_TTL_MS + 2).unwrap();
    assert!(
        a.result(Origin::Editor, id).is_some(),
        "held result must outlive ordinary TTL"
    );
    let before = a.execution_revision();
    a.report_hold_end_failure(id);
    assert_eq!(
        a.result(Origin::Editor, id).unwrap().status,
        ExecutionStatus::Failed
    );
    assert_ne!(a.execution_revision(), before);
}

#[test]
fn native_sleep_ends_original_hold_and_cancels_its_unsent_steps() {
    use rad_authority::{Command, Outcome};
    for simultaneous in [false, true] {
        let mut a = setup(Mode::Sequential);
        fn apply(a: &mut Authority, edit: Edit) {
            let ticket = a.reserve(1, Origin::Editor).unwrap();
            let request = ticket.request();
            let effects = a.submit(ticket, Command::Edits(vec![edit]));
            let id = effects
                .iter()
                .find_map(|e| {
                    if let Effect::Persist { id, .. } = e {
                        Some(*id)
                    } else {
                        None
                    }
                })
                .unwrap();
            let revision = a.editor_view(Origin::Editor).unwrap().revision + 1;
            a.complete(
                id,
                Outcome::Applied {
                    revision,
                    changed: true,
                },
            );
            a.finish(request);
        }
        let root = a.status(Origin::Editor).active.unwrap();
        apply(
            &mut a,
            Edit::CreateButton {
                folder: root.identity(),
                position: 1,
                name: "Sleep".into(),
            },
        );
        let folder = a.folder(Origin::Editor, root.short()).unwrap();
        let rad_authority::Visible::Visible(sleep) = folder.slots[1].clone().unwrap() else {
            panic!("visible");
        };
        apply(
            &mut a,
            Edit::AddAction {
                button: sleep.identity(),
                kind: ActionKind::Sleep,
            },
        );
        let (effects, jobs) = if simultaneous {
            a.input_report(0, [false; 32]).unwrap();
            let mut down = [false; 32];
            down[0] = true;
            down[1] = true;
            let effects = a.input_report(1, down).unwrap();
            let jobs = effects
                .iter()
                .filter_map(|e| {
                    if let Effect::Dispatch(job) = e {
                        Some(job.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            (effects, jobs)
        } else {
            let jobs = start(&mut a);
            let ticket = a.reserve(1, Origin::Editor).unwrap();
            let effects = a.submit(
                ticket,
                Command::Run {
                    target: Target::Button(sleep.identity()),
                },
            );
            (effects, jobs)
        };
        assert_eq!(jobs.len(), 1);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Display { asleep: true, .. }))
        );
        assert!(effects.iter().any(
            |e| matches!(e,Effect::ReleaseHold{execution} if *execution==jobs[0].id.execution())
        ));
        assert!(
            !a.job_complete(jobs[0].id, Completion::Succeeded)
                .unwrap()
                .iter()
                .any(|e| matches!(e, Effect::Dispatch(_)))
        );
        assert_eq!(a.plugin_change_allowed("fixture"), Err(PluginError::InUse));
        a.hold_released(jobs[0].id.execution());
        assert_eq!(a.plugin_change_allowed("fixture"), Ok(()));
    }
}
