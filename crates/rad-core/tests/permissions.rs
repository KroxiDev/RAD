use rad_core::permissions::{LocalPermissions, Permissions};

#[test]
fn constructor_rejects_contradictions_and_preserves_valid_sets() {
    for (c, m, e, valid) in [
        (false, false, false, true),
        (false, false, true, false),
        (false, true, false, false),
        (false, true, true, false),
        (true, false, false, true),
        (true, false, true, true),
        (true, true, false, true),
        (true, true, true, true),
    ] {
        let result = Permissions::new(c, m, e);
        assert_eq!(result.is_ok(), valid, "C={c} M={m} E={e}");
        if let Ok(p) = result {
            assert_eq!((p.consult(), p.modify(), p.execute()), (c, m, e));
        }
    }
}

#[test]
fn own_sets_replace_in_both_directions_at_button_and_action_levels() {
    let none = Permissions::new(false, false, false).unwrap();
    let all = Permissions::new(true, true, true).unwrap();
    let consult_only = Permissions::new(true, false, false).unwrap();
    for (global, own) in [(all, consult_only), (none, all)] {
        let button = LocalPermissions::own(own).resolve(global);
        assert_eq!(button, own);
        assert_eq!(LocalPermissions::default().resolve(button), own);
        let inherited_button = LocalPermissions::default().resolve(global);
        assert_eq!(LocalPermissions::own(own).resolve(inherited_button), own);
    }
    let denied_button = LocalPermissions::own(none).resolve(all);
    assert_eq!(LocalPermissions::own(all).resolve(denied_button), all);
    let allowed_button = LocalPermissions::own(all).resolve(none);
    assert_eq!(LocalPermissions::own(none).resolve(allowed_button), none);
}
