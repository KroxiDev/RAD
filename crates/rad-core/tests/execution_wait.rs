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
