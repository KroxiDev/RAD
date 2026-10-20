use rad_authority::{Authority, Command, Effect, Rejection};
use rad_core::{configuration::*, identity::*, permissions::*};
#[test]
fn denied_modify_preserves_domain_error_for_direct_and_fifo_preparation() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "protected".into(),
            },
        )
        .unwrap();
    let id = c.configuration().data().buttons[0].binding.identity();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetButtonPermissions {
                button: id,
                permissions: LocalPermissions::own(Permissions::new(true, false, false).unwrap()),
            },
        )
        .unwrap();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    for command in [
        Command::Edits(vec![Edit::SetName {
            button: id,
            name: "changed".into(),
        }]),
        Command::PrepareEdits(vec![Box::new(move |_| {
            Ok(vec![Edit::SetName {
                button: id,
                name: "changed".into(),
            }])
        })]),
    ] {
        let t = a.reserve(1, Origin::Automation).unwrap();
        let request = t.request();
        let effects = a.submit(t, command);
        assert!(matches!(
            effects.as_slice(),
            [Effect::Rejected {
                reason: Rejection::Configuration(ConfigurationError::PermissionDenied),
                ..
            }]
        ));
        assert!(a.finish(request));
        assert_eq!(a.editor_view(Origin::Editor).unwrap().history_operations, 0);
    }
}
