use rad_core::{configuration::*, execution::*, identity::*};
use std::collections::BTreeMap;

fn fixture(wait: Option<u64>) -> (Candidate, EntityIdentity, EntityIdentity, ExternalType) {
    let mut c = Configuration::prepare_new(&IdentityRegistry::new(71).unwrap()).unwrap();
    let root = c.configuration().data().principal;
    c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: root,
                position: 0,
                name: "fixture".into(),
            },
        )
        .unwrap();
    let button = c.configuration().data().buttons[0].binding.identity();
    if let Some(ms) = wait {
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::AddAction {
                    button,
                    kind: ActionKind::Wait(Wait::new(ms).unwrap()),
                },
            )
            .unwrap();
    }
    let key = ExternalType::new("fixture".into(), "connect".into()).unwrap();
    c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::AddAction {
                button,
                kind: ActionKind::External {
                    type_key: key.clone(),
                    parameters: b"{}".to_vec(),
                },
            },
        )
        .unwrap();
    let action = c.configuration().data().buttons[0]
        .actions
        .last()
        .unwrap()
        .binding
        .identity();
    (c, button, action, key)
}

#[test]
fn thirty_second_external_budget_does_not_expire_at_five_seconds() {
    let (c, button, action, key) = fixture(None);
    let mut e = Engine::new(1, 1).unwrap();
    let budgets = BTreeMap::from([(action, ResponseBudget::new(30_000).unwrap())]);
    let a = e
        .admit_with_response_budgets(
            c.configuration(),
            Target::Button(button),
            Trigger::Editor,
            &[key],
            &budgets,
        )
        .unwrap();
    assert_eq!(a.dispatches[0].deadline_ms, 30_000);
    e.advance(5_000).unwrap();
    assert_eq!(e.result(a.id).unwrap().status, ExecutionStatus::Running);
    e.advance(30_000).unwrap();
    assert_eq!(e.result(a.id).unwrap().status, ExecutionStatus::TimedOut);
    assert_eq!(e.usage().residual_jobs, 1);
}

#[test]
fn budget_is_captured_and_starts_after_the_deliberate_wait() {
    let (c, button, action, key) = fixture(Some(10_000));
    let mut e = Engine::new(2, 1).unwrap();
    let mut budgets = BTreeMap::from([(action, ResponseBudget::new(30_000).unwrap())]);
    let a = e
        .admit_with_response_budgets(
            c.configuration(),
            Target::Button(button),
            Trigger::Editor,
            &[key],
            &budgets,
        )
        .unwrap();
    assert!(a.dispatches.is_empty());
    budgets.insert(action, ResponseBudget::new(1000).unwrap());
    let dispatches = e.advance(10_000).unwrap();
    assert_eq!(dispatches[0].deadline_ms, 40_000);
    e.advance(39_999).unwrap();
    assert_eq!(e.result(a.id).unwrap().status, ExecutionStatus::Running);
    e.advance(40_000).unwrap();
    assert_eq!(e.result(a.id).unwrap().status, ExecutionStatus::TimedOut);
}

#[test]
fn budgets_have_bounded_values_and_ordinary_admission_still_has_five_seconds() {
    assert!(ResponseBudget::new(999).is_err());
    assert!(ResponseBudget::new(120_001).is_err());
    assert_eq!(ResponseBudget::new(1000).unwrap().milliseconds(), 1000);
    assert_eq!(
        ResponseBudget::new(120_000).unwrap().milliseconds(),
        120_000
    );
    let (c, button, _, key) = fixture(None);
    let mut e = Engine::new(3, 1).unwrap();
    let a = e
        .admit(
            c.configuration(),
            Target::Button(button),
            Trigger::Editor,
            &[key],
        )
        .unwrap();
    assert_eq!(a.dispatches[0].deadline_ms, 5000);
    e.advance(5000).unwrap();
    assert_eq!(e.result(a.id).unwrap().status, ExecutionStatus::TimedOut);
}

#[test]
fn overflowing_response_horizon_rejects_admission_without_reserving_or_dispatching() {
    let (c, button, action, key) = fixture(Some(1000));
    let mut e = Engine::new(4, 1).unwrap();
    e.advance(u64::MAX - 30_000).unwrap();
    let budgets = BTreeMap::from([(action, ResponseBudget::new(30_000).unwrap())]);
    let error = e
        .admit_with_response_budgets(
            c.configuration(),
            Target::Button(button),
            Trigger::Editor,
            &[key],
            &budgets,
        )
        .unwrap_err();
    assert_eq!(error, ExecutionError::TimeOverflow);
    assert_eq!(e.usage().active_executions, 0);
    assert_eq!(e.usage().jobs, 0);
}

#[test]
fn external_boundaries_are_applied_but_launcher_overrides_are_ignored() {
    for milliseconds in [1000, 120_000] {
        let (c, button, action, key) = fixture(None);
        let c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::AddAction {
                    button,
                    kind: ActionKind::Launcher(
                        DesktopEntry::new("fixture.desktop".into()).unwrap(),
                    ),
                },
            )
            .unwrap();
        let actions = &c.configuration().data().buttons[0].actions;
        let launcher = actions[1].binding.identity();
        let c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::SetPlan {
                    button,
                    mode: Mode::Parallel,
                    order: actions.iter().map(|a| a.binding.identity()).collect(),
                },
            )
            .unwrap();
        let mut e = Engine::new(milliseconds as u128, 1).unwrap();
        let budgets = BTreeMap::from([
            (action, ResponseBudget::new(milliseconds).unwrap()),
            (launcher, ResponseBudget::new(120_000).unwrap()),
        ]);
        let a = e
            .admit_with_response_budgets(
                c.configuration(),
                Target::Button(button),
                Trigger::Editor,
                &[key],
                &budgets,
            )
            .unwrap();
        assert_eq!(a.dispatches[0].deadline_ms, milliseconds);
        assert_eq!(a.dispatches[1].deadline_ms, 5000);
    }
}
