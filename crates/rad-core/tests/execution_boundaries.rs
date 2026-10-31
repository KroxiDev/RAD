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
