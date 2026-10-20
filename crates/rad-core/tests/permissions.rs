use rad_core::permissions::{LocalPermissions, Permissions};

#[test]
fn global_defaults_allow_consult_and_modify_only() {
    let permissions = Permissions::default();
    assert!(permissions.consult());
    assert!(permissions.modify());
    assert!(!permissions.execute());
}

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
fn setting_modify_enables_consult_without_changing_execute() {
    let mut p = Permissions::new(false, false, false).unwrap();
    p.set_modify(true);
    assert_eq!((p.consult(), p.modify(), p.execute()), (true, true, false));
    p.set_modify(false);
    assert_eq!((p.consult(), p.modify(), p.execute()), (true, false, false));
    let mut p = Permissions::new(true, true, true).unwrap();
    p.set_modify(false);
    assert_eq!((p.consult(), p.modify(), p.execute()), (true, false, true));
}

#[test]
fn setting_execute_enables_consult_without_changing_modify() {
    let mut p = Permissions::new(false, false, false).unwrap();
    p.set_execute(true);
    assert_eq!((p.consult(), p.modify(), p.execute()), (true, false, true));
    p.set_execute(false);
    assert_eq!((p.consult(), p.modify(), p.execute()), (true, false, false));
    let mut p = Permissions::new(true, true, true).unwrap();
    p.set_execute(false);
    assert_eq!((p.consult(), p.modify(), p.execute()), (true, true, false));
}

#[test]
fn clearing_consult_clears_both_dependents_and_reenabling_does_not_restore_them() {
    let mut p = Permissions::new(true, true, true).unwrap();
    p.set_consult(false);
    assert_eq!(
        (p.consult(), p.modify(), p.execute()),
        (false, false, false)
    );
    p.set_consult(true);
    assert_eq!((p.consult(), p.modify(), p.execute()), (true, false, false));
    p.set_consult(true);
    assert_eq!((p.consult(), p.modify(), p.execute()), (true, false, false));
}

#[test]
fn button_and_action_inherit_the_complete_parent_set_by_default() {
    let button = LocalPermissions::default();
    let action = LocalPermissions::default();
    for global in [
        Permissions::new(false, false, false).unwrap(),
        Permissions::new(true, true, true).unwrap(),
    ] {
        let effective_button = button.resolve(global);
        assert_eq!(effective_button, global);
        assert_eq!(action.resolve(effective_button), global);
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

#[test]
fn own_value_preserves_explicit_override_even_equal_to_parent() {
    let parent = Permissions::default();
    assert_eq!(LocalPermissions::default().own_value(), None);
    assert_eq!(LocalPermissions::own(parent).own_value(), Some(parent));
}
