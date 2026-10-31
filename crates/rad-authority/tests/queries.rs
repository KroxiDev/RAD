use rad_authority::*;
use rad_core::{configuration::*, identity::*, permissions::*};
fn fixture() -> Candidate {
    let c = Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "visible".into(),
            },
        )
        .unwrap();
    let b = c.configuration().data().buttons[0].binding.identity();
    c.configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::AddAction {
                button: b,
                kind: ActionKind::Launcher(DesktopEntry::new("app.desktop".into()).unwrap()),
            },
        )
        .unwrap()
}
#[test]
fn child_override_visible_under_hidden_parent_and_hidden_get_is_not_found() {
    let c = fixture();
    let mut data = c.configuration().data().clone();
    let b = data.buttons[0].binding;
    let child = data.buttons[0].actions[0].binding;
    data.buttons[0].permissions =
        LocalPermissions::own(Permissions::new(false, false, false).unwrap());
    data.buttons[0].actions[0].permissions = LocalPermissions::own(Permissions::default());
    let c = c
        .configuration()
        .prepare_restore(c.registry(), Origin::Editor, true, data)
        .unwrap();
    let a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    assert_eq!(a.get(Origin::Automation, b.short()), None);
    assert_eq!(
        a.get(
            Origin::Automation,
            ShortId::new(EntityKind::Button, 99).unwrap()
        ),
        None
    );
    let Some(EntityView::Action(v)) = a.get(Origin::Automation, child.short()) else {
        panic!("own C must reveal child")
    };
    assert_eq!(v.parent, None);
    assert_eq!(v.binding, child);
    assert!(matches!(
        a.get(Origin::Editor, b.short()),
        Some(EntityView::Button(_))
    ));
}
#[test]
fn result_filters_current_c_not_snapshot_e_and_child_does_not_require_parent_c() {
    use rad_core::execution::*;
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding;
    let child = c.configuration().data().buttons[0].actions[0].binding;
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let t = a.reserve(1, Origin::Editor).unwrap();
    let e = a.submit(
        t,
        Command::Run {
            target: Target::Button(b.identity()),
        },
    );
    let id = e
        .iter()
        .find_map(|e| {
            if let Effect::Accepted { execution, .. } = e {
                Some(*execution)
            } else {
                None
            }
        })
        .unwrap();
    assert!(a.result(Origin::Automation, id).is_some());
    let t = a.reserve(1, Origin::Editor).unwrap();
    let mut data = fixture().configuration().data().clone();
    data.buttons[0].actions[0].permissions =
        LocalPermissions::own(Permissions::new(false, false, false).unwrap());
    let e = a.submit(
        t,
        Command::Restore {
            data: data.clone(),
            confirmed: true,
        },
    );
    let commit = e
        .iter()
        .find_map(|e| {
            if let Effect::Persist { id, .. } = e {
                Some(*id)
            } else {
                None
            }
        })
        .unwrap();
    a.complete(
        commit,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert_eq!(
        a.result(Origin::Automation, id).unwrap().steps,
        vec![Visible::Restricted]
    );
    data.buttons[0].permissions =
        LocalPermissions::own(Permissions::new(false, false, false).unwrap());
    data.buttons[0].actions[0].permissions = LocalPermissions::own(Permissions::default());
    let t = a.reserve(1, Origin::Editor).unwrap();
    let e = a.submit(
        t,
        Command::Restore {
            data,
            confirmed: true,
        },
    );
    let commit = e
        .iter()
        .find_map(|e| {
            if let Effect::Persist { id, .. } = e {
                Some(*id)
            } else {
                None
            }
        })
        .unwrap();
    a.complete(
        commit,
        Outcome::Applied {
            revision: 3,
            changed: true,
        },
    );
    assert_eq!(a.result(Origin::Automation, id), None);
    let d = e;
    drop(d);
    // Complete the existing full execution; then individually admit daughter.
    let initial = a.result(Origin::Editor, id).unwrap();
    assert_eq!(initial.steps.len(), 1);
    // Timeout releases owner, but retained result stays filtered with current C.
    a.advance(5000).unwrap();
    let t = a.reserve(2, Origin::Editor).unwrap();
    let e = a.submit(
        t,
        Command::Run {
            target: Target::Action(child.identity()),
        },
    );
    let id = e
        .iter()
        .find_map(|e| {
            if let Effect::Accepted { execution, .. } = e {
                Some(*execution)
            } else {
                None
            }
        })
        .unwrap();
    assert!(matches!(
        a.result(Origin::Automation, id).unwrap().steps.as_slice(),
        [Visible::Visible(_)]
    ));
}

#[test]
fn pages_bind_connection_and_visible_revision_and_enforce_count_and_measured_bytes() {
    let mut c = fixture();
    let folder = c.configuration().data().principal;
    for position in 1..20 {
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::CreateButton {
                    folder,
                    position,
                    name: format!("b{position}"),
                },
            )
            .unwrap();
        let button = c
            .configuration()
            .data()
            .buttons
            .last()
            .unwrap()
            .binding
            .identity();
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::AddAction {
                    button,
                    kind: ActionKind::Launcher(DesktopEntry::new("app.desktop".into()).unwrap()),
                },
            )
            .unwrap();
    }
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let page = a.list(1, Origin::Automation, None, |_| 1).unwrap();
    assert_eq!(page.entries.len(), 32);
    let cursor = page.next.unwrap();
    assert_eq!(
        a.list(2, Origin::Automation, Some(&cursor), |_| 1)
            .unwrap_err(),
        QueryError::CursorExpired
    );
    assert_eq!(
        a.list(1, Origin::Editor, Some(&cursor), |_| 1).unwrap_err(),
        QueryError::CursorExpired
    );
    assert_eq!(
        a.list(1, Origin::Automation, Some(&cursor), |_| 1)
            .unwrap()
            .entries
            .len(),
        8
    );
    assert_eq!(
        a.list(1, Origin::Automation, None, |_| 24 * 1024)
            .unwrap()
            .entries
            .len(),
        2
    );
    assert_eq!(
        a.list(1, Origin::Automation, None, |_| 48 * 1024 + 1)
            .unwrap_err(),
        QueryError::Capacity
    );
    let t = a.reserve(1, Origin::Editor).unwrap();
    let e = a.submit(
        t,
        vec![Edit::SetGlobalPermissions(
            Permissions::new(false, false, false).unwrap(),
        )],
    );
    let commit = e
        .iter()
        .find_map(|e| {
            if let Effect::Persist { id, .. } = e {
                Some(*id)
            } else {
                None
            }
        })
        .unwrap();
    a.complete(
        commit,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert_eq!(
        a.list(1, Origin::Automation, Some(&cursor), |_| 1)
            .unwrap_err(),
        QueryError::CursorExpired
    );
    assert!(
        a.list(1, Origin::Automation, None, |_| 1)
            .unwrap()
            .entries
            .is_empty()
    );
}

#[test]
fn editor_view_revision_tracks_execution_and_selection_not_only_store_revision() {
    use rad_core::execution::*;
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let before = a.view_revision(Origin::Editor);
    let t = a.reserve(1, Origin::Editor).unwrap();
    a.submit(
        t,
        Command::Run {
            target: Target::Button(b),
        },
    );
    assert_ne!(a.view_revision(Origin::Editor), before);
    assert_eq!(a.editor_view(Origin::Editor).unwrap().revision, 1);
}
