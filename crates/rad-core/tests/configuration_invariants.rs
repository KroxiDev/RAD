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
