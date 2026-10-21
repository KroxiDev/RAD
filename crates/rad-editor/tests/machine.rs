use rad_editor::{Request, machine::Change};

fn saved(request: Request) -> (Option<bool>, Option<bool>) {
    match request {
        Request::SetMachineSettings {
            use_stream_deck,
            start_with_session,
        } => (use_stream_deck, start_with_session),
        other => panic!("unexpected request: {other:?}"),
    }
}

#[test]
fn using_the_deck_also_starts_rad_with_the_computer_but_turning_it_off_touches_nothing_else() {
    let on = Change::use_stream_deck(true);
    assert_eq!(on.autostart(), Some(true));
    assert_eq!(saved(on.request()), (Some(true), Some(true)));
    let off = Change::use_stream_deck(false);
    assert_eq!(off.autostart(), None);
    assert_eq!(saved(off.request()), (Some(false), None));
    let start = Change::start_with_session(false);
    assert_eq!(start.autostart(), Some(false));
    assert_eq!(saved(start.request()), (None, Some(false)));
}

#[test]
fn a_failed_autostart_step_stores_nothing() {
    let change = Change::use_stream_deck(true);
    assert!(change.after_autostart(false).is_none());
    assert_eq!(
        saved(change.after_autostart(true).unwrap()),
        (Some(true), Some(true))
    );
}
