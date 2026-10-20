use rad_core::{configuration::*, execution::*, identity::*};
/// Two launchers with a wait of `wait_ms` between them: actions 0, 1 (the wait) and 2.
fn fixture(mode: Mode, wait_ms: u64) -> Candidate {
    let c = Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "review".into(),
            },
        )
        .unwrap();
    let id = c.configuration().data().buttons[0].binding.identity();
    let c = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                Edit::AddAction {
                    button: id,
                    kind: ActionKind::Launcher(DesktopEntry::new("a.desktop".into()).unwrap()),
                },
                Edit::AddAction {
                    button: id,
                    kind: ActionKind::Wait(Wait::new(wait_ms).unwrap()),
                },
                Edit::AddAction {
                    button: id,
                    kind: ActionKind::Launcher(DesktopEntry::new("b.desktop".into()).unwrap()),
                },
            ],
        )
        .unwrap();
    let order = c.configuration().data().buttons[0]
        .actions
        .iter()
        .map(|a| a.binding.identity())
        .collect();
    c.configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetPlan {
                button: id,
                mode,
                order,
            },
        )
        .unwrap()
}
fn admit(e: &mut Engine, c: &Candidate) -> Admission {
    e.admit(
        c.configuration(),
        Target::Button(c.configuration().data().buttons[0].binding.identity()),
        Trigger::Hardware,
        &[],
    )
    .unwrap()
}
#[test]
fn success_one_ms_before_deadline_near_max_can_finish_remaining_plan() {
    let c = fixture(Mode::Sequential, 100);
    let mut e = Engine::new(1, 1).unwrap();
    let start = u64::MAX - 10100;
    e.advance(start).unwrap();
    let a = admit(&mut e, &c);
    e.advance(start + 4999).unwrap();
    assert!(
        e.complete(a.dispatches[0].id, Completion::Succeeded)
            .unwrap()
            .is_empty()
    );
    assert_eq!(e.next_deadline(), Some(start + 5099));
    let next = e.advance(start + 5099).unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].deadline_ms, u64::MAX - 1);
    e.advance(u64::MAX - 2).unwrap();
    e.complete(next[0].id, Completion::Succeeded).unwrap();
    assert_eq!(e.result(a.id).unwrap().status, ExecutionStatus::Succeeded);
    assert_eq!(e.usage().jobs, 0);
    assert_eq!(e.next_deadline(), None); // TTL cannot be represented, bounded by count
}
#[test]
fn last_dispatch_error_after_first_success_never_becomes_success() {
    let c = fixture(Mode::Parallel, 100);
    let mut e = Engine::new(1, 1).unwrap();
    let a = admit(&mut e, &c);
    e.advance(99).unwrap();
    e.complete(a.dispatches[0].id, Completion::Succeeded)
        .unwrap();
    assert_eq!(e.next_deadline(), Some(100));
    let last = e.advance(100).unwrap();
    assert_eq!(last.len(), 1);
    e.advance(5099).unwrap();
    e.complete(last[0].id, Completion::Failed(Failure::ActionFailed))
        .unwrap();
    let result = e.result(a.id).unwrap();
    assert_eq!(result.status, ExecutionStatus::Failed);
    assert_eq!(result.steps[0].status, StepStatus::Succeeded);
    assert_eq!(result.steps[1].status, StepStatus::Succeeded);
    assert_eq!(
        result.steps[2].status,
        StepStatus::Failed(Failure::ActionFailed)
    );
    assert_eq!(e.usage().jobs, 0);
    assert_eq!(
        e.complete(last[0].id, Completion::Succeeded),
        Err(ExecutionError::UnknownDispatch)
    );
    assert_eq!(e.result(a.id).unwrap(), result);
}
#[test]
fn overflow_rolls_back_other_timeout_and_clock_in_the_same_transition() {
    let c = fixture(Mode::Sequential, 100);
    let mut data = c.configuration().data().clone();
    let mut registry = c.registry().clone();
    let mut second = data.buttons[0].clone();
    second.binding = registry.allocate(EntityKind::Button).unwrap();
    for action in &mut second.actions {
        action.binding = registry.allocate(EntityKind::Action).unwrap();
    }
    let second_id = second.binding.identity();
    data.folders[0].slots[1] = Some(second_id);
    data.buttons.push(second);
    let config = Configuration::from_data(data, &registry).unwrap();
    let mut e = Engine::new(1, 1).unwrap();
    let a = e
        .admit(&config, Target::Button(second_id), Trigger::Editor, &[])
        .unwrap();
    let b = admit(&mut e, &c);
    e.complete(b.dispatches[0].id, Completion::Succeeded)
        .unwrap();
    let usage = e.usage();
    let a_before = e.result(a.id);
    let b_before = e.result(b.id);
    assert_eq!(e.advance(u64::MAX), Err(ExecutionError::TimeOverflow));
    assert_eq!(e.usage(), usage);
    assert_eq!(e.result(a.id), a_before);
    assert_eq!(e.result(b.id), b_before);
    assert_eq!(e.next_deadline(), Some(100));
    // Error did not publish MAX as current time or consume A's actual completion.
    e.advance(99).unwrap();
    e.complete(a.dispatches[0].id, Completion::Succeeded)
        .unwrap();
    assert_eq!(e.advance(100).unwrap().len(), 1);
}
