use rad_authority::{Authority, EntityView};
use rad_core::{configuration::*, identity::*, permissions::*};
#[test]
fn parent_binding_preserves_short_id_and_current_consultation_filter() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap();
    let root = c.configuration().data().folders[0].binding;
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateFolder {
                parent: root.identity(),
                position: 0,
                name: "Tools".into(),
            },
        )
        .unwrap();
    let folder = c
        .configuration()
        .data()
        .folders
        .iter()
        .find(|f| f.parent.is_some())
        .unwrap()
        .binding;
    let button = c
        .configuration()
        .data()
        .buttons
        .iter()
        .find(|b| matches!(b.actions[0].kind, ActionKind::EnterFolder(_)))
        .unwrap()
        .binding;
    let action = c
        .configuration()
        .data()
        .buttons
        .iter()
        .find(|b| b.binding == button)
        .unwrap()
        .actions[0]
        .binding;
    let c = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                Edit::SetButtonPermissions {
                    button: button.identity(),
                    permissions: LocalPermissions::own(
                        Permissions::new(false, false, false).unwrap(),
                    ),
                },
                Edit::SetActionPermissions {
                    action: action.identity(),
                    permissions: LocalPermissions::own(Permissions::default()),
                },
            ],
        )
        .unwrap();
    let a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    assert_eq!(
        a.folder(Origin::Editor, folder.short())
            .unwrap()
            .parent_binding,
        Some(root)
    );
    let Some(EntityView::Action(v)) = a.get(Origin::Editor, action.short()) else {
        panic!()
    };
    assert_eq!(v.parent_binding, Some(button));
    let Some(EntityView::Action(v)) = a.get(Origin::Automation, action.short()) else {
        panic!()
    };
    assert_eq!(v.parent_binding, None);
    assert_eq!(v.parent, None);
}
