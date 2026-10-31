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
