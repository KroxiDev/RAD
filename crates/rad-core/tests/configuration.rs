use rad_core::configuration::*;
use rad_core::identity::*;

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

fn launcher() -> ActionKind {
    ActionKind::Launcher(DesktopEntry::new("app.desktop".into()).unwrap())
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

use rad_core::permissions::{LocalPermissions, Permissions};

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
