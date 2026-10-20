use rad_core::identity::{Binding, EntityIdentity, EntityKind, ShortId};
use rad_core::input::{Change, Input, KEY_COUNT, Output};
fn binding(kind: EntityKind, number: u64) -> Binding {
    Binding::new(
        ShortId::new(kind, number).unwrap(),
        EntityIdentity::new(7, number + if kind == EntityKind::Folder { 100 } else { 0 }).unwrap(),
    )
}
fn button(number: u64) -> Binding {
    binding(EntityKind::Button, number)
}
fn layout(folder: u64, keys: &[(usize, Binding)]) -> Change {
    let mut buttons = [None; KEY_COUNT];
    for &(key, button) in keys {
        buttons[key] = Some(button);
    }
    Change::Layout {
        folder: binding(EntityKind::Folder, folder),
        buttons,
    }
}
fn report(keys: &[usize]) -> Change {
    let mut pressed = [false; KEY_COUNT];
    for &key in keys {
        pressed[key] = true;
    }
    Change::Report(pressed)
}
fn candidate(key: usize, number: u64) -> Output {
    Output::Candidate {
        key,
        button: button(number),
    }
}

#[test]
fn zero_threshold_first_single_press_is_one_shot_and_release_rearms() {
    let mut input = Input::default();
    input.handle(0, layout(1, &[(0, button(1))])).unwrap();
    assert_eq!(
        input.handle(1, report(&[0])).unwrap(),
        vec![candidate(0, 1)]
    );
    assert!(input.handle(2, report(&[0])).unwrap().is_empty());
    assert!(input.handle(3, Change::Tick).unwrap().is_empty());
    assert!(input.handle(4, report(&[])).unwrap().is_empty());
    assert_eq!(
        input.handle(5, report(&[0])).unwrap(),
        vec![candidate(0, 1)]
    );
}

#[test]
fn threshold_fires_at_exact_deadline_and_not_before_or_again() {
    let mut input = Input::default();
    input.handle(0, layout(1, &[(31, button(1))])).unwrap();
    input.handle(0, Change::Threshold(300)).unwrap();
    assert!(input.handle(10, report(&[31])).unwrap().is_empty());
    assert!(input.handle(309, Change::Tick).unwrap().is_empty());
    assert_eq!(
        input.handle(310, Change::Tick).unwrap(),
        vec![candidate(31, 1)]
    );
    assert!(input.handle(311, report(&[31])).unwrap().is_empty());
    assert!(input.handle(900, Change::Tick).unwrap().is_empty());
}

#[test]
fn releasing_early_or_at_deadline_cancels_pending_and_new_press_starts_over() {
    for release_at in [299, 300] {
        let mut input = Input::default();
        input.handle(0, layout(1, &[(0, button(1))])).unwrap();
        input.handle(0, Change::Threshold(300)).unwrap();
        input.handle(0, report(&[0])).unwrap();
        assert!(input.handle(release_at, report(&[])).unwrap().is_empty());
        assert!(input.handle(400, Change::Tick).unwrap().is_empty());
        assert!(input.handle(500, report(&[0])).unwrap().is_empty());
        assert_eq!(
            input.handle(800, Change::Tick).unwrap(),
            vec![candidate(0, 1)]
        );
    }
}

#[test]
fn assignment_change_and_back_between_reports_requires_release_but_other_key_survives() {
    let mut input = Input::default();
    input
        .handle(0, layout(1, &[(0, button(1)), (1, button(2))]))
        .unwrap();
    input.handle(0, Change::Threshold(100)).unwrap();
    input.handle(0, report(&[])).unwrap();
    input.handle(1, report(&[0, 1])).unwrap();
    input
        .handle(20, layout(1, &[(0, button(3)), (1, button(2))]))
        .unwrap();
    input
        .handle(30, layout(1, &[(0, button(1)), (1, button(2))]))
        .unwrap();
    // A parameters-only edit republishes the same assignment; key 1 remains pending.
    input
        .handle(40, layout(1, &[(0, button(1)), (1, button(2))]))
        .unwrap();
    assert_eq!(
        input.handle(101, Change::Tick).unwrap(),
        vec![candidate(1, 2)]
    );
    assert!(input.handle(102, report(&[0, 1])).unwrap().is_empty());
    input.handle(103, report(&[1])).unwrap();
    input.handle(104, report(&[0, 1])).unwrap();
    assert_eq!(
        input.handle(204, Change::Tick).unwrap(),
        vec![candidate(0, 1)]
    );
}

#[test]
fn folder_change_and_back_invalidates_even_an_identical_key_assignment() {
    let mut input = Input::default();
    input.handle(0, layout(1, &[(0, button(1))])).unwrap();
    input.handle(0, Change::Threshold(100)).unwrap();
    input.handle(1, report(&[0])).unwrap();
    input.handle(20, layout(2, &[(0, button(1))])).unwrap();
    input.handle(30, layout(1, &[(0, button(1))])).unwrap();
    assert!(input.handle(101, Change::Tick).unwrap().is_empty());
    assert!(input.handle(102, report(&[0])).unwrap().is_empty());
    input.handle(103, report(&[])).unwrap();
    input.handle(104, report(&[0])).unwrap();
    assert_eq!(
        input.handle(204, Change::Tick).unwrap(),
        vec![candidate(0, 1)]
    );
}

#[test]
fn first_multi_report_only_arms_each_key_until_its_own_release() {
    let mut input = Input::default();
    let keys: Vec<_> = (0..KEY_COUNT)
        .map(|key| (key, button(key as u64 + 1)))
        .collect();
    input.handle(0, layout(1, &keys)).unwrap();
    assert!(
        input
            .handle(1, report(&(0..KEY_COUNT).collect::<Vec<_>>()))
            .unwrap()
            .is_empty()
    );
    assert!(input.handle(2, Change::Tick).unwrap().is_empty());
    input
        .handle(3, report(&(1..KEY_COUNT).collect::<Vec<_>>()))
        .unwrap();
    assert_eq!(
        input
            .handle(4, report(&(0..KEY_COUNT).collect::<Vec<_>>()))
            .unwrap(),
        vec![candidate(0, 1)]
    );
    input.handle(5, report(&[])).unwrap();
    let all = input
        .handle(6, report(&(0..KEY_COUNT).collect::<Vec<_>>()))
        .unwrap();
    assert_eq!(
        all,
        (0..KEY_COUNT)
            .map(|key| candidate(key, key as u64 + 1))
            .collect::<Vec<_>>()
    );
    assert!(input.handle(7, Change::Tick).unwrap().is_empty());
}

#[test]
fn disconnect_discards_pending_and_reconnection_reapplies_single_and_multi_d9() {
    let mut input = Input::default();
    input
        .handle(0, layout(1, &[(0, button(1)), (1, button(2))]))
        .unwrap();
    input.handle(0, Change::Threshold(100)).unwrap();
    input.handle(1, report(&[0])).unwrap();
    input.handle(50, Change::Disconnect).unwrap();
    assert!(input.handle(101, Change::Tick).unwrap().is_empty());
    assert!(input.handle(200, report(&[0])).unwrap().is_empty());
    assert!(input.handle(299, Change::Tick).unwrap().is_empty());
    assert_eq!(
        input.handle(300, Change::Tick).unwrap(),
        vec![candidate(0, 1)]
    );
    input.handle(301, Change::Disconnect).unwrap();
    assert!(input.handle(302, report(&[0, 1])).unwrap().is_empty());
    assert!(input.handle(500, Change::Tick).unwrap().is_empty());
    input.handle(501, report(&[1])).unwrap();
    input.handle(502, report(&[0, 1])).unwrap();
    assert_eq!(
        input.handle(602, Change::Tick).unwrap(),
        vec![candidate(0, 1)]
    );
}

#[test]
fn sleep_consumes_first_single_or_multi_report_until_release_including_empty_keys() {
    for first_keys in [vec![0], vec![0, 1], vec![31]] {
        let mut input = Input::default();
        input
            .handle(0, layout(1, &[(0, button(1)), (1, button(2))]))
            .unwrap();
        input.handle(0, Change::Sleep).unwrap();
        assert!(input.handle(1, Change::Tick).unwrap().is_empty());
        assert_eq!(
            input.handle(2, report(&first_keys)).unwrap(),
            vec![Output::Wake]
        );
        assert!(input.handle(3, report(&first_keys)).unwrap().is_empty());
        assert!(input.handle(4, Change::Tick).unwrap().is_empty());
        input.handle(5, report(&[])).unwrap();
        assert_eq!(
            input.handle(6, report(&[0])).unwrap(),
            vec![candidate(0, 1)]
        );
    }
}

#[test]
fn entering_sleep_cancels_pending_and_a_held_key_does_not_wake() {
    let mut input = Input::default();
    input.handle(0, layout(1, &[(0, button(1))])).unwrap();
    input.handle(0, Change::Threshold(100)).unwrap();
    input.handle(1, report(&[0])).unwrap();
    input.handle(20, Change::Sleep).unwrap();
    assert!(input.handle(101, Change::Tick).unwrap().is_empty());
    assert!(input.handle(102, report(&[0])).unwrap().is_empty());
    input.handle(103, report(&[])).unwrap();
    assert_eq!(input.handle(104, report(&[0])).unwrap(), vec![Output::Wake]);
    assert!(input.handle(204, Change::Tick).unwrap().is_empty());
    input.handle(205, Change::Sleep).unwrap();
    input.handle(206, Change::Disconnect).unwrap();
    assert_eq!(input.handle(207, report(&[0])).unwrap(), vec![Output::Wake]);
}

#[test]
fn threshold_is_captured_at_press_and_next_deadline_schedules_without_polling() {
    let mut input = Input::default();
    input.handle(0, layout(1, &[(0, button(1))])).unwrap();
    input.handle(0, Change::Threshold(100)).unwrap();
    assert_eq!(input.next_deadline_ms(), None);
    input.handle(10, report(&[0])).unwrap();
    assert_eq!(input.next_deadline_ms(), Some(110));
    assert!(input.handle(20, Change::Threshold(0)).unwrap().is_empty());
    assert_eq!(input.next_deadline_ms(), Some(110));
    input.handle(30, layout(1, &[(0, button(1))])).unwrap();
    assert_eq!(
        input.handle(110, Change::Tick).unwrap(),
        vec![candidate(0, 1)]
    );
    assert_eq!(input.next_deadline_ms(), None);
    input.handle(111, report(&[])).unwrap();
    assert_eq!(
        input.handle(112, report(&[0])).unwrap(),
        vec![candidate(0, 1)]
    );
}

use rad_core::input::InputError;
#[test]
fn backwards_time_rejects_every_transition_without_mutation() {
    let mut input = Input::default();
    input.handle(10, layout(1, &[(0, button(1))])).unwrap();
    input.handle(10, Change::Threshold(100)).unwrap();
    input.handle(20, report(&[0])).unwrap();
    let before = input.clone();
    for change in [
        Change::Tick,
        report(&[]),
        layout(2, &[]),
        Change::Sleep,
        Change::Disconnect,
        Change::Threshold(0),
    ] {
        assert_eq!(input.handle(19, change), Err(InputError::TimeWentBackwards));
        assert_eq!(input, before);
    }
    assert_eq!(
        input.handle(120, Change::Tick).unwrap(),
        vec![candidate(0, 1)]
    );
}

#[test]
fn deadline_overflow_rolls_back_the_whole_report_and_maximum_deadline_is_valid() {
    let mut input = Input::default();
    input
        .handle(0, layout(1, &[(0, button(1)), (1, button(2))]))
        .unwrap();
    input.handle(0, Change::Threshold(100)).unwrap();
    input.handle(1, report(&[0])).unwrap();
    input.handle(2, Change::Threshold(u64::MAX)).unwrap();
    let before = input.clone();
    assert_eq!(
        input.handle(3, report(&[1])),
        Err(InputError::DeadlineOverflow)
    );
    assert_eq!(input, before);
    assert_eq!(
        input.handle(101, Change::Tick).unwrap(),
        vec![candidate(0, 1)]
    );
    let mut edge = Input::default();
    edge.handle(0, layout(1, &[(0, button(1))])).unwrap();
    edge.handle(0, Change::Threshold(u64::MAX)).unwrap();
    edge.handle(0, report(&[0])).unwrap();
    assert_eq!(edge.next_deadline_ms(), Some(u64::MAX));
    assert_eq!(
        edge.handle(u64::MAX, Change::Tick).unwrap(),
        vec![candidate(0, 1)]
    );
}

#[test]
fn malformed_layout_is_rejected_atomically_without_disarming_valid_pending_input() {
    let mut input = Input::default();
    input.handle(0, layout(1, &[(0, button(1))])).unwrap();
    input.handle(0, Change::Threshold(100)).unwrap();
    input.handle(1, report(&[0])).unwrap();
    let before = input.clone();
    let mut wrong_folder = layout(1, &[]);
    if let Change::Layout { folder, .. } = &mut wrong_folder {
        *folder = button(2);
    }
    let alias = Binding::new(
        ShortId::new(EntityKind::Button, 2).unwrap(),
        button(1).identity(),
    );
    let collision = Binding::new(button(1).short(), EntityIdentity::new(8, 1).unwrap());
    let folder_identity =
        Binding::new(button(1).short(), binding(EntityKind::Folder, 1).identity());
    for invalid in [
        wrong_folder,
        layout(1, &[(0, binding(EntityKind::Action, 1))]),
        layout(1, &[(0, button(1)), (31, button(1))]),
        layout(1, &[(0, button(1)), (31, alias)]),
        layout(1, &[(0, button(1)), (31, collision)]),
        layout(1, &[(0, folder_identity)]),
    ] {
        assert_eq!(input.handle(20, invalid), Err(InputError::InvalidLayout));
        assert_eq!(input, before);
    }
    assert_eq!(
        input.handle(101, Change::Tick).unwrap(),
        vec![candidate(0, 1)]
    );
}

#[test]
fn hold_ignores_press_threshold_and_ends_original_binding_on_layout_change() {
    use rad_core::{configuration::Gesture, input::ControlGesture};
    let mut input = Input::default();
    input.handle(0, layout(1, &[(0, button(1))])).unwrap();
    input.handle(0, Change::Threshold(9999)).unwrap();
    input
        .handle(
            0,
            Change::Gestures {
                selected: [Gesture::Hold; KEY_COUNT],
                long_press_ms: 500,
            },
        )
        .unwrap();
    assert_eq!(
        input.handle(1, report(&[0])).unwrap(),
        vec![Output::Gesture {
            key: 0,
            button: button(1),
            gesture: ControlGesture::HoldStart
        }]
    );
    assert_eq!(
        input.handle(2, layout(2, &[(0, button(2))])).unwrap(),
        vec![Output::Gesture {
            key: 0,
            button: button(1),
            gesture: ControlGesture::HoldEnd
        }]
    );
    assert!(input.handle(3, report(&[])).unwrap().is_empty());
}

#[test]
fn long_press_uses_its_captured_threshold_once_and_never_emits_press() {
    use rad_core::{configuration::Gesture, input::ControlGesture};
    let mut input = Input::default();
    input.handle(0, layout(1, &[(0, button(1))])).unwrap();
    input.handle(0, Change::Threshold(1)).unwrap();
    input
        .handle(
            0,
            Change::Gestures {
                selected: [Gesture::LongPress; KEY_COUNT],
                long_press_ms: 500,
            },
        )
        .unwrap();
    assert!(input.handle(1, report(&[0])).unwrap().is_empty());
    input
        .handle(
            2,
            Change::Gestures {
                selected: [Gesture::Press; KEY_COUNT],
                long_press_ms: 1,
            },
        )
        .unwrap();
    assert_eq!(input.next_deadline_ms(), Some(501));
    assert!(input.handle(500, Change::Tick).unwrap().is_empty());
    assert_eq!(
        input.handle(501, Change::Tick).unwrap(),
        vec![Output::Gesture {
            key: 0,
            button: button(1),
            gesture: ControlGesture::LongPress
        }]
    );
    assert!(input.handle(600, Change::Tick).unwrap().is_empty());
    assert!(input.handle(700, report(&[])).unwrap().is_empty());
}

#[test]
fn hold_cancellation_sends_one_end_and_wake_or_reconnect_never_replays_start() {
    use rad_core::{configuration::Gesture, input::ControlGesture};
    for cancel in [Change::Disconnect, Change::Sleep] {
        let mut input = Input::default();
        input.handle(0, layout(1, &[(0, button(1))])).unwrap();
        input
            .handle(
                0,
                Change::Gestures {
                    selected: [Gesture::Hold; KEY_COUNT],
                    long_press_ms: 500,
                },
            )
            .unwrap();
        input.handle(1, report(&[0])).unwrap();
        assert_eq!(
            input.handle(2, cancel).unwrap(),
            vec![Output::Gesture {
                key: 0,
                button: button(1),
                gesture: ControlGesture::HoldEnd
            }]
        );
        assert!(input.handle(3, report(&[])).unwrap().is_empty());
    }
}
