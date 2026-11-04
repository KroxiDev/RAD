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
                states: Default::default(),
                text: "changed".into(),
                image: None,
                text_style: Default::default(),
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
                            states: Default::default(),
                            text: "custom".into(),
                            image: None,
                            text_style: Default::default(),
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

#[test]
fn duplicate_preserves_content_with_fresh_ids_and_replace_controls_occupied_slots() {
    let c = fresh();
    let root = c.configuration().data().principal;
    let c = create(&c, root, 1, "source");
    let source = c.configuration().data().folders[0].slots[1].unwrap();
    let c = apply(
        &c,
        Edit::AddAction {
            button: source,
            kind: launcher(),
        },
    );
    let c = create(&c, root, 2, "previous");
    let original = c.configuration().data().buttons[0].clone();
    for replace in [false, true] {
        let copy = apply(
            &c,
            Edit::DuplicateButton {
                button: source,
                folder: root,
                position: 3,
                replace,
            },
        );
        let duplicate = copy.configuration().data().buttons.last().unwrap();
        assert_ne!(duplicate.binding, original.binding);
        assert_ne!(duplicate.actions[0].binding, original.actions[0].binding);
        let mut normalized = duplicate.clone();
        normalized.binding = original.binding;
        normalized.actions[0].binding = original.actions[0].binding;
        assert_eq!(normalized, original);
    }
    assert_eq!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::DuplicateButton {
                    button: source,
                    folder: root,
                    position: 2,
                    replace: false
                }
            )
            .unwrap_err(),
        ConfigurationError::Occupied
    );
    let copied = apply(
        &c,
        Edit::DuplicateButton {
            button: source,
            folder: root,
            position: 2,
            replace: true,
        },
    );
    assert_eq!(copied.configuration().data().buttons.len(), 2);
    assert!(
        copied
            .configuration()
            .data()
            .buttons
            .iter()
            .all(|b| b.name == "source")
    );
    let c = apply(
        &c,
        Edit::CreateFolder {
            parent: root,
            position: 4,
            name: "folder".into(),
        },
    );
    let folder = c.configuration().data().folders[1].binding.identity();
    assert_eq!(
        c.configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::DuplicateButton {
                    button: source,
                    folder,
                    position: 0,
                    replace: true
                }
            )
            .unwrap_err(),
        ConfigurationError::ProtectedNavigation
    );
    let copied = apply(
        &c,
        Edit::DuplicateButton {
            button: source,
            folder: root,
            position: 4,
            replace: true,
        },
    );
    assert_eq!(copied.configuration().data().folders.len(), 1);
    assert_eq!(copied.configuration().data().buttons.len(), 3);
}

#[test]
fn duplicate_folder_snapshots_complete_subtree_even_inside_itself_and_over_a_descendant() {
    let c = fresh();
    let root = c.configuration().data().principal;
    let c = apply(
        &c,
        Edit::CreateFolder {
            parent: root,
            position: 1,
            name: "F".into(),
        },
    );
    let f = c.configuration().data().folders[1].binding.identity();
    let entry = c.configuration().data().folders[0].slots[1].unwrap();
    let c = apply(
        &c,
        Edit::CreateFolder {
            parent: f,
            position: 1,
            name: "G".into(),
        },
    );
    let g = c.configuration().data().folders[2].binding.identity();
    let c = create(&c, g, 1, "leaf");
    let leaf = c.configuration().data().folders[2].slots[1].unwrap();
    let c = apply(
        &c,
        Edit::AddAction {
            button: leaf,
            kind: launcher(),
        },
    );
    let c = apply(
        &c,
        Edit::SetAppearance {
            button: leaf,
            appearance: Appearance {
                image: Some("a".repeat(64)),
                text: "leaf image".into(),
                states: std::collections::BTreeMap::from([(
                    "on".into(),
                    StateFace {
                        image: Some("b".repeat(64)),
                        text: "On".into(),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            },
        },
    );
    let before = c.configuration().data();
    let old_ids: std::collections::BTreeSet<_> = before
        .folders
        .iter()
        .map(|f| f.binding.identity())
        .chain(before.buttons.iter().flat_map(|b| {
            std::iter::once(b.binding.identity())
                .chain(b.actions.iter().map(|a| a.binding.identity()))
        }))
        .collect();
    for (destination, position, expected_folders) in [(root, 2, 5), (f, 2, 5), (f, 1, 4)] {
        let copied = apply(
            &c,
            Edit::DuplicateButton {
                button: entry,
                folder: destination,
                position,
                replace: true,
            },
        );
        let data = copied.configuration().data();
        assert_eq!(data.folders.len(), expected_folders);
        let copy_entry = data
            .folders
            .iter()
            .find(|f| f.binding.identity() == destination)
            .unwrap()
            .slots[position]
            .unwrap();
        assert!(!old_ids.contains(&copy_entry));
        let button = data
            .buttons
            .iter()
            .find(|b| b.binding.identity() == copy_entry)
            .unwrap();
        let ActionKind::EnterFolder(copy_f) = button.actions[0].kind else {
            panic!()
        };
        let folder = data
            .folders
            .iter()
            .find(|f| f.binding.identity() == copy_f)
            .unwrap();
        assert_eq!(folder.parent, Some(destination));
        let copy_g_button = data
            .buttons
            .iter()
            .find(|b| Some(b.binding.identity()) == folder.slots[1])
            .unwrap();
        let ActionKind::EnterFolder(copy_g) = copy_g_button.actions[0].kind else {
            panic!()
        };
        let child = data
            .folders
            .iter()
            .find(|f| f.binding.identity() == copy_g)
            .unwrap();
        assert_eq!(child.parent, Some(copy_f));
        let copy_leaf = data
            .buttons
            .iter()
            .find(|b| Some(b.binding.identity()) == child.slots[1])
            .unwrap();
        assert_eq!(
            copy_leaf.appearance,
            before
                .buttons
                .iter()
                .find(|b| b.binding.identity() == leaf)
                .unwrap()
                .appearance
        );
        assert_eq!(copy_leaf.actions[0].kind, launcher());
        for folder in [folder, child] {
            assert!(!old_ids.contains(&folder.binding.identity()));
            for id in folder.slots.iter().flatten() {
                assert!(!old_ids.contains(id));
                let b = data
                    .buttons
                    .iter()
                    .find(|b| b.binding.identity() == *id)
                    .unwrap();
                assert!(
                    b.actions
                        .iter()
                        .all(|a| !old_ids.contains(&a.binding.identity()))
                );
            }
            let back = data
                .buttons
                .iter()
                .find(|b| Some(b.binding.identity()) == folder.slots[0])
                .unwrap();
            assert_eq!(back.actions[0].kind, ActionKind::Return);
        }
        Configuration::from_data(data.clone(), copied.registry()).unwrap();
    }
}
