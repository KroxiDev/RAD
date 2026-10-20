use rad_core::configuration::*;
use rad_core::identity::*;

#[test]
fn new_configuration_has_one_principal_with_32_empty_positions_and_coherent_registry() {
    let registry = IdentityRegistry::new(1).unwrap();
    let candidate = Configuration::prepare_new(&registry).unwrap();
    let data = candidate.configuration().data();
    assert_eq!(data.folders.len(), 1);
    assert_eq!(data.folders[0].slots, vec![None; 32]);
    assert_eq!(data.folders[0].parent, None);
    assert_eq!(data.folders[0].binding.identity(), data.principal);
    assert_eq!(
        candidate
            .registry()
            .records()
            .filter(|r| !r.retired())
            .count(),
        1
    );
    assert_eq!(registry.records().count(), 0);
}

#[test]
fn reconstruction_rejects_invalid_root_slots_and_registry_without_repair() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap();
    assert_eq!(
        Configuration::from_data(c.configuration().data().clone(), c.registry()).unwrap(),
        *c.configuration()
    );
    let mut data = c.configuration().data().clone();
    data.folders[0].slots.pop();
    assert!(Configuration::from_data(data, c.registry()).is_err());
    let mut data = c.configuration().data().clone();
    data.folders[0].parent = Some(data.principal);
    assert!(Configuration::from_data(data, c.registry()).is_err());
    let mut data = c.configuration().data().clone();
    data.folders.clear();
    assert!(Configuration::from_data(data, c.registry()).is_err());
    let mut data = c.configuration().data().clone();
    data.folders.push(data.folders[0].clone());
    assert!(Configuration::from_data(data, c.registry()).is_err());
    assert!(
        Configuration::from_data(
            c.configuration().data().clone(),
            &IdentityRegistry::new(1).unwrap()
        )
        .is_err()
    );
}

fn fresh() -> Candidate {
    Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap()
}
fn apply(c: &Candidate, edit: Edit) -> Candidate {
    c.configuration()
        .prepare_edit(c.registry(), Origin::Editor, edit)
        .unwrap()
}
fn create(c: &Candidate, folder: EntityIdentity, position: usize, name: &str) -> Candidate {
    apply(
        c,
        Edit::CreateButton {
            folder,
            position,
            name: name.into(),
        },
    )
}
#[test]
fn create_empty_button_is_atomic_allocates_identity_and_rejects_occupied_or_out_of_bounds() {
    let c = fresh();
    let root = c.configuration().data().principal;
    let next = create(&c, root, 31, "Launch");
    let data = next.configuration().data();
    assert_eq!(data.buttons.len(), 1);
    let b = &data.buttons[0];
    assert_eq!(data.folders[0].slots[31], Some(b.binding.identity()));
    assert_eq!(b.name, "Launch");
    assert_eq!(b.mode, Mode::Sequential);
    assert!(b.actions.is_empty());
    assert_eq!(next.registry().records().count(), 2);
    assert!(c.configuration().data().buttons.is_empty());
    for position in [31, 32] {
        assert!(
            next.configuration()
                .prepare_edit(
                    next.registry(),
                    Origin::Editor,
                    Edit::CreateButton {
                        folder: root,
                        position,
                        name: "bad".into()
                    }
                )
                .is_err()
        );
    }
    assert_eq!(
        Configuration::from_data(data.clone(), next.registry()).unwrap(),
        *next.configuration()
    );
}

#[test]
fn reconstruction_rejects_orphan_duplicate_missing_and_wrong_kind_bindings() {
    let c = fresh();
    let c = create(&c, c.configuration().data().principal, 0, "A");
    let data = c.configuration().data();
    let mut bad = data.clone();
    bad.folders[0].slots[0] = None;
    assert!(Configuration::from_data(bad, c.registry()).is_err());
    let mut bad = data.clone();
    bad.folders[0].slots[1] = bad.folders[0].slots[0];
    assert!(Configuration::from_data(bad, c.registry()).is_err());
    let mut bad = data.clone();
    bad.folders[0].slots[1] = Some(EntityIdentity::new(99, 99).unwrap());
    assert!(Configuration::from_data(bad, c.registry()).is_err());
    let mut bad = data.clone();
    bad.buttons.push(bad.buttons[0].clone());
    assert!(Configuration::from_data(bad, c.registry()).is_err());
    let mut bad = data.clone();
    bad.buttons[0].binding = bad.folders[0].binding;
    assert!(Configuration::from_data(bad, c.registry()).is_err());
}

#[test]
fn folder_creation_builds_exclusive_entry_and_return_at_zero_with_distinct_ids() {
    let c = fresh();
    let root = c.configuration().data().principal;
    let c = apply(
        &c,
        Edit::CreateFolder {
            parent: root,
            position: 4,
            name: "Work".into(),
        },
    );
    let d = c.configuration().data();
    assert_eq!(d.folders.len(), 2);
    assert_eq!(d.buttons.len(), 2);
    let child = &d.folders[1];
    assert_eq!(child.parent, Some(root));
    let entry = d
        .buttons
        .iter()
        .find(|b| Some(b.binding.identity()) == d.folders[0].slots[4])
        .unwrap();
    assert_eq!(entry.actions.len(), 1);
    assert_eq!(
        entry.actions[0].kind,
        ActionKind::EnterFolder(child.binding.identity())
    );
    let back = d
        .buttons
        .iter()
        .find(|b| Some(b.binding.identity()) == child.slots[0])
        .unwrap();
    assert_eq!(back.actions.len(), 1);
    assert_eq!(back.actions[0].kind, ActionKind::Return);
    assert_eq!(c.registry().records().count(), 6);
    assert_eq!(
        Configuration::from_data(d.clone(), c.registry()).unwrap(),
        *c.configuration()
    );
    let mut bad = d.clone();
    bad.folders[1].parent = Some(child.binding.identity());
    assert!(Configuration::from_data(bad, c.registry()).is_err());
    let mut bad = d.clone();
    bad.buttons
        .iter_mut()
        .find(|b| b.binding == back.binding)
        .unwrap()
        .actions[0]
        .kind = ActionKind::EnterFolder(root);
    assert!(Configuration::from_data(bad, c.registry()).is_err());
}

#[test]
fn launcher_reference_is_a_desktop_id_not_a_shell_command_or_path() {
    assert_eq!(
        DesktopEntry::new("org.example.App.desktop".into())
            .unwrap()
            .id(),
        "org.example.App.desktop"
    );
    for invalid in [
        "",
        "sh -c echo hello",
        "/tmp/app.desktop",
        "../app.desktop",
        "app.desktop\n",
        ".desktop",
    ] {
        assert!(DesktopEntry::new(invalid.into()).is_err(), "{invalid:?}");
    }
    assert!(ExternalType::new("".into(), "launch".into()).is_err());
    assert!(ExternalType::new("plugin".into(), "".into()).is_err());
}

fn launcher() -> ActionKind {
    ActionKind::Launcher(DesktopEntry::new("app.desktop".into()).unwrap())
}
#[test]
fn actions_have_own_identities_zero_through_twenty_and_the_next_is_an_atomic_rejection() {
    let c = fresh();
    let mut c = create(&c, c.configuration().data().principal, 0, "A");
    let button = c.configuration().data().buttons[0].binding.identity();
    for n in 0..20 {
        c = apply(
            &c,
            Edit::AddAction {
                button,
                kind: launcher(),
            },
        );
        let b = &c.configuration().data().buttons[0];
        assert_eq!(b.actions.len(), n + 1);
        assert_eq!(b.actions[n].binding.short().kind(), EntityKind::Action);
    }
    let before = c.clone();
    assert!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::AddAction {
                    button,
                    kind: launcher(),
                }
            )
            .is_err()
    );
    assert_eq!(c, before);
    let mut bad = c.configuration().data().clone();
    let duplicate = bad.buttons[0].actions[0].clone();
    bad.buttons[0].actions.push(duplicate);
    assert!(Configuration::from_data(bad, c.registry()).is_err());
}

#[test]
fn moving_swaps_both_buttons_preserves_ids_and_moves_subtree_without_cycles() {
    let c = fresh();
    let root = c.configuration().data().principal;
    let c = create(&c, root, 1, "A");
    let c = create(&c, root, 2, "B");
    let a = c.configuration().data().folders[0].slots[1].unwrap();
    let b = c.configuration().data().folders[0].slots[2].unwrap();
    let swapped = apply(
        &c,
        Edit::MoveButton {
            button: a,
            folder: root,
            position: 2,
        },
    );
    assert_eq!(swapped.configuration().data().folders[0].slots[1], Some(b));
    assert_eq!(swapped.configuration().data().folders[0].slots[2], Some(a));
    assert_eq!(swapped.registry(), c.registry());
    let c = apply(
        &swapped,
        Edit::CreateFolder {
            parent: root,
            position: 3,
            name: "child".into(),
        },
    );
    let child = c.configuration().data().folders[1].binding.identity();
    let entry = c.configuration().data().folders[0].slots[3].unwrap();
    let back = c.configuration().data().folders[1].slots[0].unwrap();
    let c = apply(
        &c,
        Edit::MoveButton {
            button: back,
            folder: child,
            position: 9,
        },
    );
    assert_eq!(c.configuration().data().folders[1].slots[9], Some(back));
    assert!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::MoveButton {
                    button: back,
                    folder: root,
                    position: 0
                }
            )
            .is_err()
    );
    assert!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::MoveButton {
                    button: entry,
                    folder: child,
                    position: 0
                }
            )
            .is_err()
    );
    let c = apply(
        &c,
        Edit::CreateFolder {
            parent: root,
            position: 4,
            name: "other".into(),
        },
    );
    let other = c.configuration().data().folders[2].binding.identity();
    let moved = apply(
        &c,
        Edit::MoveButton {
            button: entry,
            folder: other,
            position: 1,
        },
    );
    assert_eq!(moved.configuration().data().folders[1].parent, Some(other));
    assert_eq!(moved.configuration().data().folders[1].slots[9], Some(back));
}

#[test]
fn confirmed_subtree_deletion_retires_all_ids_and_never_deletes_principal_or_return_alone() {
    let c = fresh();
    let root = c.configuration().data().principal;
    let c = apply(
        &c,
        Edit::CreateFolder {
            parent: root,
            position: 0,
            name: "child".into(),
        },
    );
    let child = c.configuration().data().folders[1].binding.identity();
    let back = c.configuration().data().folders[1].slots[0].unwrap();
    let c = apply(
        &c,
        Edit::CreateFolder {
            parent: child,
            position: 1,
            name: "grandchild".into(),
        },
    );
    let before = c.clone();
    let deleted = apply(
        &c,
        Edit::DeleteFolder {
            folder: child,
            confirm_contents: true,
        },
    );
    assert_eq!(deleted.configuration().data().folders.len(), 1);
    assert!(deleted.configuration().data().buttons.is_empty());
    assert!(
        deleted.configuration().data().folders[0]
            .slots
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(
        deleted.registry().records().count(),
        c.registry().records().count()
    );
    assert_eq!(
        deleted
            .registry()
            .records()
            .filter(|r| !r.retired())
            .count(),
        1
    );
    for edit in [
        Edit::DeleteFolder {
            folder: child,
            confirm_contents: false,
        },
        Edit::DeleteFolder {
            folder: root,
            confirm_contents: true,
        },
        Edit::DeleteButton { button: back },
    ] {
        assert!(
            c.configuration()
                .prepare_edit(c.registry(), Origin::Editor, edit)
                .is_err()
        );
    }
    assert_eq!(c, before);
    let added = create(&deleted, root, 0, "new");
    assert!(
        added.configuration().data().buttons[0]
            .binding
            .short()
            .number()
            > 4
    );
}

#[test]
fn planning_reorders_existing_actions_without_changing_identities() {
    let c = fresh();
    let c = create(&c, c.configuration().data().principal, 0, "A");
    let b = c.configuration().data().buttons[0].binding.identity();
    let c = apply(
        &c,
        Edit::AddAction {
            button: b,
            kind: launcher(),
        },
    );
    let c = apply(
        &c,
        Edit::AddAction {
            button: b,
            kind: launcher(),
        },
    );
    let original = &c.configuration().data().buttons[0].actions;
    let order = vec![
        original[1].binding.identity(),
        original[0].binding.identity(),
    ];
    let changed = apply(
        &c,
        Edit::SetPlan {
            button: b,
            mode: Mode::Parallel,
            order: order.clone(),
        },
    );
    let button = &changed.configuration().data().buttons[0];
    assert_eq!(button.mode, Mode::Parallel);
    assert_eq!(
        button
            .actions
            .iter()
            .map(|a| a.binding.identity())
            .collect::<Vec<_>>(),
        order
    );
    assert_eq!(changed.registry(), c.registry());
    for bad_order in [vec![order[0], order[0]], vec![order[0]], vec![]] {
        assert!(
            c.configuration()
                .prepare_edit(
                    c.registry(),
                    Origin::Editor,
                    Edit::SetPlan {
                        button: b,
                        mode: Mode::Parallel,
                        order: bad_order,
                    }
                )
                .is_err()
        );
    }
}

#[test]
fn appearance_and_external_visual_are_independent_of_actions_and_availability() {
    let c = fresh();
    let c = create(&c, c.configuration().data().principal, 0, "A");
    let b = c.configuration().data().buttons[0].binding.identity();
    let visual = VisualSource::External {
        type_key: ExternalType::new("not-installed".into(), "cover".into()).unwrap(),
        configuration: vec![0, 255, 42],
    };
    let appearance = Appearance {
        text: "label".into(),
        image: Some("owned-media-key".into()),
        text_position: None,
    };
    let changed = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                Edit::SetName {
                    button: b,
                    name: "renamed".into(),
                },
                Edit::SetAppearance {
                    button: b,
                    appearance: appearance.clone(),
                },
                Edit::SetVisual {
                    button: b,
                    visual: visual.clone(),
                },
            ],
        )
        .unwrap();
    let button = &changed.configuration().data().buttons[0];
    assert_eq!(button.name, "renamed");
    assert_eq!(button.appearance, appearance);
    assert_eq!(button.visual, visual);
    assert!(button.actions.is_empty());
    assert_eq!(
        Configuration::from_data(changed.configuration().data().clone(), changed.registry())
            .unwrap(),
        *changed.configuration()
    );
}

#[test]
fn parameter_edits_preserve_type_and_id_and_external_payload_without_executor() {
    let c = fresh();
    let c = create(&c, c.configuration().data().principal, 0, "A");
    let b = c.configuration().data().buttons[0].binding.identity();
    let key = ExternalType::new("absent".into(), "action".into()).unwrap();
    let c = apply(
        &c,
        Edit::AddAction {
            button: b,
            kind: ActionKind::External {
                type_key: key.clone(),
                parameters: vec![1],
            },
        },
    );
    let a = c.configuration().data().buttons[0].actions[0].binding;
    let next = apply(
        &c,
        Edit::SetParameters {
            action: a.identity(),
            parameters: ActionParameters::External(vec![9, 8, 7]),
        },
    );
    assert_eq!(
        next.configuration().data().buttons[0].actions[0].kind,
        ActionKind::External {
            type_key: key,
            parameters: vec![9, 8, 7]
        }
    );
    assert_eq!(next.configuration().data().buttons[0].actions[0].binding, a);
    assert!(
        next.configuration()
            .prepare_edit(
                next.registry(),
                Origin::Editor,
                Edit::SetParameters {
                    action: a.identity(),
                    parameters: ActionParameters::Launcher(
                        DesktopEntry::new("x.desktop".into()).unwrap()
                    )
                }
            )
            .is_err()
    );
    let c = apply(
        &next,
        Edit::CreateFolder {
            parent: next.configuration().data().principal,
            position: 1,
            name: "F".into(),
        },
    );
    for navigation in &c.configuration().data().buttons[1..] {
        assert!(
            c.configuration()
                .prepare_edit(
                    c.registry(),
                    Origin::Editor,
                    Edit::SetParameters {
                        action: navigation.actions[0].binding.identity(),
                        parameters: ActionParameters::External(vec![])
                    }
                )
                .is_err()
        );
    }
}

use rad_core::permissions::{LocalPermissions, Permissions};
#[test]
fn only_editor_can_administer_even_if_requested_permission_value_is_unchanged() {
    let c = fresh();
    let root = c.configuration().data().principal;
    let c = create(&c, root, 0, "A");
    let b = c.configuration().data().buttons[0].binding.identity();
    let c = apply(
        &c,
        Edit::AddAction {
            button: b,
            kind: launcher(),
        },
    );
    let a = c.configuration().data().buttons[0].actions[0]
        .binding
        .identity();
    let none = Permissions::new(false, false, false).unwrap();
    let c = apply(&c, Edit::SetGlobalPermissions(none));
    assert_eq!(c.configuration().data().global_permissions, none);
    let own = LocalPermissions::own(Permissions::default());
    let c = apply(
        &c,
        Edit::SetButtonPermissions {
            button: b,
            permissions: own,
        },
    );
    let c = apply(
        &c,
        Edit::SetActionPermissions {
            action: a,
            permissions: own,
        },
    );
    assert_eq!(c.configuration().data().buttons[0].permissions, own);
    assert_eq!(
        c.configuration().data().buttons[0].actions[0].permissions,
        own
    );
    for edit in [
        Edit::SetGlobalPermissions(none),
        Edit::SetButtonPermissions {
            button: b,
            permissions: own,
        },
        Edit::SetActionPermissions {
            action: a,
            permissions: own,
        },
        Edit::CreateButton {
            folder: root,
            position: 1,
            name: "X".into(),
        },
        Edit::CreateFolder {
            parent: root,
            position: 1,
            name: "X".into(),
        },
        Edit::AddAction {
            button: b,
            kind: launcher(),
        },
        Edit::DeleteButton { button: b },
        Edit::DeleteFolder {
            folder: root,
            confirm_contents: true,
        },
    ] {
        assert_eq!(
            c.configuration()
                .prepare_edit(c.registry(), Origin::Automation, edit)
                .unwrap_err(),
            ConfigurationError::NotAllowed
        );
    }
}

#[test]
fn automation_checks_real_field_owners_and_aggregates_atomically_in_both_permission_directions() {
    let c = fresh();
    let c = create(&c, c.configuration().data().principal, 0, "A");
    let b = c.configuration().data().buttons[0].binding.identity();
    let c = apply(
        &c,
        Edit::AddAction {
            button: b,
            kind: launcher(),
        },
    );
    let a = c.configuration().data().buttons[0].actions[0]
        .binding
        .identity();
    let read = LocalPermissions::own(Permissions::new(true, false, false).unwrap());
    let modify = LocalPermissions::own(Permissions::default());
    let c = apply(
        &c,
        Edit::SetActionPermissions {
            action: a,
            permissions: read,
        },
    );
    let rename = Edit::SetName {
        button: b,
        name: "new".into(),
    };
    let params = Edit::SetParameters {
        action: a,
        parameters: ActionParameters::Launcher(DesktopEntry::new("other.desktop".into()).unwrap()),
    };
    assert!(
        c.configuration()
            .prepare_edit(c.registry(), Origin::Automation, rename.clone())
            .is_ok()
    );
    assert_eq!(
        c.configuration()
            .prepare_edits(
                c.registry(),
                Origin::Automation,
                &[rename.clone(), params.clone()]
            )
            .unwrap_err(),
        ConfigurationError::PermissionDenied
    );
    assert_eq!(c.configuration().data().buttons[0].name, "A");
    assert!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Automation,
                Edit::SetPlan {
                    button: b,
                    mode: Mode::Parallel,
                    order: vec![a],
                }
            )
            .is_ok()
    );
    let c = apply(
        &c,
        Edit::SetButtonPermissions {
            button: b,
            permissions: read,
        },
    );
    let c = apply(
        &c,
        Edit::SetActionPermissions {
            action: a,
            permissions: modify,
        },
    );
    assert!(
        c.configuration()
            .prepare_edit(c.registry(), Origin::Automation, params)
            .is_ok()
    );
    assert_eq!(
        c.configuration()
            .prepare_edit(c.registry(), Origin::Automation, rename)
            .unwrap_err(),
        ConfigurationError::PermissionDenied
    );
    assert!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Automation,
                Edit::SetName {
                    button: b,
                    name: "A".into()
                }
            )
            .is_ok()
    );
    for edit in [
        Edit::SetAppearance {
            button: b,
            appearance: Appearance {
                text: "changed".into(),
                image: None,
                text_position: None,
            },
        },
        Edit::SetVisual {
            button: b,
            visual: VisualSource::External {
                type_key: ExternalType::new("p".into(), "v".into()).unwrap(),
                configuration: vec![],
            },
        },
        Edit::SetPlan {
            button: b,
            mode: Mode::Parallel,
            order: vec![a],
        },
    ] {
        assert_eq!(
            c.configuration()
                .prepare_edit(c.registry(), Origin::Automation, edit)
                .unwrap_err(),
            ConfigurationError::PermissionDenied
        );
    }
}

#[test]
fn automation_move_requires_both_button_permissions_and_never_changes_folder() {
    let c = fresh();
    let root = c.configuration().data().principal;
    let c = create(&c, root, 0, "A");
    let c = create(&c, root, 1, "B");
    let a = c.configuration().data().buttons[0].binding.identity();
    let b = c.configuration().data().buttons[1].binding.identity();
    let c = apply(
        &c,
        Edit::SetButtonPermissions {
            button: b,
            permissions: LocalPermissions::own(Permissions::new(true, false, false).unwrap()),
        },
    );
    assert_eq!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Automation,
                Edit::MoveButton {
                    button: a,
                    folder: root,
                    position: 1
                }
            )
            .unwrap_err(),
        ConfigurationError::PermissionDenied
    );
    assert_eq!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Automation,
                Edit::MoveButton {
                    button: b,
                    folder: root,
                    position: 2
                }
            )
            .unwrap_err(),
        ConfigurationError::PermissionDenied
    );
    assert!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Automation,
                Edit::MoveButton {
                    button: a,
                    folder: root,
                    position: 2
                }
            )
            .is_ok()
    );
    let c = apply(
        &c,
        Edit::CreateFolder {
            parent: root,
            position: 3,
            name: "folder".into(),
        },
    );
    let child = c.configuration().data().folders[1].binding.identity();
    assert_eq!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Automation,
                Edit::MoveButton {
                    button: a,
                    folder: child,
                    position: 1
                }
            )
            .unwrap_err(),
        ConfigurationError::NotAllowed
    );
    let back = c.configuration().data().folders[1].slots[0].unwrap();
    assert!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Automation,
                Edit::MoveButton {
                    button: back,
                    folder: child,
                    position: 1
                }
            )
            .is_ok()
    );
}

#[test]
fn action_lifecycle_is_editor_only_and_navigation_cannot_be_removed_retyped_or_combined() {
    let c = fresh();
    let root = c.configuration().data().principal;
    let c = create(&c, root, 0, "A");
    let b = c.configuration().data().buttons[0].binding.identity();
    let c = apply(
        &c,
        Edit::AddAction {
            button: b,
            kind: launcher(),
        },
    );
    let a = c.configuration().data().buttons[0].actions[0]
        .binding
        .identity();
    let replacement = ActionKind::External {
        type_key: ExternalType::new("p".into(), "t".into()).unwrap(),
        parameters: vec![42],
    };
    let changed = apply(
        &c,
        Edit::ReplaceActionType {
            action: a,
            kind: replacement.clone(),
        },
    );
    assert_eq!(
        changed.configuration().data().buttons[0].actions[0].kind,
        replacement
    );
    let removed = apply(&changed, Edit::RemoveAction { action: a });
    assert!(removed.configuration().data().buttons[0].actions.is_empty());
    assert!(
        removed
            .registry()
            .records()
            .find(|r| r.binding().identity() == a)
            .unwrap()
            .retired()
    );
    for edit in [
        Edit::RemoveAction { action: a },
        Edit::ReplaceActionType {
            action: a,
            kind: launcher(),
        },
    ] {
        assert_eq!(
            c.configuration()
                .prepare_edit(c.registry(), Origin::Automation, edit)
                .unwrap_err(),
            ConfigurationError::NotAllowed
        );
    }
    let c = apply(
        &c,
        Edit::CreateFolder {
            parent: root,
            position: 1,
            name: "F".into(),
        },
    );
    for nav in &c.configuration().data().buttons[1..] {
        let button = nav.binding.identity();
        let action = nav.actions[0].binding.identity();
        for edit in [
            Edit::RemoveAction { action },
            Edit::ReplaceActionType {
                action,
                kind: launcher(),
            },
            Edit::AddAction {
                button,
                kind: launcher(),
            },
        ] {
            assert!(
                c.configuration()
                    .prepare_edit(c.registry(), Origin::Editor, edit)
                    .is_err()
            );
        }
        assert!(
            c.configuration()
                .prepare_edit(
                    c.registry(),
                    Origin::Editor,
                    Edit::SetAppearance {
                        button,
                        appearance: Appearance {
                            text: "custom".into(),
                            image: None,
                            text_position: None,
                        }
                    }
                )
                .is_ok()
        );
    }
    assert!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::ReplaceActionType {
                    action: a,
                    kind: ActionKind::Return
                }
            )
            .is_err()
    );
}

#[test]
fn duplicate_ordinary_button_allocates_all_new_ids_without_reusing_removed_ones() {
    let c = fresh();
    let root = c.configuration().data().principal;
    let c = create(&c, root, 0, "A");
    let b = c.configuration().data().buttons[0].binding.identity();
    let c = apply(
        &c,
        Edit::AddAction {
            button: b,
            kind: launcher(),
        },
    );
    let d = apply(
        &c,
        Edit::DuplicateButton {
            button: b,
            folder: root,
            position: 1,
        },
    );
    assert_eq!(d.configuration().data().buttons.len(), 2);
    let original = &d.configuration().data().buttons[0];
    let copy = &d.configuration().data().buttons[1];
    assert_ne!(original.binding, copy.binding);
    assert_ne!(original.actions[0].binding, copy.actions[0].binding);
    assert_eq!(original.actions[0].kind, copy.actions[0].kind);
    assert_eq!(original.name, copy.name);
    assert_eq!(
        d.registry().records().count(),
        c.registry().records().count() + 2
    );
    assert_eq!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Automation,
                Edit::DuplicateButton {
                    button: b,
                    folder: root,
                    position: 1
                }
            )
            .unwrap_err(),
        ConfigurationError::NotAllowed
    );
    let deleted = apply(&d, Edit::DeleteButton { button: b });
    assert!(
        deleted
            .registry()
            .records()
            .find(|r| r.binding().identity() == b)
            .unwrap()
            .retired()
    );
    let c = apply(
        &deleted,
        Edit::CreateFolder {
            parent: root,
            position: 0,
            name: "F".into(),
        },
    );
    let entry = c.configuration().data().folders[0].slots[0].unwrap();
    assert!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::DuplicateButton {
                    button: entry,
                    folder: root,
                    position: 2
                }
            )
            .is_err()
    );
}

#[test]
fn replacement_rebuilds_validated_configuration_without_rolling_registry_back() {
    let old = fresh();
    let root = old.configuration().data().principal;
    let c = create(&old, root, 0, "later");
    let issued = c.configuration().data().buttons[0].binding;
    let restored = c
        .configuration()
        .prepare_restore(
            c.registry(),
            Origin::Editor,
            true,
            old.configuration().data().clone(),
        )
        .unwrap();
    assert_eq!(restored.configuration(), old.configuration());
    assert!(
        restored
            .registry()
            .records()
            .find(|r| r.binding() == issued)
            .unwrap()
            .retired()
    );
    let next = create(&restored, root, 0, "new");
    assert_ne!(
        next.configuration().data().buttons[0].binding.short(),
        issued.short()
    );
    for (origin, confirmed) in [(Origin::Automation, true), (Origin::Editor, false)] {
        assert!(
            c.configuration()
                .prepare_restore(
                    c.registry(),
                    origin,
                    confirmed,
                    old.configuration().data().clone()
                )
                .is_err()
        );
    }
    let mut invalid = c.configuration().data().clone();
    invalid.buttons[0].binding = Binding::new(issued.short(), EntityIdentity::new(42, 1).unwrap());
    assert!(
        c.configuration()
            .prepare_restore(c.registry(), Origin::Editor, true, invalid)
            .is_err()
    );
    let mut invalid = old.configuration().data().clone();
    invalid.folders[0].slots.clear();
    assert!(
        c.configuration()
            .prepare_restore(c.registry(), Origin::Editor, true, invalid)
            .is_err()
    );
    assert_eq!(c.configuration().data().buttons[0].binding, issued);
}

#[test]
fn initialization_cannot_discard_an_existing_registry_and_failed_batch_does_not_consume_ids() {
    let c = fresh();
    assert_eq!(
        Configuration::prepare_new(c.registry()).unwrap_err(),
        ConfigurationError::RegistryMismatch
    );
    let root = c.configuration().data().principal;
    let before = c.clone();
    let result = c.configuration().prepare_edits(
        c.registry(),
        Origin::Editor,
        &[
            Edit::CreateButton {
                folder: root,
                position: 0,
                name: "valid prefix".into(),
            },
            Edit::CreateButton {
                folder: root,
                position: 32,
                name: "invalid suffix".into(),
            },
        ],
    );
    assert!(result.is_err());
    assert_eq!(c, before);
    let next = create(&c, root, 0, "first");
    assert_eq!(
        next.configuration().data().buttons[0]
            .binding
            .short()
            .number(),
        1
    );
    let exhausted = Binding::new(
        ShortId::new(EntityKind::Action, u64::MAX).unwrap(),
        EntityIdentity::new(99, 1).unwrap(),
    );
    let mut records: Vec<_> = c.registry().records().collect();
    records.push(IdentityRecord::new(exhausted, true));
    let registry = IdentityRegistry::from_records(1, records).unwrap();
    let before = registry.clone();
    assert_eq!(
        c.configuration()
            .prepare_edit(
                &registry,
                Origin::Editor,
                Edit::CreateFolder {
                    parent: root,
                    position: 0,
                    name: "fail midway".into()
                }
            )
            .unwrap_err(),
        ConfigurationError::Identity(IdentityError::ShortIdExhausted)
    );
    assert_eq!(registry, before);
}

#[test]
fn reconstruction_rejects_disconnected_navigation_missing_return_and_duplicate_action_ownership() {
    let c = fresh();
    let root = c.configuration().data().principal;
    let c = apply(
        &c,
        Edit::CreateFolder {
            parent: root,
            position: 0,
            name: "F".into(),
        },
    );
    let child = c.configuration().data().folders[1].binding.identity();
    let c = create(&c, root, 1, "A");
    let ordinary = c.configuration().data().buttons[2].binding.identity();
    let c = apply(
        &c,
        Edit::AddAction {
            button: ordinary,
            kind: launcher(),
        },
    );
    let good = c.configuration().data();
    let mut cases = Vec::new();
    let mut d = good.clone();
    d.folders[1].parent = None;
    cases.push(d);
    let mut d = good.clone();
    d.folders[1].parent = Some(EntityIdentity::new(99, 99).unwrap());
    cases.push(d);
    let mut d = good.clone();
    d.buttons[1].actions.clear();
    cases.push(d);
    let mut d = good.clone();
    d.buttons[0].actions[0].kind = ActionKind::EnterFolder(root);
    cases.push(d);
    let mut d = good.clone();
    d.buttons[2].actions[0].kind = ActionKind::EnterFolder(child);
    cases.push(d);
    let mut d = good.clone();
    d.buttons[2].actions[0].binding = d.buttons[1].actions[0].binding;
    cases.push(d);
    let mut d = good.clone();
    d.buttons[2].actions[0].binding = d.buttons[2].binding;
    cases.push(d);
    let mut d = good.clone();
    d.folders[1].slots.push(None);
    cases.push(d);
    for invalid in cases {
        assert!(Configuration::from_data(invalid, c.registry()).is_err());
    }
    let mut reordered = good.clone();
    reordered.folders.reverse();
    reordered.buttons.reverse();
    assert!(Configuration::from_data(reordered, c.registry()).is_ok());
}

#[test]
fn text_position_follows_the_image_until_one_is_chosen() {
    let face = |image: Option<&str>, text_position| Appearance {
        text: "Mute".into(),
        image: image.map(str::to_owned),
        text_position,
    };
    assert_eq!(
        face(None, None).effective_text_position(),
        TextPosition::Middle
    );
    assert_eq!(
        face(Some("media"), None).effective_text_position(),
        TextPosition::Bottom
    );
    for chosen in [
        TextPosition::Top,
        TextPosition::Middle,
        TextPosition::Bottom,
    ] {
        assert_eq!(face(None, Some(chosen)).effective_text_position(), chosen);
        assert_eq!(
            face(Some("media"), Some(chosen)).effective_text_position(),
            chosen
        );
    }
    assert_eq!(Appearance::default().text_position, None);
}

#[test]
fn key_reads_its_text_else_its_name_and_nothing_once_it_has_an_image() {
    let face = |text: &str, image: Option<&str>| Appearance {
        text: text.into(),
        image: image.map(str::to_owned),
        text_position: None,
    };
    assert_eq!(face("Go", None).label("Steam"), "Go");
    assert_eq!(face("Go", Some("media")).label("Steam"), "Go");
    assert_eq!(face("", None).label("Steam"), "Steam");
    assert_eq!(face("", Some("media")).label("Steam"), "");
}
