use rad_authority::{Authority, Command, Effect, plugins::PluginError};
use rad_core::{configuration::*, execution::*, identity::*};
use rad_plugin_contract::{Definition, Manifest, Schema, Version};
fn manifest(major: u32) -> Manifest {
    Manifest {
        lifecycle: Default::default(),
        contract: 1,
        id: "fixture".into(),
        version: Version {
            major,
            minor: 0,
            patch: 0,
        },
        host_min: 1,
        host_max: 1,
        targets: vec!["x86_64-unknown-linux-gnu".into()],
        actions: vec![Definition {
            response_timeout: None,
            id: "run".into(),
            label: "Run".into(),
            schema: Schema {
                version: 1,
                fields: vec![],
            },
            credential_slots: vec![],
        }],
        visuals: vec![],
    }
}
fn edit(c: Candidate, e: Edit) -> Candidate {
    c.configuration()
        .prepare_edit(c.registry(), Origin::Editor, e)
        .unwrap()
}
fn fixture() -> Candidate {
    let c = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let folder = c.configuration().data().principal;
    let c = edit(
        c,
        Edit::CreateButton {
            folder,
            position: 0,
            name: "Run".into(),
        },
    );
    let button = c.configuration().data().buttons[0].binding.identity();
    let c = edit(
        c,
        Edit::AddAction {
            button,
            kind: ActionKind::Wait(Wait::new(100).unwrap()),
        },
    );
    edit(
        c,
        Edit::AddAction {
            button,
            kind: ActionKind::External {
                type_key: ExternalType::new("fixture".into(), "run".into()).unwrap(),
                parameters: br#"{"schema_version":1,"values":{}}"#.to_vec(),
            },
        },
    )
}
fn submit(a: &mut Authority, cmd: impl Into<Command>) -> Vec<Effect> {
    let t = a.reserve(1, Origin::Editor).unwrap();
    a.submit(t, cmd)
}
#[test]
fn admitted_wait_reserves_exact_version_until_finish_and_residual_is_distinct() {
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(7).unwrap());
    a.install_plugin(manifest(1), 10, "x86_64-unknown-linux-gnu")
        .unwrap();
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Accepted { .. })));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Dispatch(_))));
    assert_eq!(a.plugin_change_allowed("fixture"), Err(PluginError::InUse));
    assert_eq!(
        a.install_plugin(manifest(2), 11, "x86_64-unknown-linux-gnu"),
        Err(PluginError::InUse)
    );
    let effects = a.before_event(100).unwrap();
    let dispatch = effects
        .into_iter()
        .find_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(a.job_plugin(dispatch.id), Some(("fixture", 10)));
    a.before_event(5100).unwrap();
    assert_eq!(
        a.plugin_change_allowed("fixture"),
        Err(PluginError::Residual)
    );
    a.job_complete(dispatch.id, Completion::Succeeded).unwrap();
    assert_eq!(a.plugin_change_allowed("fixture"), Ok(()));
    a.install_plugin(manifest(2), 11, "x86_64-unknown-linux-gnu")
        .unwrap();
    // No queued version activation: the caller explicitly requested it again.
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Accepted { .. })));
    let d = a
        .before_event(5200)
        .unwrap()
        .into_iter()
        .find_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(a.job_plugin(d.id), Some(("fixture", 11)));
}
#[test]
fn schema_edits_reject_before_persist_and_failed_executor_rejects_whole_button() {
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let action = c.configuration().data().buttons[0].actions[1]
        .binding
        .identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(8).unwrap());
    a.install_plugin(manifest(1), 1, "x86_64-unknown-linux-gnu")
        .unwrap();
    let e = submit(
        &mut a,
        vec![Edit::SetParameters {
            action,
            parameters: ActionParameters::External(
                br#"{"schema_version":1,"values":{"unknown":true}}"#.to_vec(),
            ),
        }],
    );
    assert!(
        matches!(e.as_slice(), [Effect::Rejected { .. }]),
        "must reject before persistence"
    );
    a.plugin_failed("fixture", 1);
    let e = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(matches!(e.as_slice(), [Effect::Rejected { .. }]));
    assert_eq!(a.plugin_change_allowed("fixture"), Ok(()));
}
#[test]
fn missing_external_executor_rejects_before_a_healthy_launcher_effect() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let c = edit(
        c,
        Edit::AddAction {
            button: b,
            kind: ActionKind::Launcher(DesktopEntry::new("safe.desktop".into()).unwrap()),
        },
    );
    let mut order: Vec<_> = c.configuration().data().buttons[0]
        .actions
        .iter()
        .map(|a| a.binding.identity())
        .collect();
    order.rotate_right(1);
    let c = edit(
        c,
        Edit::SetPlan {
            button: b,
            mode: Mode::Sequential,
            order,
        },
    );
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(9).unwrap());
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
}
#[test]
fn pending_publication_cannot_race_a_version_swap() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(88).unwrap());
    a.install_plugin(manifest(1), 1, "x86_64-unknown-linux-gnu")
        .unwrap();
    let effects = submit(
        &mut a,
        vec![Edit::SetName {
            button: b,
            name: "pending".into(),
        }],
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Persist { .. })));
    assert!(
        a.install_plugin(manifest(2), 2, "x86_64-unknown-linux-gnu")
            .is_err(),
        "cannot swap while publication is unconfirmed"
    );
}
#[test]
fn denied_automation_does_not_reveal_missing_plugin_before_permission_check() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(99).unwrap());
    let ticket = a.reserve(1, Origin::Automation).unwrap();
    let effects = a.submit(
        ticket,
        Command::Run {
            target: Target::Button(b),
        },
    );
    assert!(matches!(
        effects.as_slice(),
        [Effect::Rejected {
            reason: rad_authority::Rejection::Execution(ExecutionError::PermissionDenied),
            ..
        }]
    ));
}

#[test]
fn administrative_barrier_is_owner_bound_and_blocks_only_affected_runs_until_explicit_finish() {
    use rad_authority::plugins::{PluginChange, Selection};
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let wait = c.configuration().data().buttons[0].actions[0]
        .binding
        .identity();
    let mut a = Authority::new(c.clone(), 1, std::num::NonZeroU128::new(7).unwrap());
    let mut b = Authority::new(c, 1, std::num::NonZeroU128::new(7).unwrap());
    let change = PluginChange::Select {
        operation: Selection::Update,
        manifest: manifest(2),
    };
    for actor in [&mut a, &mut b] {
        actor
            .install_plugin(manifest(1), 1, "x86_64-unknown-linux-gnu")
            .unwrap();
        assert!(
            actor
                .begin_plugin_change(
                    Origin::Automation,
                    change.clone(),
                    "x86_64-unknown-linux-gnu"
                )
                .is_err()
        );
    }
    let mut pa = Some(
        a.begin_plugin_change(Origin::Editor, change.clone(), "x86_64-unknown-linux-gnu")
            .unwrap(),
    );
    let mut pb = Some(
        b.begin_plugin_change(Origin::Editor, change.clone(), "x86_64-unknown-linux-gnu")
            .unwrap(),
    );
    assert!(b.finish_plugin_change(&mut pa, false).is_err());
    assert!(pa.is_some());
    assert!(
        b.begin_plugin_change(Origin::Editor, change.clone(), "x86_64-unknown-linux-gnu")
            .is_err()
    );
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::Accepted { .. } | Effect::Dispatch(_)))
    );
    let effects = submit(
        &mut a,
        vec![Edit::SetName {
            button,
            name: "blocked publication".into(),
        }],
    );
    assert!(!effects.iter().any(|e| matches!(e, Effect::Persist { .. })));
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::Rejected {
                reason: rad_authority::Rejection::PublicationPending,
                ..
            }
        )),
        "blocked edits report publication_pending, not capacity"
    );
    // Pure Wait selected individually does not use the plugin, despite its sibling.
    assert!(
        submit(
            &mut a,
            Command::Run {
                target: Target::Action(wait)
            }
        )
        .iter()
        .any(|e| matches!(e, Effect::Accepted { .. }))
    );
    a.before_event(100).unwrap();
    a.finish_plugin_change(&mut pa, false).unwrap();
    assert!(pa.is_none());
    b.finish_plugin_change(&mut pb, false).unwrap();
    assert!(a.plugin_change_allowed("fixture").is_ok());
    assert!(
        submit(
            &mut a,
            Command::Run {
                target: Target::Button(button)
            }
        )
        .iter()
        .any(|e| matches!(e, Effect::Accepted { .. }))
    );
    assert!(
        a.begin_plugin_change(Origin::Editor, change, "x86_64-unknown-linux-gnu")
            .is_err()
    );
}

#[test]
fn durable_selection_stays_guarded_through_activation_without_config_or_history_changes() {
    use rad_authority::plugins::{PluginChange, Selection};
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(7).unwrap());
    a.install_plugin(manifest(1), 1, "x86_64-unknown-linux-gnu")
        .unwrap();
    let before = a.editor_view(Origin::Editor).unwrap();
    let mut permit = Some(
        a.begin_plugin_change(
            Origin::Editor,
            PluginChange::Select {
                operation: Selection::Update,
                manifest: manifest(2),
            },
            "x86_64-unknown-linux-gnu",
        )
        .unwrap(),
    );
    assert!(
        a.install_plugin(manifest(2), 2, "x86_64-unknown-linux-gnu")
            .is_err()
    );
    a.allow_plugin_activation(permit.as_ref().unwrap()).unwrap();
    assert!(
        a.install_plugin(manifest(1), 2, "x86_64-unknown-linux-gnu")
            .is_err()
    );
    a.install_plugin(manifest(2), 2, "x86_64-unknown-linux-gnu")
        .unwrap();
    assert!(
        !submit(
            &mut a,
            Command::Run {
                target: Target::Button(b)
            }
        )
        .iter()
        .any(|e| matches!(e, Effect::Accepted { .. }))
    );
    a.finish_plugin_change(&mut permit, true).unwrap();
    assert_eq!(a.editor_view(Origin::Editor).unwrap(), before);
    assert!(
        submit(
            &mut a,
            Command::Run {
                target: Target::Button(b)
            }
        )
        .iter()
        .any(|e| matches!(e, Effect::Accepted { .. }))
    );
}

#[test]
fn authority_accepts_shared_thirty_two_plugin_budget() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(909).unwrap()).unwrap();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(909).unwrap());
    for n in 0..32 {
        let mut m = manifest(1);
        m.id = format!("fixture{n}");
        assert_eq!(
            a.install_plugin(m, 1, "x86_64-unknown-linux-gnu"),
            Ok(()),
            "plugin {n}"
        );
    }
    let mut m = manifest(1);
    m.id = "overflow".into();
    assert_eq!(
        a.install_plugin(m, 1, "x86_64-unknown-linux-gnu"),
        Err(PluginError::PluginLimit)
    );
}

#[test]
fn admission_captures_declared_response_budget_before_wait_and_parameter_edit() {
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let action = c.configuration().data().buttons[0].actions[1]
        .binding
        .identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(771).unwrap());
    let mut m = manifest(1);
    m.host_min = 3;
    m.host_max = 3;
    m.actions[0].schema.fields.push(rad_plugin_contract::Field {
        id: "timeout_seconds".into(),
        label: "Timeout".into(),
        required: false,
        kind: rad_plugin_contract::FieldKind::Integer { min: 1, max: 120 },
    });
    m.actions[0].response_timeout = Some(rad_plugin_contract::ResponseTimeout {
        field: "timeout_seconds".into(),
        default_seconds: 30,
    });
    a.install_plugin(m, 1, "x86_64-unknown-linux-gnu").unwrap();
    submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    let edits = submit(
        &mut a,
        vec![Edit::SetParameters {
            action,
            parameters: ActionParameters::External(
                br#"{"schema_version":1,"values":{"timeout_seconds":1}}"#.to_vec(),
            ),
        }],
    );
    let commit = edits
        .iter()
        .find_map(|e| {
            if let Effect::Persist { id, .. } = e {
                Some(*id)
            } else {
                None
            }
        })
        .unwrap();
    a.complete(
        commit,
        rad_authority::Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    let effects = a.before_event(100).unwrap();
    let dispatch = effects
        .into_iter()
        .find_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(dispatch.deadline_ms, 30_100);
    a.before_event(5100).unwrap();
    assert!(a.job_plugin(dispatch.id).is_some());
    assert_eq!(a.plugin_change_allowed("fixture"), Err(PluginError::InUse));
    a.before_event(30_100).unwrap();
    assert_eq!(
        a.plugin_change_allowed("fixture"),
        Err(PluginError::Residual)
    );
    a.job_complete(dispatch.id, Completion::Succeeded).unwrap();
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Action(action),
        },
    );
    let next = effects
        .into_iter()
        .find_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(next.deadline_ms, 31_100);
}

fn credential_values(value: &str) -> std::sync::Arc<rad_plugin_contract::CredentialValues> {
    std::sync::Arc::new(
        rad_plugin_contract::CredentialValues::new(vec![rad_plugin_contract::CredentialValue {
            slot: "password".into(),
            value: value.into(),
        }])
        .unwrap(),
    )
}
fn credential_manifest() -> Manifest {
    let mut m = manifest(1);
    m.host_min = 3;
    m.host_max = 3;
    m.actions[0].credential_slots.push("password".into());
    m
}
#[test]
fn credentials_are_captured_before_wait_and_replacement_does_not_change_the_job() {
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(772).unwrap());
    a.install_plugin(credential_manifest(), 1, "x86_64-unknown-linux-gnu")
        .unwrap();
    a.publish_plugin_credentials("fixture", 1, 1, credential_values("old-canary"))
        .unwrap();
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Accepted { .. })));
    a.publish_plugin_credentials("fixture", 1, 2, credential_values("new-canary"))
        .unwrap();
    let dispatch = a
        .before_event(100)
        .unwrap()
        .into_iter()
        .find_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d)
            } else {
                None
            }
        })
        .unwrap();
    let captured = a
        .job_credentials(dispatch.id)
        .expect("admission captured credentials before wait");
    assert_eq!(captured.revision(), 1);
    assert_eq!(captured.values().as_slice()[0].value, "old-canary");
    assert_eq!(
        a.current_plugin_credentials("fixture", 1)
            .unwrap()
            .revision(),
        2
    );
    assert!(!format!("{captured:?}").contains("canary"));
}

#[test]
fn terminal_result_keeps_only_the_residual_jobs_plugin_map_not_the_whole_execution() {
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let c = edit(
        c,
        Edit::AddAction {
            button,
            kind: ActionKind::External {
                type_key: ExternalType::new("other".into(), "run".into()).unwrap(),
                parameters: br#"{"schema_version":1,"values":{}}"#.to_vec(),
            },
        },
    );
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(773).unwrap());
    a.install_plugin(credential_manifest(), 1, "x86_64-unknown-linux-gnu")
        .unwrap();
    let mut other = credential_manifest();
    other.id = "other".into();
    a.install_plugin(other, 2, "x86_64-unknown-linux-gnu")
        .unwrap();
    let first = credential_values("first-canary");
    let second = credential_values("second-canary");
    let first_weak = std::sync::Arc::downgrade(&first);
    let second_weak = std::sync::Arc::downgrade(&second);
    a.publish_plugin_credentials("fixture", 1, 1, first)
        .unwrap();
    a.publish_plugin_credentials("other", 2, 1, second).unwrap();
    submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    a.publish_plugin_credentials("fixture", 1, 2, credential_values("new-first"))
        .unwrap();
    a.publish_plugin_credentials("other", 2, 2, credential_values("new-second"))
        .unwrap();
    assert_eq!(
        a.retained_plugin_credentials("fixture", 1)
            .iter()
            .map(|c| c.revision())
            .collect::<Vec<_>>(),
        [1, 2]
    );
    let dispatch = a
        .before_event(100)
        .unwrap()
        .into_iter()
        .find_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d)
            } else {
                None
            }
        })
        .unwrap();
    a.before_event(5100).unwrap();
    assert!(
        first_weak.upgrade().is_some(),
        "real residual owns its plugin map"
    );
    assert!(
        second_weak.upgrade().is_none(),
        "not-dispatched plugin must not survive in terminal result"
    );
    assert_eq!(
        a.retained_plugin_credentials("fixture", 1)
            .iter()
            .map(|c| c.revision())
            .collect::<Vec<_>>(),
        [1, 2]
    );
    a.job_complete(dispatch.id, Completion::Succeeded).unwrap();
    assert!(
        first_weak.upgrade().is_none(),
        "terminal result does not own secrets"
    );
    assert_eq!(
        a.retained_plugin_credentials("fixture", 1)
            .iter()
            .map(|c| c.revision())
            .collect::<Vec<_>>(),
        [2]
    );
    assert!(a.job_credentials(dispatch.id).is_none());
}

#[test]
fn unknown_blocks_only_new_snapshots_and_keymissing_can_publish_captured_absence() {
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(774).unwrap());
    a.install_plugin(credential_manifest(), 1, "x86_64-unknown-linux-gnu")
        .unwrap();
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(
        matches!(effects.as_slice(), [Effect::Rejected { .. }]),
        "no authenticated map has been published yet"
    );
    let empty = std::sync::Arc::new(rad_plugin_contract::CredentialValues::new(vec![]).unwrap());
    a.publish_plugin_credentials("fixture", 1, 1, empty)
        .unwrap();
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Accepted { .. })));
    a.invalidate_plugin_credentials("fixture", 1).unwrap();
    assert!(a.current_plugin_credentials("fixture", 1).is_none());
    let dispatch = a
        .before_event(100)
        .unwrap()
        .into_iter()
        .find_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d)
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        a.job_credentials(dispatch.id)
            .unwrap()
            .values()
            .as_slice()
            .is_empty()
    );
    a.job_complete(dispatch.id, Completion::Succeeded).unwrap();
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(matches!(effects.as_slice(), [Effect::Rejected { .. }]));
    a.publish_plugin_credentials("fixture", 1, 2, credential_values("reentered"))
        .unwrap();
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Accepted { .. })));
}

#[test]
fn concurrent_runs_keep_distinct_revisions_and_repeated_save_coalesces_unowned_maps() {
    let mut c = fixture();
    let first_button = c.configuration().data().buttons[0].binding.identity();
    let root = c.configuration().data().principal;
    c = edit(
        c,
        Edit::CreateButton {
            folder: root,
            position: 1,
            name: "Second".into(),
        },
    );
    let second_button = c.configuration().data().buttons[1].binding.identity();
    c = edit(
        c,
        Edit::AddAction {
            button: second_button,
            kind: ActionKind::Wait(Wait::new(100).unwrap()),
        },
    );
    c = edit(
        c,
        Edit::AddAction {
            button: second_button,
            kind: ActionKind::External {
                type_key: ExternalType::new("fixture".into(), "run".into()).unwrap(),
                parameters: br#"{"schema_version":1,"values":{}}"#.to_vec(),
            },
        },
    );
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(775).unwrap());
    a.install_plugin(credential_manifest(), 1, "x86_64-unknown-linux-gnu")
        .unwrap();
    let initial_view = a.editor_view(Origin::Editor).unwrap();
    a.publish_plugin_credentials("fixture", 1, 1, credential_values("first"))
        .unwrap();
    submit(
        &mut a,
        Command::Run {
            target: Target::Button(first_button),
        },
    );
    a.publish_plugin_credentials("fixture", 1, 2, credential_values("second"))
        .unwrap();
    submit(
        &mut a,
        Command::Run {
            target: Target::Button(second_button),
        },
    );
    for revision in (3..100).step_by(2) {
        let values = credential_values("unowned");
        let weak = std::sync::Arc::downgrade(&values);
        a.publish_plugin_credentials("fixture", 1, revision, values)
            .unwrap();
        a.publish_plugin_credentials("fixture", 1, revision + 1, credential_values("current"))
            .unwrap();
        assert!(weak.upgrade().is_none());
    }
    assert_eq!(
        a.retained_plugin_credentials("fixture", 1)
            .iter()
            .map(|c| c.revision())
            .collect::<Vec<_>>(),
        [1, 2, 100]
    );
    let effects = a.before_event(100).unwrap();
    let jobs: Vec<_> = effects
        .iter()
        .filter_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d.id)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(jobs.len(), 2);
    let revisions: std::collections::BTreeSet<_> = jobs
        .iter()
        .map(|id| a.job_credentials(*id).unwrap().revision())
        .collect();
    assert_eq!(revisions, [1, 2].into());
    for job in jobs {
        a.job_complete(job, Completion::Succeeded).unwrap();
    }
    assert_eq!(a.retained_plugin_credentials("fixture", 1).len(), 1);
    assert_eq!(
        a.editor_view(Origin::Editor).unwrap(),
        initial_view,
        "credential publication is not configuration or undo history"
    );
}

#[test]
fn trusted_credential_publication_rejects_stale_selection_revision_and_undeclared_slots() {
    let mut a = Authority::new(fixture(), 1, std::num::NonZeroU128::new(776).unwrap());
    a.install_plugin(credential_manifest(), 9, "x86_64-unknown-linux-gnu")
        .unwrap();
    let selection = a.plugin_manifests().next().unwrap().1;
    assert!(
        a.publish_plugin_credentials("missing", 9, 1, credential_values("x"))
            .is_err()
    );
    assert!(
        a.publish_plugin_credentials("fixture", 8, 1, credential_values("x"))
            .is_err()
    );
    assert!(
        a.publish_plugin_credentials("fixture", selection, 0, credential_values("x"))
            .is_err()
    );
    a.publish_plugin_credentials("fixture", selection, 1, credential_values("original"))
        .unwrap();
    assert!(
        a.publish_plugin_credentials("fixture", selection, 1, credential_values("replacement"))
            .is_err()
    );
    let other = std::sync::Arc::new(
        rad_plugin_contract::CredentialValues::new(vec![rad_plugin_contract::CredentialValue {
            slot: "foreign".into(),
            value: "x".into(),
        }])
        .unwrap(),
    );
    assert!(
        a.publish_plugin_credentials("fixture", selection, 2, other)
            .is_err()
    );
    assert_eq!(
        a.current_plugin_credentials("fixture", selection)
            .unwrap()
            .values()
            .as_slice()[0]
            .value,
        "original"
    );
    assert!(a.invalidate_plugin_credentials("fixture", 8).is_err());
    a.invalidate_plugin_credentials("fixture", selection)
        .unwrap();
    assert!(
        a.publish_plugin_credentials("fixture", selection, 1, credential_values("reused"))
            .is_err()
    );
}

#[test]
fn process_replacement_keeps_the_selected_metadata_identity() {
    let mut a = Authority::new(fixture(), 1, std::num::NonZeroU128::new(7).unwrap());
    a.install_plugin(manifest(1), 11, "x86_64-unknown-linux-gnu")
        .unwrap();
    let selection = a.plugin_manifests().next().unwrap().1;
    assert_ne!(selection, 0);
    a.plugin_failed("fixture", 11);
    a.install_plugin(manifest(1), 12, "x86_64-unknown-linux-gnu")
        .unwrap();
    assert_eq!(a.plugin_manifests().next().unwrap().1, selection);
}

#[test]
fn dormant_metadata_admits_wait_and_pins_selection_credentials_and_generation_once() {
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let root = c.configuration().data().principal;
    let c = edit(
        c,
        Edit::CreateFolder {
            parent: root,
            position: 1,
            name: "empty".into(),
        },
    );
    let empty = c.configuration().data().folders[1].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(779).unwrap());
    a.register_plugin(credential_manifest(), "x86_64-unknown-linux-gnu")
        .unwrap();
    let (metadata, selection, ready) = a.plugin_manifests().next().unwrap();
    assert_eq!(metadata, &credential_manifest());
    assert!(!ready);
    assert_eq!(a.plugin_generation("fixture"), None);
    assert!(a.plugin_available("fixture", selection));
    a.publish_plugin_credentials("fixture", selection, 1, credential_values("before"))
        .unwrap();
    assert_eq!(a.plugin_demand(), ["fixture".to_owned()].into());
    a.navigate(Origin::Editor, empty).unwrap();
    assert!(a.plugin_demand().is_empty());
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Accepted { .. })));
    assert_eq!(
        a.plugin_demand(),
        ["fixture".to_owned()].into(),
        "future step after Wait owns demand outside active folder"
    );
    assert_eq!(
        a.register_plugin(credential_manifest(), "x86_64-unknown-linux-gnu"),
        Err(PluginError::InUse)
    );
    a.publish_plugin_credentials("fixture", selection, 2, credential_values("after"))
        .unwrap();
    a.activate_plugin("fixture", selection, 71).unwrap();
    assert_eq!(a.deactivate_plugin("fixture", 71), Err(PluginError::InUse));
    a.plugin_failed("fixture", 71);
    a.activate_plugin("fixture", selection, 72).unwrap();
    let dispatch = a
        .before_event(100)
        .unwrap()
        .into_iter()
        .find_map(|e| match e {
            Effect::Dispatch(d) => Some(d),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        a.job_plugin_selection(dispatch.id),
        Some(("fixture", selection))
    );
    assert_eq!(
        a.job_plugin(dispatch.id),
        Some(("fixture", 71)),
        "failed generation is never replayed on a replacement"
    );
    assert_eq!(
        a.job_credentials(dispatch.id).unwrap().values().as_slice()[0].value,
        "before"
    );
    a.before_event(5100).unwrap();
    assert_eq!(
        a.plugin_change_allowed("fixture"),
        Err(PluginError::Residual)
    );
    assert_eq!(
        a.plugin_demand(),
        ["fixture".to_owned()].into(),
        "residual owns demand after result is terminal"
    );
    a.job_complete(dispatch.id, Completion::Succeeded).unwrap();
    assert!(a.plugin_demand().is_empty());
    a.deactivate_plugin("fixture", 72).unwrap();
    assert_eq!(a.plugin_manifests().next().unwrap().1, selection);
    assert_eq!(
        a.current_plugin_credentials("fixture", selection)
            .unwrap()
            .revision(),
        2
    );
}

#[test]
fn dormant_dispatch_binds_on_first_activation_and_explicit_reinstall_gets_fresh_selection() {
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(780).unwrap());
    a.register_plugin(manifest(1), "x86_64-unknown-linux-gnu")
        .unwrap();
    let selection = a.plugin_manifests().next().unwrap().1;
    submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    let job = a
        .before_event(100)
        .unwrap()
        .into_iter()
        .find_map(|e| match e {
            Effect::Dispatch(d) => Some(d.id),
            _ => None,
        })
        .unwrap();
    assert_eq!(a.job_plugin(job), None);
    assert_eq!(a.job_plugin_selection(job), Some(("fixture", selection)));
    assert_eq!(
        a.activate_plugin("fixture", selection, 0),
        Err(PluginError::Incompatible)
    );
    assert_eq!(
        a.activate_plugin("fixture", selection + 1, 33),
        Err(PluginError::Incompatible)
    );
    a.activate_plugin("fixture", selection, 33).unwrap();
    assert_eq!(a.job_plugin(job), Some(("fixture", 33)));
    a.job_complete(job, Completion::Succeeded).unwrap();
    a.deactivate_plugin("fixture", 33).unwrap();
    assert!(a.plugin_available("fixture", selection));
    a.fail_plugin_selection("fixture", selection).unwrap();
    assert!(!a.plugin_available("fixture", selection));
    assert!(matches!(
        submit(
            &mut a,
            Command::Run {
                target: Target::Button(button)
            }
        )
        .as_slice(),
        [Effect::Rejected { .. }]
    ));
    a.register_plugin(manifest(1), "x86_64-unknown-linux-gnu")
        .unwrap();
    let next = a.plugin_manifests().next().unwrap().1;
    assert_ne!(selection, next);
    assert!(!a.plugin_available("fixture", selection));
    assert!(a.plugin_available("fixture", next));
    assert_eq!(
        a.activate_plugin("fixture", selection, 34),
        Err(PluginError::Incompatible)
    );
    a.activate_plugin("fixture", next, 34).unwrap();
}

#[test]
fn demand_includes_static_opt_in_and_only_active_folder_visuals_and_actions() {
    let c = fixture();
    let root = c.configuration().data().principal;
    let button = c.configuration().data().buttons[0].binding.identity();
    let c = edit(
        c,
        Edit::SetVisual {
            button,
            visual: VisualSource::External {
                type_key: ExternalType::new("visual-only".into(), "run".into()).unwrap(),
                configuration: br#"{"schema_version":1,"values":{}}"#.to_vec(),
            },
        },
    );
    let c = edit(
        c,
        Edit::CreateFolder {
            parent: root,
            position: 1,
            name: "empty".into(),
        },
    );
    let empty = c.configuration().data().folders[1].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(781).unwrap());
    a.register_plugin(manifest(1), "x86_64-unknown-linux-gnu")
        .unwrap();
    let mut visual = manifest(1);
    visual.id = "visual-only".into();
    visual.visuals = std::mem::take(&mut visual.actions);
    a.register_plugin(visual, "x86_64-unknown-linux-gnu")
        .unwrap();
    let mut background = manifest(1);
    background.id = "background".into();
    background.lifecycle = rad_plugin_contract::PluginLifecycle::AlwaysLoaded;
    a.register_plugin(background, "x86_64-unknown-linux-gnu")
        .unwrap();
    assert_eq!(
        a.plugin_demand(),
        [
            "fixture".to_owned(),
            "background".to_owned(),
            "visual-only".to_owned()
        ]
        .into()
    );
    a.navigate(Origin::Editor, empty).unwrap();
    assert_eq!(a.plugin_demand(), ["background".to_owned()].into());
    a.navigate(Origin::Editor, root).unwrap();
    assert_eq!(
        a.plugin_demand(),
        [
            "fixture".to_owned(),
            "background".to_owned(),
            "visual-only".to_owned()
        ]
        .into()
    );
}

#[test]
fn admission_during_normal_stop_waits_for_new_generation_without_rebinding_old_work() {
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(782).unwrap());
    a.install_plugin(manifest(1), 41, "x86_64-unknown-linux-gnu")
        .unwrap();
    let selection = a.plugin_manifests().next().unwrap().1;
    a.begin_plugin_stop("fixture", 41).unwrap();
    assert_eq!(
        a.plugin_generation("fixture"),
        None,
        "retiring process cannot capture a new admission"
    );
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Accepted { .. })));
    assert_eq!(a.plugin_change_allowed("fixture"), Err(PluginError::InUse));
    let dispatch = a
        .before_event(100)
        .unwrap()
        .into_iter()
        .find_map(|e| match e {
            Effect::Dispatch(d) => Some(d),
            _ => None,
        })
        .unwrap();
    assert_eq!(a.job_plugin(dispatch.id), None);
    assert_eq!(dispatch.deadline_ms, 5100);
    // Caller has now reaped generation 41 and completed the new handshake.
    a.activate_plugin("fixture", selection, 42).unwrap();
    assert_eq!(a.job_plugin(dispatch.id), Some(("fixture", 42)));
    assert_eq!(a.begin_plugin_stop("fixture", 42), Err(PluginError::InUse));
    a.before_event(5100).unwrap();
    assert_eq!(
        a.begin_plugin_stop("fixture", 42),
        Err(PluginError::Residual)
    );
    a.job_complete(dispatch.id, Completion::Succeeded).unwrap();
    a.begin_plugin_stop("fixture", 42).unwrap();
    assert_eq!(a.plugin_manifests().next().unwrap().1, selection);
}

#[test]
fn credential_requirements_use_installed_slots_and_never_expose_values() {
    let mut a = Authority::new(fixture(), 1, std::num::NonZeroU128::new(772).unwrap());
    assert!(a.plugin_credential_status("fixture").is_empty());
    a.install_plugin(credential_manifest(), 1, "x86_64-unknown-linux-gnu")
        .unwrap();
    assert_eq!(
        a.plugin_credential_status("fixture"),
        vec![("password".into(), false)]
    );
    a.publish_plugin_credentials("fixture", 1, 1, credential_values("private-canary"))
        .unwrap();
    assert_eq!(
        a.plugin_credential_status("fixture"),
        vec![("password".into(), true)]
    );
    assert!(!format!("{:?}", a.plugin_credential_status("fixture")).contains("private-canary"));
    a.invalidate_plugin_credentials("fixture", 1).unwrap();
    assert_eq!(
        a.plugin_credential_status("fixture"),
        vec![("password".into(), false)]
    );
}
