//! The window changes screen instead of opening floating windows; Back and Esc never lose
//! unapplied changes without the user saying so (E4).
use rad_editor::screens::{Leave, Navigation, Screen, Section};

#[test]
fn unapplied_changes_keep_the_screen_and_ask_until_applied_or_discarded() {
    let mut navigation = Navigation::default();
    navigation.open(Section::General);

    assert!(!navigation.back(true));
    assert_eq!(navigation.screen(), Screen::Settings(Section::General));
    assert!(navigation.asking(true));
    assert!(!navigation.back(true), "asking again does not discard");
    assert!(
        !navigation.asking(false),
        "once applied there is nothing to ask"
    );
    assert!(navigation.back(false));

    navigation.open(Section::Plugins);
    assert!(
        !navigation.asking(true),
        "a new visit starts without the question"
    );
    navigation.back(true);
    navigation.discard();
    assert_eq!(navigation.screen(), Screen::Keys);
    navigation.open(Section::Plugins);
    assert!(!navigation.asking(true));
}

#[test]
fn a_form_in_the_key_panel_closes_only_without_unapplied_changes() {
    let mut leave = Leave::default();
    assert!(leave.request(false));
    assert!(!leave.request(true));
    assert!(leave.asking(true));
    leave.reset();
    assert!(!leave.asking(true));
}
