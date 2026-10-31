use rad_authority::{Authority, Command, Effect, Outcome};
use rad_core::{configuration::*, execution::*, identity::*};
fn setup(brightness: u8, actions: &[ActionKind], mode: Mode) -> (Authority, EntityIdentity) {
    let c = Configuration::prepare_new(&IdentityRegistry::new(290).unwrap()).unwrap();
    let root = c.configuration().data().principal;
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: root,
                position: 0,
                name: "Brightness".into(),
            },
        )
        .unwrap();
    let b = c.configuration().data().buttons[0].binding.identity();
    let mut settings = c.configuration().settings().data();
    settings.brightness_percent = brightness;
    let mut edits = vec![Edit::SetSettings(settings)];
    edits.extend(
        actions
            .iter()
            .cloned()
            .map(|kind| Edit::AddAction { button: b, kind }),
    );
    let c = c
        .configuration()
        .prepare_edits(c.registry(), Origin::Editor, &edits)
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
                button: b,
                mode,
                order,
            },
        )
        .unwrap();
    (
        Authority::new(c, 1, std::num::NonZeroU128::new(291).unwrap()),
        b,
    )
}
fn run(a: &mut Authority, b: EntityIdentity) -> Vec<Effect> {
    let t = a.reserve(1, Origin::Editor).unwrap();
    a.submit(
        t,
        Command::Run {
            target: Target::Button(b),
        },
    )
}
fn persist(e: &[Effect]) -> rad_authority::CommitId {
    e.iter()
        .find_map(|e| {
            if let Effect::Persist { id, .. } = e {
                Some(*id)
            } else {
                None
            }
        })
        .expect("Brightness must use the publication lane")
}
#[test]
fn brightness_publishes_only_after_applied_and_preserves_non_brightness_settings() {
    let (mut a, b) = setup(
        55,
        &[ActionKind::Brightness(BrightnessDirection::Increase)],
        Mode::Sequential,
    );
    let effects = run(&mut a, b);
    let id = persist(&effects);
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::Dispatch(_) | Effect::Display { .. }))
    );
    assert_eq!(a.editor_view(Origin::Editor).unwrap().brightness, 55);
    let effects = a.complete(
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert_eq!(a.editor_view(Origin::Editor).unwrap().brightness, 65);
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Display {
            brightness: 65,
            asleep: false
        }
    )));
    assert_eq!(a.settings(Origin::Editor).unwrap().sleep_ms, 300_000);
}
fn execution(e: &[Effect]) -> ExecutionId {
    e.iter()
        .find_map(|e| {
            if let Effect::Accepted { execution, .. } = e {
                Some(*execution)
            } else {
                None
            }
        })
        .unwrap()
}
#[test]
fn fixed_steps_clamp_only_at_the_limits_and_saturated_actions_do_not_persist() {
    for (value, direction, want) in [
        (55, BrightnessDirection::Decrease, 45),
        (95, BrightnessDirection::Increase, 100),
        (15, BrightnessDirection::Decrease, 10),
        (100, BrightnessDirection::Increase, 100),
        (10, BrightnessDirection::Decrease, 10),
    ] {
        let (mut a, b) = setup(
            value,
            &[ActionKind::Brightness(direction)],
            Mode::Sequential,
        );
        let effects = run(&mut a, b);
        let execution = execution(&effects);
        if want == value {
            assert!(!effects.iter().any(|e| matches!(e, Effect::Persist { .. })));
            assert_eq!(a.editor_view(Origin::Editor).unwrap().revision, 1);
        } else {
            let id = persist(&effects);
            assert_eq!(a.commit_request(id), None);
            a.complete(
                id,
                Outcome::Applied {
                    revision: 2,
                    changed: true,
                },
            );
        }
        assert_eq!(a.settings(Origin::Editor).unwrap().brightness_percent, want);
        assert_eq!(
            a.result(Origin::Editor, execution).unwrap().status,
            ExecutionStatus::Succeeded
        );
    }
}

#[test]
fn uncertain_publication_reconciles_the_original_delta_once_and_never_replays_after_timeout() {
    for expire in [false, true] {
        let (mut a, b) = setup(
            55,
            &[
                ActionKind::Brightness(BrightnessDirection::Increase),
                ActionKind::Sleep,
            ],
            Mode::Sequential,
        );
        let effects = run(&mut a, b);
        let execution = execution(&effects);
        let id = persist(&effects);
        assert_eq!(
            a.result(Origin::Editor, execution).unwrap().status,
            ExecutionStatus::Running
        );
        let effects = a.complete(id, Outcome::Uncertain);
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::Unknown { .. } | Effect::Persist { .. }))
        );
        assert_eq!(a.settings(Origin::Editor).unwrap().brightness_percent, 55);
        if expire {
            a.advance(RESPONSE_TIMEOUT_MS + 1).unwrap();
        }
        let reconciliation = a.reconcile();
        let [Effect::Reconcile { id: reconciled }] = reconciliation.as_slice() else {
            panic!("reconciliation must retain the native candidate")
        };
        let reconciled = *reconciled;
        assert!(
            a.complete(
                id,
                Outcome::Applied {
                    revision: 2,
                    changed: true
                }
            )
            .is_empty()
        );
        let effects = a.complete(
            reconciled,
            Outcome::Applied {
                revision: 2,
                changed: true,
            },
        );
        assert_eq!(a.settings(Origin::Editor).unwrap().brightness_percent, 65);
        assert!(!effects.iter().any(|e| matches!(e, Effect::Persist { .. })));
        assert_eq!(
            a.result(Origin::Editor, execution).unwrap().status,
            if expire {
                ExecutionStatus::TimedOut
            } else {
                ExecutionStatus::Succeeded
            }
        );
        assert_eq!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Display { asleep: true, .. })),
            !expire
        );
        assert!(
            a.complete(
                id,
                Outcome::Applied {
                    revision: 2,
                    changed: true
                }
            )
            .is_empty()
        );
    }
}
