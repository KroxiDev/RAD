use rad_authority::{Authority, Command, Effect, Outcome};
use rad_core::{configuration::*, identity::*};
fn actor() -> Authority {
    Authority::new(
        Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap(),
        1,
        std::num::NonZeroU128::new(1).unwrap(),
    )
}
fn submit(a: &mut Authority, origin: Origin, c: Command) -> Vec<Effect> {
    let t = a.reserve(1, origin).unwrap();
    a.submit(t, c)
}
fn brightness(n: u8) -> Edit {
    Edit::SetSettings(SettingsData {
        brightness_percent: n,
        ..SettingsData::default()
    })
}
#[test]
fn deferred_preparation_sees_fifo_publication_and_prior_batch_step() {
    let mut a = actor();
    let first = submit(&mut a, Origin::Editor, vec![brightness(40)].into());
    let Effect::Persist { id, .. } = first[0] else {
        panic!("first persist")
    };
    let pending = submit(
        &mut a,
        Origin::Editor,
        Command::PrepareEdits(vec![
            Box::new(|cfg| Ok(vec![brightness(cfg.settings().brightness_percent() + 1)])),
            Box::new(|cfg| Ok(vec![brightness(cfg.settings().brightness_percent() + 1)])),
        ]),
    );
    assert!(pending.is_empty());
    let effects = a.complete(
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    let (second, candidate) = effects
        .iter()
        .find_map(|e| match e {
            Effect::Persist { id, candidate, .. } => Some((*id, candidate)),
            _ => None,
        })
        .expect("deferred persist");
    assert_eq!(
        candidate.configuration().settings().brightness_percent(),
        42
    );
    assert_eq!(a.settings(Origin::Editor).unwrap().brightness_percent, 40);
    a.complete(
        second,
        Outcome::Applied {
            revision: 3,
            changed: true,
        },
    );
    assert_eq!(a.settings(Origin::Editor).unwrap().brightness_percent, 42);
}
#[test]
fn deferred_failure_is_atomic_and_origin_is_not_delegated() {
    let mut a = actor();
    let effects = submit(
        &mut a,
        Origin::Editor,
        Command::PrepareEdits(vec![
            Box::new(|_| Ok(vec![brightness(42)])),
            Box::new(|_| Err(ConfigurationError::NotFound)),
        ]),
    );
    assert!(matches!(effects.as_slice(), [Effect::Rejected { .. }]));
    assert_eq!(a.settings(Origin::Editor).unwrap().brightness_percent, 100);
    let effects = submit(
        &mut a,
        Origin::Automation,
        Command::PrepareEdits(vec![Box::new(|_| Ok(vec![brightness(42)]))]),
    );
    assert!(matches!(effects.as_slice(), [Effect::Rejected { .. }]));
    assert_eq!(a.editor_view(Origin::Editor).unwrap().history_operations, 0);
}
#[test]
fn result_numbers_resolve_retained_id_with_current_consultation() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "run".into(),
            },
        )
        .unwrap();
    let b = c.configuration().data().buttons[0].binding;
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(u128::MAX).unwrap());
    let effects = submit(&mut a, Origin::Editor, Command::RunId { id: b.short() });
    let id = effects
        .iter()
        .find_map(|e| {
            if let Effect::Accepted { execution, .. } = e {
                Some(*execution)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(
        a.result_id(Origin::Editor, id.session(), id.counter()),
        a.result(Origin::Editor, id)
    );
    assert!(a.result_id(Origin::Editor, 1, id.counter()).is_none());
    let e = submit(
        &mut a,
        Origin::Editor,
        vec![Edit::SetButtonPermissions {
            button: b.identity(),
            permissions: rad_core::permissions::LocalPermissions::own(
                rad_core::permissions::Permissions::new(false, false, false).unwrap(),
            ),
        }]
        .into(),
    );
    let Effect::Persist { id: commit, .. } = e[0] else {
        panic!()
    };
    a.complete(
        commit,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert!(
        a.result_id(Origin::Automation, id.session(), id.counter())
            .is_none()
    );
    assert!(
        a.result_id(Origin::Editor, id.session(), id.counter())
            .is_some()
    );
}
#[test]
fn navigate_short_id_resolves_published_folder_without_automation_bypass() {
    let mut a = actor();
    let folder = a.status(Origin::Editor).active.unwrap();
    assert!(a.navigate_id(Origin::Automation, folder.short()).is_err());
    let effects = a.navigate_id(Origin::Editor, folder.short()).unwrap();
    assert!(
        matches!(effects.as_slice(),[Effect::SelectionChanged{folder:f}] if *f == folder.identity())
    );
    assert!(
        a.navigate_id(
            Origin::Editor,
            ShortId::new(EntityKind::Folder, 99).unwrap()
        )
        .is_err()
    );
}
#[test]
fn editor_projection_preserves_fields_and_own_equal_parent() {
    use rad_authority::EntityView;
    use rad_core::permissions::{LocalPermissions, Permissions};
    let c = Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap();
    let root = c.configuration().data().principal;
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: root,
                position: 3,
                name: "run".into(),
            },
        )
        .unwrap();
    let b = c.configuration().data().buttons[0].binding;
    let c = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                Edit::SetButtonPermissions {
                    button: b.identity(),
                    permissions: LocalPermissions::own(Permissions::default()),
                },
                Edit::AddAction {
                    button: b.identity(),
                    kind: ActionKind::Launcher(DesktopEntry::new("a.desktop".into()).unwrap()),
                },
                Edit::AddAction {
                    button: b.identity(),
                    kind: ActionKind::Launcher(DesktopEntry::new("b.desktop".into()).unwrap()),
                },
            ],
        )
        .unwrap();
    let action = c.configuration().data().buttons[0].actions[0].binding;
    let a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let Some(EntityView::Button(v)) = a.get(Origin::Editor, b.short()) else {
        panic!()
    };
    assert_eq!(v.position, 3);
    assert_eq!(v.folder.unwrap().identity(), root);
    assert_eq!(
        v.local_permissions.own_value(),
        Some(Permissions::default())
    );
    let Some(EntityView::Action(v)) = a.get(Origin::Editor, action.short()) else {
        panic!()
    };
    assert_eq!(v.desktop_entry.unwrap().id(), "a.desktop");
    assert_eq!(v.local_permissions.own_value(), None);
    assert_eq!(
        a.global_permissions(Origin::Editor),
        Some(Permissions::default())
    );
    assert_eq!(a.global_permissions(Origin::Automation), None);
}
#[test]
fn navigation_projection_uses_entry_name_and_filters_hidden_destination() {
    use rad_authority::EntityView;
    use rad_core::permissions::{LocalPermissions, Permissions};
    let c = Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap();
    let root = c.configuration().data().principal;
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateFolder {
                parent: root,
                position: 1,
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
    let entry = c
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
        .find(|b| b.binding == entry)
        .unwrap()
        .actions[0]
        .binding;
    let back = c
        .configuration()
        .data()
        .buttons
        .iter()
        .find(|b| matches!(b.actions[0].kind, ActionKind::Return))
        .unwrap()
        .binding;
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetButtonPermissions {
                button: back.identity(),
                permissions: LocalPermissions::own(Permissions::new(false, false, false).unwrap()),
            },
        )
        .unwrap();
    let a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let view = a.folder(Origin::Editor, folder.short()).unwrap();
    assert_eq!(view.name.as_deref(), Some("Tools"));
    assert_eq!(view.entry_button, Some(entry));
    let Some(EntityView::Action(v)) = a.get(Origin::Editor, action.short()) else {
        panic!()
    };
    assert_eq!(v.destination, Some(folder));
    let Some(EntityView::Action(v)) = a.get(Origin::Automation, action.short()) else {
        panic!()
    };
    assert_eq!(v.destination, None);
}
