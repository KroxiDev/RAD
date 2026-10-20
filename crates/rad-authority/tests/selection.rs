use rad_authority::{Authority, Effect, Outcome};
use rad_core::{
    configuration::{Configuration, Edit, Origin},
    identity::IdentityRegistry,
};
#[test]
fn publication_preserves_latest_active_folder_and_only_falls_back_when_deleted() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let root = c.configuration().data().principal;
    let c = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                Edit::CreateFolder {
                    parent: root,
                    position: 0,
                    name: "a".into(),
                },
                Edit::CreateFolder {
                    parent: root,
                    position: 1,
                    name: "b".into(),
                },
            ],
        )
        .unwrap();
    let fa = c.configuration().data().folders[1].binding.identity();
    let fb = c.configuration().data().folders[2].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    a.navigate(Origin::Editor, fa).unwrap();
    let t = a.reserve(1, Origin::Editor).unwrap();
    let e = a.submit(
        t,
        vec![Edit::DeleteFolder {
            folder: fa,
            confirm_contents: true,
        }],
    );
    let Effect::Persist { id, .. } = e[0] else {
        panic!()
    };
    a.navigate(Origin::Editor, fb).unwrap();
    a.complete(
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert_eq!(a.editor_view(Origin::Editor).unwrap().active, fb);
    let t = a.reserve(1, Origin::Editor).unwrap();
    let e = a.submit(
        t,
        vec![Edit::DeleteFolder {
            folder: fb,
            confirm_contents: true,
        }],
    );
    let Effect::Persist { id, .. } = e[0] else {
        panic!()
    };
    a.complete(
        id,
        Outcome::Applied {
            revision: 3,
            changed: true,
        },
    );
    assert_eq!(a.editor_view(Origin::Editor).unwrap().active, root);
    assert!(a.navigate(Origin::Editor, fb).is_err());
    assert!(a.navigate(Origin::Automation, root).is_err());
}
#[test]
fn navigation_dispatch_is_applied_in_authority_not_forwarded_as_external_job() {
    use rad_authority::Command;
    use rad_core::execution::Target;
    let c = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let root = c.configuration().data().principal;
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateFolder {
                parent: root,
                position: 0,
                name: "a".into(),
            },
        )
        .unwrap();
    let folder = c.configuration().data().folders[1].binding.identity();
    let button = c.configuration().data().folders[0].slots[0].unwrap();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let t = a.reserve(1, Origin::Editor).unwrap();
    let e = a.submit(
        t,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(e.iter().any(|e| matches!(e, Effect::Accepted { .. })));
    assert!(!e.iter().any(|e| matches!(e, Effect::Dispatch(_))));
    assert_eq!(a.editor_view(Origin::Editor).unwrap().active, folder);
    let t = a.reserve(1, Origin::Editor).unwrap();
    assert!(
        a.submit(
            t,
            Command::Run {
                target: Target::Button(button)
            }
        )
        .iter()
        .any(|e| matches!(e, Effect::Accepted { .. }))
    );
}
