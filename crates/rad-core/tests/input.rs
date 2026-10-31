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
