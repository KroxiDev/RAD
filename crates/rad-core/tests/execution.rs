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
fn adapter_reported_timeout_ends_the_execution_once_in_either_order_and_frees_credit() {
    let c = actions(2);
    let started = |e: &mut Engine| {
        e.admit(
            c.configuration(),
            Target::Button(button(&c)),
            Trigger::Editor,
            &[],
        )
        .unwrap()
    };
    // The adapter reports first: Engine has not processed its own deadline yet.
    let mut e = Engine::new(1, 1).unwrap();
    let run = started(&mut e);
    assert!(
        e.complete(run.dispatches[0].id, Completion::TimedOut)
            .unwrap()
            .is_empty()
    );
    let result = e.result(run.id).unwrap();
    assert_eq!(result.status, ExecutionStatus::TimedOut);
    assert_eq!(result.steps[0].status, StepStatus::TimedOut);
    assert_eq!(result.steps[1].status, StepStatus::NotDispatched);
    assert_eq!(
        e.usage(),
        Usage {
            active_executions: 0,
            jobs: 0,
            residual_jobs: 0
        }
    );
    let before = e.result(run.id).unwrap().clone();
    assert!(e.advance(5000).unwrap().is_empty());
    assert_eq!(e.result(run.id).unwrap(), before);
    assert_eq!(e.usage().residual_jobs, 0);
    assert_eq!(
        e.complete(run.dispatches[0].id, Completion::TimedOut)
            .unwrap_err(),
        ExecutionError::UnknownDispatch
    );
    // Engine processes its deadline first: the report only frees the credit.
    let mut e = Engine::new(1, 1).unwrap();
    let run = started(&mut e);
    e.advance(5000).unwrap();
    assert_eq!(e.usage().residual_jobs, 1);
    assert!(
        e.complete(run.dispatches[0].id, Completion::TimedOut)
            .unwrap()
            .is_empty()
    );
    assert_eq!(e.usage().jobs, 0);
    assert_eq!(e.usage().residual_jobs, 0);
    let result = e.result(run.id).unwrap();
    assert_eq!(result.status, ExecutionStatus::TimedOut);
    assert_eq!(result.steps[0].status, StepStatus::TimedOut);
    assert_eq!(
        e.complete(run.dispatches[0].id, Completion::TimedOut)
            .unwrap_err(),
        ExecutionError::UnknownDispatch
    );
}
