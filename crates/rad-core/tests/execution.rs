use rad_core::configuration::*;
use rad_core::execution::*;
use rad_core::identity::*;

fn empty_button() -> Candidate {
    let c = Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap();
    c.configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "A".into(),
            },
        )
        .unwrap()
}
fn button(c: &Candidate) -> EntityIdentity {
    c.configuration().data().buttons[0].binding.identity()
}
#[test]
fn empty_button_completes_immediately_without_retaining_occupation() {
    let c = empty_button();
    let mut engine = Engine::new(7, 1).unwrap();
    let first = engine
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    assert_eq!(
        engine.result(first.id).unwrap().status,
        ExecutionStatus::Succeeded
    );
    let second = engine
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Hardware,
            &[],
        )
        .unwrap();
    assert_ne!(first.id, second.id);
}

use rad_core::permissions::{LocalPermissions, Permissions};
fn edit(c: &Candidate, e: Edit) -> Candidate {
    c.configuration()
        .prepare_edit(c.registry(), Origin::Editor, e)
        .unwrap()
}
fn launcher() -> ActionKind {
    ActionKind::Launcher(DesktopEntry::new("app.desktop".into()).unwrap())
}
fn actions(n: usize) -> Candidate {
    let mut c = empty_button();
    let b = button(&c);
    for _ in 0..n {
        c = edit(
            &c,
            Edit::AddAction {
                button: b,
                kind: launcher(),
            },
        );
    }
    c
}
fn allow() -> LocalPermissions {
    LocalPermissions::own(Permissions::new(true, true, true).unwrap())
}
#[test]
fn admission_checks_all_permissions_and_installed_types_before_allocating_execution() {
    let c = actions(2);
    let b = button(&c);
    let a0 = c.configuration().data().buttons[0].actions[0]
        .binding
        .identity();
    let a1 = c.configuration().data().buttons[0].actions[1]
        .binding
        .identity();
    let mut engine = Engine::new(1, 1).unwrap();
    assert_eq!(
        engine
            .admit(
                c.configuration(),
                Target::Button(b),
                Trigger::Automation,
                &[]
            )
            .unwrap_err(),
        ExecutionError::PermissionDenied
    );
    let c = edit(
        &c,
        Edit::SetButtonPermissions {
            button: b,
            permissions: allow(),
        },
    );
    let c = edit(
        &c,
        Edit::SetActionPermissions {
            action: a1,
            permissions: LocalPermissions::own(Permissions::default()),
        },
    );
    assert_eq!(
        engine
            .admit(
                c.configuration(),
                Target::Button(b),
                Trigger::Automation,
                &[]
            )
            .unwrap_err(),
        ExecutionError::PermissionDenied
    );
    let c = edit(
        &c,
        Edit::SetButtonPermissions {
            button: b,
            permissions: LocalPermissions::own(Permissions::new(false, false, false).unwrap()),
        },
    );
    let c = edit(
        &c,
        Edit::SetActionPermissions {
            action: a0,
            permissions: allow(),
        },
    );
    let admitted = engine
        .admit(
            c.configuration(),
            Target::Action(a0),
            Trigger::Automation,
            &[],
        )
        .unwrap();
    assert_eq!(admitted.id.counter(), 1);
    let key = ExternalType::new("plugin".into(), "do".into()).unwrap();
    let c = edit(
        &c,
        Edit::ReplaceActionType {
            action: a1,
            kind: ActionKind::External {
                type_key: key.clone(),
                parameters: vec![],
            },
        },
    );
    for trigger in [Trigger::Hardware, Trigger::Editor] {
        let mut e = Engine::new(2, 1).unwrap();
        assert_eq!(
            e.admit(c.configuration(), Target::Button(b), trigger, &[])
                .unwrap_err(),
            ExecutionError::ExecutorUnavailable
        );
        assert!(
            e.admit(
                c.configuration(),
                Target::Button(b),
                trigger,
                std::slice::from_ref(&key)
            )
            .is_ok()
        );
    }
}

#[test]
fn all_triggers_and_siblings_share_owner_occupation_without_queue() {
    let c = actions(2);
    let b = button(&c);
    let children: Vec<_> = c.configuration().data().buttons[0]
        .actions
        .iter()
        .map(|a| a.binding.identity())
        .collect();
    for first in [Target::Button(b), Target::Action(children[0])] {
        let mut engine = Engine::new(1, 1).unwrap();
        engine
            .admit(c.configuration(), first, Trigger::Editor, &[])
            .unwrap();
        for trigger in [Trigger::Hardware, Trigger::Editor, Trigger::Automation] {
            for target in [
                Target::Button(b),
                Target::Action(children[0]),
                Target::Action(children[1]),
            ] {
                assert_eq!(
                    engine
                        .admit(c.configuration(), target, trigger, &[])
                        .unwrap_err(),
                    ExecutionError::Busy
                );
            }
        }
    }
}

#[test]
fn first_dispatch_captures_full_relevant_snapshot_not_future_edits_or_visual_availability() {
    let c = actions(2);
    let b = button(&c);
    let visual = VisualSource::External {
        type_key: ExternalType::new("missing".into(), "cover".into()).unwrap(),
        configuration: vec![42],
    };
    let c = edit(
        &c,
        Edit::SetVisual {
            button: b,
            visual: visual.clone(),
        },
    );
    let mut engine = Engine::new(3, 1).unwrap();
    let admission = engine
        .admit(c.configuration(), Target::Button(b), Trigger::Hardware, &[])
        .unwrap();
    assert_eq!(admission.dispatches.len(), 1);
    assert_eq!(admission.dispatches[0].deadline_ms, 5000);
    assert_eq!(
        admission.dispatches[0].operation,
        Operation::Launcher(DesktopEntry::new("app.desktop".into()).unwrap())
    );
    let original = c.configuration().data().buttons[0].clone();
    let changed = edit(
        &c,
        Edit::SetName {
            button: b,
            name: "changed".into(),
        },
    );
    let deleted = edit(&changed, Edit::DeleteButton { button: b });
    assert!(deleted.configuration().data().buttons.is_empty());
    let snapshot = engine.snapshot(admission.id).unwrap();
    assert_eq!(snapshot.button(), &original);
    assert_eq!(snapshot.button().visual, visual);
    assert_eq!(snapshot.target(), Target::Button(b));
    assert_eq!(snapshot.trigger(), Trigger::Hardware);
}

/// One button running `count` launchers in `mode`. With `wait_ms`, a wait of that length
/// sits between each pair, so launcher `k` is action `2 * k`; without, it is action `k`.
fn spaced(count: usize, mode: Mode, wait_ms: Option<u64>) -> Candidate {
    let mut c = empty_button();
    let b = button(&c);
    for i in 0..count {
        if let (Some(milliseconds), true) = (wait_ms, i > 0) {
            c = edit(
                &c,
                Edit::AddAction {
                    button: b,
                    kind: ActionKind::Wait(Wait::new(milliseconds).unwrap()),
                },
            );
        }
        c = edit(
            &c,
            Edit::AddAction {
                button: b,
                kind: launcher(),
            },
        );
    }
    let order = c.configuration().data().buttons[0]
        .actions
        .iter()
        .map(|a| a.binding.identity())
        .collect();
    edit(
        &c,
        Edit::SetPlan {
            button: b,
            mode,
            order,
        },
    )
}
#[test]
fn sequential_success_then_wait_starts_next_response_budget_at_dispatch() {
    let c = spaced(2, Mode::Sequential, Some(100));
    let mut e = Engine::new(1, 1).unwrap();
    let first = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    assert_eq!(e.next_deadline(), Some(5000));
    assert!(e.advance(1000).unwrap().is_empty());
    assert!(
        e.complete(first.dispatches[0].id, Completion::Succeeded)
            .unwrap()
            .is_empty()
    );
    assert_eq!(e.next_deadline(), Some(1100));
    assert!(e.advance(1099).unwrap().is_empty());
    let second = e.advance(1100).unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].id.index(), 2);
    assert_eq!(second[0].deadline_ms, 6100);
    assert_eq!(e.result(first.id).unwrap().status, ExecutionStatus::Running);
    e.complete(second[0].id, Completion::Succeeded).unwrap();
    assert_eq!(
        e.result(first.id).unwrap().status,
        ExecutionStatus::Succeeded
    );
    assert!(
        e.admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Hardware,
            &[]
        )
        .is_ok()
    );
}

#[test]
fn service_availability_is_only_a_dispatch_result_and_failure_stops_sequential_pending() {
    let c = actions(3);
    let a = c.configuration().data().buttons[0].actions[0]
        .binding
        .identity();
    let key = ExternalType::new("obs".into(), "control".into()).unwrap();
    let c = edit(
        &c,
        Edit::ReplaceActionType {
            action: a,
            kind: ActionKind::External {
                type_key: key.clone(),
                parameters: vec![1],
            },
        },
    );
    let mut e = Engine::new(1, 1).unwrap();
    let run = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            std::slice::from_ref(&key),
        )
        .unwrap();
    assert_eq!(run.dispatches.len(), 1);
    assert!(
        e.complete(
            run.dispatches[0].id,
            Completion::Failed(Failure::ServiceUnavailable)
        )
        .unwrap()
        .is_empty()
    );
    let result = e.result(run.id).unwrap();
    assert_eq!(result.status, ExecutionStatus::Failed);
    assert_eq!(
        result.steps.iter().map(|s| s.status).collect::<Vec<_>>(),
        vec![
            StepStatus::Failed(Failure::ServiceUnavailable),
            StepStatus::NotDispatched,
            StepStatus::NotDispatched
        ]
    );
    assert!(e.advance(50_000).unwrap().is_empty());
    assert!(
        e.admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[key]
        )
        .is_ok()
    );
}

#[test]
fn parallel_waits_space_dispatches_and_failure_collects_launched_even_after_last_dispatch() {
    let c = spaced(3, Mode::Parallel, Some(500));
    let mut e = Engine::new(1, 1).unwrap();
    let a = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    assert_eq!(e.next_deadline(), Some(500));
    assert!(e.advance(499).unwrap().is_empty());
    let b = e.advance(500).unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].deadline_ms, 5500);
    assert_eq!(e.next_deadline(), Some(1000));
    e.advance(600).unwrap();
    assert!(
        e.complete(
            a.dispatches[0].id,
            Completion::Failed(Failure::ActionFailed)
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(e.result(a.id).unwrap().status, ExecutionStatus::Running);
    assert_eq!(
        e.result(a.id).unwrap().steps[4].status,
        StepStatus::NotDispatched
    );
    e.advance(900).unwrap();
    e.complete(b[0].id, Completion::Succeeded).unwrap();
    assert_eq!(e.result(a.id).unwrap().status, ExecutionStatus::Failed);
    assert!(e.advance(1000).unwrap().is_empty());
    let zero = spaced(3, Mode::Parallel, None);
    let all = e
        .admit(
            zero.configuration(),
            Target::Button(button(&zero)),
            Trigger::Hardware,
            &[],
        )
        .unwrap();
    assert_eq!(all.dispatches.len(), 3);
    e.complete(
        all.dispatches[2].id,
        Completion::Failed(Failure::ActionFailed),
    )
    .unwrap();
    assert_eq!(e.result(all.id).unwrap().status, ExecutionStatus::Running);
    e.complete(all.dispatches[0].id, Completion::Succeeded)
        .unwrap();
    e.complete(all.dispatches[1].id, Completion::Succeeded)
        .unwrap();
    assert_eq!(e.result(all.id).unwrap().status, ExecutionStatus::Failed);
    assert!(
        e.result(all.id)
            .unwrap()
            .steps
            .iter()
            .all(|s| s.status != StepStatus::NotDispatched)
    );
}

#[test]
fn timeout_frees_button_but_not_external_resource_and_late_result_cannot_complete_reentry() {
    let c = actions(2);
    let mut e = Engine::new(1, 1).unwrap();
    let old = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    assert_eq!(e.usage().jobs, 2);
    assert!(e.advance(4999).unwrap().is_empty());
    assert_eq!(e.result(old.id).unwrap().status, ExecutionStatus::Running);
    assert!(e.advance(5000).unwrap().is_empty());
    assert_eq!(e.result(old.id).unwrap().status, ExecutionStatus::TimedOut);
    assert_eq!(
        e.result(old.id).unwrap().steps[1].status,
        StepStatus::NotDispatched
    );
    assert_eq!(
        e.usage(),
        Usage {
            active_executions: 0,
            jobs: 1,
            residual_jobs: 1
        }
    );
    let new = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Hardware,
            &[],
        )
        .unwrap();
    assert_eq!(e.usage().jobs, 3);
    assert!(
        e.complete(old.dispatches[0].id, Completion::Succeeded)
            .unwrap()
            .is_empty()
    );
    assert_eq!(e.usage().jobs, 2);
    assert_eq!(e.result(old.id).unwrap().status, ExecutionStatus::TimedOut);
    assert_eq!(e.result(new.id).unwrap().status, ExecutionStatus::Running);
    assert_eq!(
        e.complete(old.dispatches[0].id, Completion::Succeeded)
            .unwrap_err(),
        ExecutionError::UnknownDispatch
    );
}

fn deck(count: usize, per_button: usize) -> Candidate {
    let mut c = Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap();
    let root = c.configuration().data().principal;
    for position in 0..count {
        c = edit(
            &c,
            Edit::CreateButton {
                folder: root,
                position,
                name: format!("B{position}"),
            },
        );
        let b = c.configuration().data().buttons[position]
            .binding
            .identity();
        for _ in 0..per_button {
            c = edit(
                &c,
                Edit::AddAction {
                    button: b,
                    kind: launcher(),
                },
            );
        }
    }
    c
}
#[test]
fn admission_caps_sixteen_executions_and_thirty_two_reserved_jobs_without_consuming_rejected_id() {
    let c = deck(17, 1);
    let mut e = Engine::new(1, 1).unwrap();
    let mut first = None;
    for b in &c.configuration().data().buttons[..16] {
        let a = e
            .admit(
                c.configuration(),
                Target::Button(b.binding.identity()),
                Trigger::Editor,
                &[],
            )
            .unwrap();
        first.get_or_insert(a);
    }
    let last = c.configuration().data().buttons[16].binding.identity();
    assert_eq!(
        e.admit(
            c.configuration(),
            Target::Button(last),
            Trigger::Editor,
            &[]
        )
        .unwrap_err(),
        ExecutionError::Capacity
    );
    e.complete(first.unwrap().dispatches[0].id, Completion::Succeeded)
        .unwrap();
    assert_eq!(
        e.admit(
            c.configuration(),
            Target::Button(last),
            Trigger::Editor,
            &[]
        )
        .unwrap()
        .id
        .counter(),
        17
    );
    let c = deck(6, 10);
    let mut e = Engine::new(2, 1).unwrap();
    for b in &c.configuration().data().buttons[..3] {
        e.admit(
            c.configuration(),
            Target::Button(b.binding.identity()),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    }
    assert_eq!(e.usage().jobs, 30);
    assert_eq!(
        e.admit(
            c.configuration(),
            Target::Button(c.configuration().data().buttons[3].binding.identity()),
            Trigger::Editor,
            &[]
        )
        .unwrap_err(),
        ExecutionError::Capacity
    );
    for b in &c.configuration().data().buttons[3..5] {
        e.admit(
            c.configuration(),
            Target::Action(b.actions[0].binding.identity()),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    }
    assert_eq!(e.usage().jobs, 32);
    assert_eq!(
        e.admit(
            c.configuration(),
            Target::Action(
                c.configuration().data().buttons[5].actions[0]
                    .binding
                    .identity()
            ),
            Trigger::Editor,
            &[]
        )
        .unwrap_err(),
        ExecutionError::Capacity
    );
}

#[test]
fn terminal_results_are_capped_at_256_without_evicting_active_execution() {
    let c = deck(2, 1);
    let b0 = c.configuration().data().buttons[0].binding.identity();
    let b1 = c.configuration().data().buttons[1].binding.identity();
    let mut e = Engine::new(1, 1).unwrap();
    let active = e
        .admit(c.configuration(), Target::Button(b0), Trigger::Editor, &[])
        .unwrap();
    let mut ids = vec![];
    for _ in 0..257 {
        let a = e
            .admit(c.configuration(), Target::Button(b1), Trigger::Editor, &[])
            .unwrap();
        e.complete(a.dispatches[0].id, Completion::Succeeded)
            .unwrap();
        ids.push(a.id);
    }
    assert!(e.result(ids[0]).is_none());
    assert!(ids[1..].iter().all(|id| e.result(*id).is_some()));
    assert_eq!(
        e.result(active.id).unwrap().status,
        ExecutionStatus::Running
    );
    assert_eq!(e.usage().active_executions, 1);
}

#[test]
fn terminal_ttl_is_ten_minutes_from_finish_and_late_termination_does_not_resurrect_it() {
    let c = empty_button();
    let mut e = Engine::new(1, 1).unwrap();
    let empty = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    assert_eq!(e.next_deadline(), Some(600_000));
    e.advance(599_999).unwrap();
    assert!(e.result(empty.id).is_some());
    e.advance(600_000).unwrap();
    assert!(e.result(empty.id).is_none());
    assert_eq!(e.next_deadline(), None);
    let c = actions(1);
    let timeout = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    e.advance(605_000).unwrap();
    assert_eq!(e.usage().residual_jobs, 1);
    e.advance(1_204_999).unwrap();
    assert!(e.result(timeout.id).is_some());
    e.advance(1_205_000).unwrap();
    assert!(e.result(timeout.id).is_none());
    e.complete(timeout.dispatches[0].id, Completion::Succeeded)
        .unwrap();
    assert_eq!(e.usage().jobs, 0);
    assert!(e.result(timeout.id).is_none());
}

#[test]
fn unrepresentable_response_and_wait_horizons_reject_before_effects_and_counters_never_wrap() {
    // Two responses and the wait between them need 610 000 ms: one less is left.
    let long = spaced(2, Mode::Sequential, Some(600_000));
    let mut e = Engine::new(1, 1).unwrap();
    e.advance(u64::MAX - 609_999).unwrap();
    assert_eq!(
        e.admit(
            long.configuration(),
            Target::Button(button(&long)),
            Trigger::Editor,
            &[]
        )
        .unwrap_err(),
        ExecutionError::TimeOverflow
    );
    assert_eq!(e.usage().jobs, 0);
    let c = actions(1);
    let a = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    assert_eq!(a.id.counter(), 1);
    let mut end = Engine::new(2, 1).unwrap();
    end.advance(u64::MAX - 4999).unwrap();
    assert_eq!(
        end.admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[]
        )
        .unwrap_err(),
        ExecutionError::TimeOverflow
    );
    assert_eq!(end.usage().jobs, 0);
    let empty = empty_button();
    let mut ids = Engine::new(3, u64::MAX).unwrap();
    let last = ids
        .admit(
            empty.configuration(),
            Target::Button(button(&empty)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    assert_eq!(last.id.counter(), u64::MAX);
    assert_eq!(
        ids.admit(
            empty.configuration(),
            Target::Button(button(&empty)),
            Trigger::Editor,
            &[]
        )
        .unwrap_err(),
        ExecutionError::CounterExhausted
    );
    assert!(Engine::new(0, 1).is_err());
    assert!(Engine::new(1, 0).is_err());
    let c = spaced(2, Mode::Sequential, Some(100));
    let mut e = Engine::new(4, 1).unwrap();
    let a = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    e.complete(a.dispatches[0].id, Completion::Succeeded)
        .unwrap();
    assert_eq!(
        e.advance(u64::MAX).unwrap_err(),
        ExecutionError::TimeOverflow
    );
    assert_eq!(e.next_deadline(), Some(100));
    // The wait in flight and the launcher behind it are both still reserved.
    assert_eq!(e.usage().jobs, 2);
    assert_eq!(e.advance(100).unwrap().len(), 1);
    assert_eq!(e.advance(99).unwrap_err(), ExecutionError::TimeReversed);
}

#[test]
fn residual_saturation_requires_real_termination_not_result_eviction_or_reentry() {
    let c = actions(1);
    let mut e = Engine::new(1, 1).unwrap();
    let mut jobs = vec![];
    for i in 0..32 {
        let a = e
            .admit(
                c.configuration(),
                Target::Button(button(&c)),
                Trigger::Editor,
                &[],
            )
            .unwrap();
        jobs.push(a.dispatches[0].id);
        e.advance((i + 1) * 5000).unwrap();
    }
    assert_eq!(
        e.usage(),
        Usage {
            active_executions: 0,
            jobs: 32,
            residual_jobs: 32
        }
    );
    e.advance(1_000_000).unwrap();
    assert_eq!(
        e.admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Hardware,
            &[]
        )
        .unwrap_err(),
        ExecutionError::Capacity
    );
    e.complete(jobs[0], Completion::Failed(Failure::ActionFailed))
        .unwrap();
    let a = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Hardware,
            &[],
        )
        .unwrap();
    assert_eq!(a.id.counter(), 33);
    assert_eq!(e.usage().jobs, 32);
    for id in jobs.into_iter().skip(1) {
        e.complete(id, Completion::Succeeded).unwrap();
    }
    assert_eq!(e.usage().jobs, 1);
    e.complete(a.dispatches[0].id, Completion::Succeeded)
        .unwrap();
    assert_eq!(e.usage().jobs, 0);
}

#[test]
fn pending_snapshot_survives_parameter_edit_revocation_and_owner_deletion() {
    let c = actions(2);
    let b = button(&c);
    let a = c.configuration().data().buttons[0].actions[1]
        .binding
        .identity();
    let key = ExternalType::new("plugin".into(), "action".into()).unwrap();
    let c = edit(
        &c,
        Edit::ReplaceActionType {
            action: a,
            kind: ActionKind::External {
                type_key: key.clone(),
                parameters: vec![7, 8],
            },
        },
    );
    let c = edit(
        &c,
        Edit::SetButtonPermissions {
            button: b,
            permissions: allow(),
        },
    );
    let mut e = Engine::new(1, 1).unwrap();
    let run = e
        .admit(
            c.configuration(),
            Target::Button(b),
            Trigger::Automation,
            std::slice::from_ref(&key),
        )
        .unwrap();
    let changed = edit(
        &c,
        Edit::ReplaceActionType {
            action: a,
            kind: launcher(),
        },
    );
    let revoked = edit(
        &changed,
        Edit::SetButtonPermissions {
            button: b,
            permissions: LocalPermissions::own(Permissions::default()),
        },
    );
    let deleted = edit(&revoked, Edit::DeleteButton { button: b });
    let next = e
        .complete(run.dispatches[0].id, Completion::Succeeded)
        .unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(
        next[0].operation,
        Operation::External {
            type_key: key,
            parameters: vec![7, 8]
        }
    );
    e.complete(next[0].id, Completion::Succeeded).unwrap();
    assert_eq!(e.result(run.id).unwrap().status, ExecutionStatus::Succeeded);
    assert_eq!(
        e.admit(
            deleted.configuration(),
            Target::Button(b),
            Trigger::Automation,
            &[]
        )
        .unwrap_err(),
        ExecutionError::NotFound
    );
    assert_eq!(
        e.admit(
            revoked.configuration(),
            Target::Button(b),
            Trigger::Automation,
            &[]
        )
        .unwrap_err(),
        ExecutionError::PermissionDenied
    );
}

#[test]
fn navigation_intents_capture_destination_but_runtime_must_validate_it_when_applying() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap();
    let root = c.configuration().data().principal;
    let c = edit(
        &c,
        Edit::CreateFolder {
            parent: root,
            position: 3,
            name: "child".into(),
        },
    );
    let child = c.configuration().data().folders[1].binding.identity();
    let entry = c
        .configuration()
        .data()
        .buttons
        .iter()
        .find(|b| b.actions[0].kind == ActionKind::EnterFolder(child))
        .unwrap();
    let back = c
        .configuration()
        .data()
        .buttons
        .iter()
        .find(|b| b.actions[0].kind == ActionKind::Return)
        .unwrap();
    let mut e = Engine::new(1, 1).unwrap();
    let enter = e
        .admit(
            c.configuration(),
            Target::Button(entry.binding.identity()),
            Trigger::Hardware,
            &[],
        )
        .unwrap();
    let leave = e
        .admit(
            c.configuration(),
            Target::Button(back.binding.identity()),
            Trigger::Hardware,
            &[],
        )
        .unwrap();
    assert_eq!(enter.dispatches[0].operation, Operation::Navigate(child));
    assert_eq!(leave.dispatches[0].operation, Operation::Navigate(root));
    let deleted = edit(
        &c,
        Edit::DeleteFolder {
            folder: child,
            confirm_contents: true,
        },
    );
    assert!(
        !deleted
            .configuration()
            .data()
            .folders
            .iter()
            .any(|f| f.binding.identity() == child)
    );
    // No automatic navigation is applied by the engine. Adapter reports stale target failure.
    e.complete(
        enter.dispatches[0].id,
        Completion::Failed(Failure::ActionFailed),
    )
    .unwrap();
    e.complete(leave.dispatches[0].id, Completion::Succeeded)
        .unwrap();
    assert_eq!(e.result(enter.id).unwrap().status, ExecutionStatus::Failed);
    assert_eq!(
        e.result(leave.id).unwrap().status,
        ExecutionStatus::Succeeded
    );
}

#[test]
fn individual_last_child_ignores_sibling_executor_permissions_and_parent_plan_waits() {
    let c = spaced(2, Mode::Parallel, Some(600_000));
    let b = button(&c);
    let a0 = c.configuration().data().buttons[0].actions[0]
        .binding
        .identity();
    let a1 = c.configuration().data().buttons[0].actions[2]
        .binding
        .identity();
    let c = edit(
        &c,
        Edit::ReplaceActionType {
            action: a0,
            kind: ActionKind::External {
                type_key: ExternalType::new("missing".into(), "action".into()).unwrap(),
                parameters: vec![],
            },
        },
    );
    let c = edit(
        &c,
        Edit::SetButtonPermissions {
            button: b,
            permissions: LocalPermissions::own(Permissions::new(false, false, false).unwrap()),
        },
    );
    let c = edit(
        &c,
        Edit::SetActionPermissions {
            action: a1,
            permissions: allow(),
        },
    );
    let mut e = Engine::new(1, 1).unwrap();
    let a = e
        .admit(
            c.configuration(),
            Target::Action(a1),
            Trigger::Automation,
            &[],
        )
        .unwrap();
    assert_eq!(a.dispatches.len(), 1);
    assert_eq!(a.dispatches[0].id.index(), 2);
    assert_eq!(e.usage().jobs, 1);
    e.complete(a.dispatches[0].id, Completion::Succeeded)
        .unwrap();
    assert_eq!(e.result(a.id).unwrap().steps.len(), 1);
    assert_eq!(e.result(a.id).unwrap().status, ExecutionStatus::Succeeded);
}

#[test]
fn timeout_precedes_equal_due_dispatch_and_first_failure_wins_while_collecting() {
    let c = spaced(2, Mode::Parallel, Some(5000));
    let mut e = Engine::new(1, 1).unwrap();
    let a = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    assert!(e.advance(5000).unwrap().is_empty());
    assert_eq!(
        e.result(a.id).unwrap().steps[2].status,
        StepStatus::NotDispatched
    );
    let c = spaced(2, Mode::Parallel, Some(100));
    let a = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    let second = e.advance(5100).unwrap();
    e.complete(
        a.dispatches[0].id,
        Completion::Failed(Failure::ActionFailed),
    )
    .unwrap();
    e.advance(10100).unwrap();
    assert_eq!(e.result(a.id).unwrap().status, ExecutionStatus::Failed);
    e.complete(second[0].id, Completion::Succeeded).unwrap();
    let a = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    let second = e.advance(10200).unwrap();
    e.advance(15100).unwrap();
    assert_eq!(e.result(a.id).unwrap().status, ExecutionStatus::Running);
    e.complete(second[0].id, Completion::Failed(Failure::ActionFailed))
        .unwrap();
    assert_eq!(e.result(a.id).unwrap().status, ExecutionStatus::TimedOut);
}

#[test]
fn clock_jumps_never_backdate_dispatch_and_foreign_session_cannot_complete_job() {
    let c = spaced(2, Mode::Sequential, Some(100));
    let mut e = Engine::new(1, 1).unwrap();
    let a = e
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    e.complete(a.dispatches[0].id, Completion::Succeeded)
        .unwrap();
    let next = e.advance(100_000).unwrap();
    assert_eq!(next[0].deadline_ms, 105_000);
    let mut other = Engine::new(2, 1).unwrap();
    let foreign = other
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    assert_eq!(
        e.complete(foreign.dispatches[0].id, Completion::Succeeded)
            .unwrap_err(),
        ExecutionError::UnknownDispatch
    );
    assert_eq!(e.usage().jobs, 1);
    e.advance(105_000).unwrap();
    assert_eq!(e.result(a.id).unwrap().status, ExecutionStatus::TimedOut);
}

#[test]
fn empty_actions_still_require_permission_and_admission_slot_and_ttl_does_not_wrap() {
    let c = empty_button();
    let mut e = Engine::new(1, 1).unwrap();
    assert_eq!(
        e.admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Automation,
            &[]
        )
        .unwrap_err(),
        ExecutionError::PermissionDenied
    );
    let c = deck(17, 1);
    let last = c.configuration().data().buttons[16].clone();
    let c = edit(
        &c,
        Edit::RemoveAction {
            action: last.actions[0].binding.identity(),
        },
    );
    for b in &c.configuration().data().buttons[..16] {
        e.admit(
            c.configuration(),
            Target::Button(b.binding.identity()),
            Trigger::Hardware,
            &[],
        )
        .unwrap();
    }
    assert_eq!(
        e.admit(
            c.configuration(),
            Target::Button(last.binding.identity()),
            Trigger::Editor,
            &[]
        )
        .unwrap_err(),
        ExecutionError::Capacity
    );
    let c = empty_button();
    let mut end = Engine::new(2, 1).unwrap();
    end.advance(u64::MAX).unwrap();
    let a = end
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap();
    assert_eq!(end.next_deadline(), None);
    assert_eq!(end.result(a.id).unwrap().status, ExecutionStatus::Succeeded);
    end.advance(u64::MAX).unwrap();
    assert!(end.result(a.id).is_some());
}

#[test]
fn sequential_middle_failure_keeps_prior_success_and_new_admission_uses_new_parameters() {
    let c = actions(3);
    let b = button(&c);
    let a = c.configuration().data().buttons[0].actions[1]
        .binding
        .identity();
    let key = ExternalType::new("plugin".into(), "action".into()).unwrap();
    let c = edit(
        &c,
        Edit::ReplaceActionType {
            action: a,
            kind: ActionKind::External {
                type_key: key.clone(),
                parameters: vec![1],
            },
        },
    );
    let mut e = Engine::new(1, 1).unwrap();
    let first = e
        .admit(
            c.configuration(),
            Target::Button(b),
            Trigger::Editor,
            std::slice::from_ref(&key),
        )
        .unwrap();
    let changed = edit(
        &c,
        Edit::SetParameters {
            action: a,
            parameters: ActionParameters::External(vec![2]),
        },
    );
    let middle = e
        .complete(first.dispatches[0].id, Completion::Succeeded)
        .unwrap();
    assert_eq!(
        middle[0].operation,
        Operation::External {
            type_key: key.clone(),
            parameters: vec![1]
        }
    );
    assert!(
        e.complete(middle[0].id, Completion::Failed(Failure::ActionFailed))
            .unwrap()
            .is_empty()
    );
    let result = e.result(first.id).unwrap();
    assert_eq!(
        result.steps.iter().map(|s| s.status).collect::<Vec<_>>(),
        vec![
            StepStatus::Succeeded,
            StepStatus::Failed(Failure::ActionFailed),
            StepStatus::NotDispatched
        ]
    );
    let new = e
        .admit(
            changed.configuration(),
            Target::Action(a),
            Trigger::Editor,
            std::slice::from_ref(&key),
        )
        .unwrap();
    assert_eq!(
        new.dispatches[0].operation,
        Operation::External {
            type_key: key,
            parameters: vec![2]
        }
    );
}

#[test]
fn cleanup_pin_survives_result_capacity_and_gets_ttl_after_release() {
    let c = empty_button();
    let mut engine = Engine::new(7, 1).unwrap();
    let held = engine
        .admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap()
        .id;
    engine.pin_result(held).unwrap();
    for _ in 0..MAX_RESULTS + 1 {
        engine
            .admit(
                c.configuration(),
                Target::Button(button(&c)),
                Trigger::Editor,
                &[],
            )
            .unwrap();
    }
    engine.advance(RESULT_TTL_MS + 1).unwrap();
    assert_eq!(
        engine.result(held).unwrap().status,
        ExecutionStatus::Running
    );
    engine.report_cleanup_failure(held);
    engine.release_result(held);
    assert_eq!(engine.result(held).unwrap().status, ExecutionStatus::Failed);
    engine.advance(2 * RESULT_TTL_MS).unwrap();
    assert!(engine.result(held).is_some());
    engine.advance(2 * RESULT_TTL_MS + 1).unwrap();
    assert!(engine.result(held).is_none());
}
