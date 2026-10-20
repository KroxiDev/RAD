use rad_core::configuration::*;
use rad_core::identity::*;
fn edit(c: &Candidate, e: Edit) -> Candidate {
    c.configuration()
        .prepare_edit(c.registry(), Origin::Editor, e)
        .unwrap()
}
fn folder(c: &Candidate, parent: EntityIdentity, position: usize) -> Candidate {
    edit(
        c,
        Edit::CreateFolder {
            parent,
            position,
            name: "F".into(),
        },
    )
}
#[test]
fn ledger_order_is_not_configuration_order() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap();
    let c = folder(&c, c.configuration().data().principal, 0);
    let mut records: Vec<_> = c.registry().records().collect();
    records.reverse();
    let reversed = IdentityRegistry::from_records(1, records).unwrap();
    let rebuilt = Configuration::from_data(c.configuration().data().clone(), &reversed).unwrap();
    let next = rebuilt
        .prepare_edit(
            &reversed,
            Origin::Editor,
            Edit::CreateButton {
                folder: rebuilt.data().principal,
                position: 1,
                name: "B".into(),
            },
        )
        .unwrap();
    assert_eq!(
        next.configuration()
            .data()
            .buttons
            .last()
            .unwrap()
            .binding
            .short()
            .number(),
        3
    );
}
#[test]
fn cross_folder_swap_updates_both_subtrees_and_rejects_ancestor_cycle() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap();
    let root = c.configuration().data().principal;
    let c = folder(&c, root, 0);
    let a = c.configuration().data().folders[1].binding.identity();
    let c = folder(&c, root, 1);
    let b = c.configuration().data().folders[2].binding.identity();
    let c = folder(&c, a, 1);
    let x = c.configuration().data().folders[3].binding.identity();
    let c = folder(&c, b, 1);
    let y = c.configuration().data().folders[4].binding.identity();
    let x_entry = c.configuration().data().folders[1].slots[1].unwrap();
    let moved = edit(
        &c,
        Edit::MoveButton {
            button: x_entry,
            folder: b,
            position: 1,
        },
    );
    assert_eq!(moved.configuration().data().folders[3].parent, Some(b));
    assert_eq!(moved.configuration().data().folders[4].parent, Some(a));
    let a_entry = moved.configuration().data().folders[0].slots[0].unwrap();
    assert!(
        moved
            .configuration()
            .prepare_edit(
                moved.registry(),
                Origin::Editor,
                Edit::MoveButton {
                    button: a_entry,
                    folder: y,
                    position: 1
                }
            )
            .is_err()
    );
    assert_ne!(x, y);
}
#[test]
fn return_cannot_be_displaced_across_folder_and_batch_failure_preserves_ledger() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap();
    let root = c.configuration().data().principal;
    let c = folder(&c, root, 0);
    let child = c.configuration().data().folders[1].binding.identity();
    let c = edit(
        &c,
        Edit::CreateButton {
            folder: root,
            position: 1,
            name: "B".into(),
        },
    );
    let ordinary = c.configuration().data().folders[0].slots[1].unwrap();
    let before = c.clone();
    assert!(
        c.configuration()
            .prepare_edits(
                c.registry(),
                Origin::Editor,
                &[
                    Edit::SetName {
                        button: ordinary,
                        name: "prefix".into()
                    },
                    Edit::MoveButton {
                        button: ordinary,
                        folder: child,
                        position: 0
                    }
                ]
            )
            .is_err()
    );
    assert_eq!(c, before);
}
