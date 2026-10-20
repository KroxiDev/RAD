//! A wait is an action the engine times itself: nothing is dispatched for it, it holds
//! the plan back in both modes, and it is a deliberate pause rather than a response budget.
use rad_core::configuration::*;
use rad_core::execution::*;
use rad_core::identity::*;

fn launcher() -> ActionKind {
    ActionKind::Launcher(DesktopEntry::new("app.desktop".into()).unwrap())
}
fn wait(milliseconds: u64) -> ActionKind {
    ActionKind::Wait(Wait::new(milliseconds).unwrap())
}
fn edit(c: &Candidate, e: Edit) -> Candidate {
    c.configuration()
        .prepare_edit(c.registry(), Origin::Editor, e)
        .unwrap()
}
/// One button holding `kinds` in order, in `mode`.
fn button_with(kinds: &[ActionKind], mode: Mode) -> (Candidate, EntityIdentity) {
    let c = Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap();
    let mut c = edit(
        &c,
        Edit::CreateButton {
            folder: c.configuration().data().principal,
            position: 0,
            name: "A".into(),
        },
    );
    let button = c.configuration().data().buttons[0].binding.identity();
    for kind in kinds {
        c = edit(
            &c,
            Edit::AddAction {
                button,
                kind: kind.clone(),
            },
        );
    }
    let data = &c.configuration().data().buttons[0];
    let order = data.actions.iter().map(|a| a.binding.identity()).collect();
    let c = edit(
        &c,
        Edit::SetPlan {
            button,
            mode,
            order,
        },
    );
    (c, button)
}
fn admit(engine: &mut Engine, c: &Candidate, button: EntityIdentity) -> Admission {
    engine
        .admit(
            c.configuration(),
            Target::Button(button),
            Trigger::Hardware,
            &[],
        )
        .unwrap()
}
fn statuses(engine: &Engine, id: ExecutionId) -> Vec<StepStatus> {
    let result = engine.result(id).unwrap();
    result.steps.iter().map(|s| s.status).collect()
}

#[test]
fn wait_duration_is_bounded_to_ten_minutes() {
    assert_eq!(Wait::new(0).unwrap().milliseconds(), 0);
    assert_eq!(Wait::new(600_000).unwrap().milliseconds(), 600_000);
    assert!(Wait::new(600_001).is_err());
}

#[test]
fn sequential_wait_holds_the_next_action_without_dispatching_anything_itself() {
    let (c, button) = button_with(&[launcher(), wait(500), launcher()], Mode::Sequential);
    let mut engine = Engine::new(1, 1).unwrap();
    let admission = admit(&mut engine, &c, button);
    assert_eq!(admission.dispatches.len(), 1);
    assert_eq!(admission.dispatches[0].id.index(), 0);
    engine.advance(100).unwrap();
    // The wait starts when the action before it has finished, not at admission.
    let resumed = engine
        .complete(admission.dispatches[0].id, Completion::Succeeded)
        .unwrap();
    assert!(resumed.is_empty());
    assert_eq!(
        statuses(&engine, admission.id),
        [
            StepStatus::Succeeded,
            StepStatus::Dispatched,
            StepStatus::Waiting
        ]
    );
    assert_eq!(engine.next_deadline(), Some(600));
    assert!(engine.advance(599).unwrap().is_empty());
    let after = engine.advance(600).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id.index(), 2);
    assert_eq!(after[0].deadline_ms, 600 + RESPONSE_TIMEOUT_MS);
    assert_eq!(statuses(&engine, admission.id)[1], StepStatus::Succeeded);
    engine.complete(after[0].id, Completion::Succeeded).unwrap();
    assert_eq!(
        engine.result(admission.id).unwrap().status,
        ExecutionStatus::Succeeded
    );
}

#[test]
fn wait_longer_than_the_response_budget_is_a_pause_not_a_timeout() {
    let (c, button) = button_with(&[wait(6_000), launcher()], Mode::Sequential);
    let mut engine = Engine::new(1, 1).unwrap();
    let admission = admit(&mut engine, &c, button);
    assert!(admission.dispatches.is_empty());
    assert!(engine.advance(RESPONSE_TIMEOUT_MS).unwrap().is_empty());
    assert_eq!(
        engine.result(admission.id).unwrap().status,
        ExecutionStatus::Running
    );
    let after = engine.advance(6_000).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id.index(), 1);
    assert_eq!(engine.usage().residual_jobs, 0);
}

#[test]
fn parallel_wait_is_a_barrier_and_a_failure_ends_it_with_the_plan() {
    let (c, button) = button_with(
        &[launcher(), launcher(), wait(500), launcher()],
        Mode::Parallel,
    );
    let mut engine = Engine::new(1, 1).unwrap();
    let admission = admit(&mut engine, &c, button);
    // Both actions before the wait start together; the one after it is held back.
    let started: Vec<_> = admission.dispatches.iter().map(|d| d.id.index()).collect();
    assert_eq!(started, [0, 1]);
    assert!(engine.advance(499).unwrap().is_empty());
    let after = engine.advance(500).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id.index(), 3);

    let mut engine = Engine::new(2, 1).unwrap();
    let admission = admit(&mut engine, &c, button);
    engine.advance(100).unwrap();
    engine
        .complete(
            admission.dispatches[0].id,
            Completion::Failed(Failure::ActionFailed),
        )
        .unwrap();
    assert_eq!(
        statuses(&engine, admission.id),
        [
            StepStatus::Failed(Failure::ActionFailed),
            StepStatus::Dispatched,
            StepStatus::NotDispatched,
            StepStatus::NotDispatched
        ]
    );
    engine
        .complete(admission.dispatches[1].id, Completion::Succeeded)
        .unwrap();
    // The interrupted wait does not keep the button busy until it would have ended.
    assert_eq!(
        engine.result(admission.id).unwrap().status,
        ExecutionStatus::Failed
    );
    assert_eq!(engine.usage().active_executions, 0);
    assert!(engine.advance(600).unwrap().is_empty());
}

#[test]
fn button_of_only_waits_stays_busy_for_their_duration_and_then_succeeds() {
    let (c, button) = button_with(&[wait(0), wait(200)], Mode::Sequential);
    let mut engine = Engine::new(1, 1).unwrap();
    let admission = admit(&mut engine, &c, button);
    assert!(admission.dispatches.is_empty());
    assert_eq!(
        statuses(&engine, admission.id),
        [StepStatus::Succeeded, StepStatus::Dispatched]
    );
    assert_eq!(
        engine
            .admit(
                c.configuration(),
                Target::Button(button),
                Trigger::Editor,
                &[]
            )
            .unwrap_err(),
        ExecutionError::Busy
    );
    assert!(engine.advance(200).unwrap().is_empty());
    assert_eq!(
        engine.result(admission.id).unwrap().status,
        ExecutionStatus::Succeeded
    );
    assert_eq!(engine.usage().active_executions, 0);
}

#[test]
fn wait_in_progress_cannot_be_completed_from_outside() {
    let (c, button) = button_with(&[wait(300), launcher()], Mode::Sequential);
    let mut engine = Engine::new(1, 1).unwrap();
    let admission = admit(&mut engine, &c, button);
    let (c2, other) = button_with(&[launcher()], Mode::Sequential);
    let mut scratch = Engine::new(1, 1).unwrap();
    // Same session, counter and index as the wait: the only way to name its dispatch.
    let forged = admit(&mut scratch, &c2, other).dispatches[0].id;
    assert_eq!(forged.execution(), admission.id);
    assert_eq!(
        engine.complete(forged, Completion::Succeeded).unwrap_err(),
        ExecutionError::UnknownDispatch
    );
    assert_eq!(
        statuses(&engine, admission.id),
        [StepStatus::Dispatched, StepStatus::Waiting]
    );
}

#[test]
fn wait_duration_is_an_editable_parameter_and_other_parameters_do_not_fit_it() {
    let (c, _) = button_with(&[wait(100), launcher()], Mode::Sequential);
    let actions = &c.configuration().data().buttons[0].actions;
    let (waiting, launching) = (actions[0].binding.identity(), actions[1].binding.identity());
    let next = edit(
        &c,
        Edit::SetParameters {
            action: waiting,
            parameters: ActionParameters::Wait(Wait::new(2_500).unwrap()),
        },
    );
    assert_eq!(
        next.configuration().data().buttons[0].actions[0].kind,
        wait(2_500)
    );
    for (action, parameters) in [
        (launching, ActionParameters::Wait(Wait::new(1).unwrap())),
        (
            waiting,
            ActionParameters::Launcher(DesktopEntry::new("app.desktop".into()).unwrap()),
        ),
    ] {
        assert_eq!(
            c.configuration()
                .prepare_edit(
                    c.registry(),
                    Origin::Editor,
                    Edit::SetParameters { action, parameters }
                )
                .unwrap_err(),
            ConfigurationError::InvalidActions
        );
    }
}
