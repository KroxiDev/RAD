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
fn hidden_edits_do_not_advance_automation_revision_but_visible_edits_do() {
    let c = fixture();
    let mut data = c.configuration().data().clone();
    let button = data.buttons[0].binding.identity();
    data.buttons[0].permissions =
        LocalPermissions::own(Permissions::new(false, false, false).unwrap());
    let c = c
        .configuration()
        .prepare_restore(c.registry(), Origin::Editor, true, data)
        .unwrap();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let hidden = a.view_revision(Origin::Automation);
    let editor = a.view_revision(Origin::Editor);
    let t = a.reserve(1, Origin::Editor).unwrap();
    let e = a.submit(
        t,
        vec![Edit::SetName {
            button,
            name: "hidden edit".into(),
        }],
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
    assert_eq!(a.view_revision(Origin::Automation), hidden);
    assert_ne!(a.view_revision(Origin::Editor), editor);
    let t = a.reserve(1, Origin::Editor).unwrap();
    let e = a.submit(
        t,
        vec![Edit::SetButtonPermissions {
            button,
            permissions: LocalPermissions::default(),
        }],
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
    assert_ne!(a.view_revision(Origin::Automation), hidden);
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
fn visible_occupation_changes_revision_without_exposing_hidden_owner() {
    use rad_core::execution::*;
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let old = a.view_revision(Origin::Automation);
    let t = a.reserve(1, Origin::Editor).unwrap();
    let e = a.submit(
        t,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert_ne!(a.view_revision(Origin::Automation), old);
    let busy = a.view_revision(Origin::Automation);
    let d = e
        .iter()
        .find_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d.id)
            } else {
                None
            }
        })
        .unwrap();
    a.job_complete(d, Completion::Succeeded).unwrap();
    assert_ne!(a.view_revision(Origin::Automation), busy);
}
#[test]
fn appearance_view_preserves_owned_reference_but_never_external_payload() {
    let c = fixture();
    let mut data = c.configuration().data().clone();
    let b = data.buttons[0].binding;
    data.buttons[0].appearance = Appearance {
        text: "hello".into(),
        image: None,
        text_position: None,
    };
    let kind = ExternalType::new("visual".into(), "image".into()).unwrap();
    data.buttons[0].visual = VisualSource::External {
        type_key: kind.clone(),
        configuration: b"private opaque config".to_vec(),
    };
    let c = c
        .configuration()
        .prepare_restore(c.registry(), Origin::Editor, true, data)
        .unwrap();
    let a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let v = a.appearance(Origin::Automation, b.short()).unwrap();
    assert_eq!(v.appearance.text, "hello");
    assert_eq!(v.visual, VisualView::External(kind));
    assert!(!format!("{v:?}").contains("private opaque"));
}
#[test]
fn observable_appearance_changes_invalidate_cursor_revision() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let before = a.view_revision(Origin::Automation);
    let t = a.reserve(1, Origin::Editor).unwrap();
    let e = a.submit(
        t,
        vec![Edit::SetAppearance {
            button: b,
            appearance: Appearance {
                text: "changed".into(),
                image: None,
                text_position: None,
            },
        }],
    );
    let id = e
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
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert_ne!(a.view_revision(Origin::Automation), before);
}

#[test]
fn folder_and_active_structure_only_reveal_consultable_content() {
    let c = fixture();
    let root = c.configuration().data().folders[0].binding;
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    assert_eq!(a.status(Origin::Automation).active, Some(root));
    assert!(matches!(
        a.folder(Origin::Automation, root.short()).unwrap().slots[0],
        Some(Visible::Visible(_))
    ));
    let t = a.reserve(1, Origin::Editor).unwrap();
    let e = a.submit(
        t,
        vec![Edit::SetGlobalPermissions(
            Permissions::new(false, false, false).unwrap(),
        )],
    );
    let id = e
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
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert_eq!(a.status(Origin::Automation).active, None);
    assert_eq!(a.folder(Origin::Automation, root.short()), None);
    assert_eq!(a.status(Origin::Editor).active, Some(root));
    assert!(a.folder(Origin::Editor, root.short()).is_some());
}
#[test]
fn selection_is_part_of_visible_revision_and_status_projection() {
    let c = fixture();
    let root = c.configuration().data().principal;
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateFolder {
                parent: root,
                position: 1,
                name: "sub".into(),
            },
        )
        .unwrap();
    let sub = c.configuration().data().folders[1].binding;
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let before = a.view_revision(Origin::Automation);
    a.navigate(Origin::Editor, sub.identity()).unwrap();
    assert_ne!(a.view_revision(Origin::Automation), before);
    assert_eq!(a.status(Origin::Automation).active, Some(sub));
}
#[test]
fn result_step_change_advances_visible_revision_while_owner_stays_busy() {
    use rad_core::execution::*;
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding.identity();
    let c = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[Edit::AddAction {
                button: b,
                kind: ActionKind::Launcher(DesktopEntry::new("other.desktop".into()).unwrap()),
            }],
        )
        .unwrap();
    let mut data = c.configuration().data().clone();
    data.buttons[0].mode = Mode::Parallel;
    let c = c
        .configuration()
        .prepare_restore(c.registry(), Origin::Editor, true, data)
        .unwrap();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let t = a.reserve(1, Origin::Editor).unwrap();
    let e = a.submit(
        t,
        Command::Run {
            target: Target::Button(b),
        },
    );
    let dispatch = e
        .iter()
        .find_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d.id)
            } else {
                None
            }
        })
        .unwrap();
    let before = a.view_revision(Origin::Automation);
    a.job_complete(dispatch, Completion::Succeeded).unwrap();
    assert_ne!(a.view_revision(Origin::Automation), before);
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
#[test]
fn actions_section_is_bounded_and_cursor_cannot_be_used_for_list() {
    let c = fixture();
    let b = c.configuration().data().buttons[0].binding;
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::AddAction {
                button: b.identity(),
                kind: ActionKind::Launcher(DesktopEntry::new("two.desktop".into()).unwrap()),
            },
        )
        .unwrap();
    let mut data = c.configuration().data().clone();
    data.buttons[0].actions[1].permissions =
        LocalPermissions::own(Permissions::new(false, false, false).unwrap());
    let c = c
        .configuration()
        .prepare_restore(c.registry(), Origin::Editor, true, data)
        .unwrap();
    let a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let page = a
        .actions(1, Origin::Automation, b.short(), None, |_| 30 * 1024)
        .unwrap();
    assert_eq!(page.entries.len(), 1);
    let cursor = page.next.unwrap();
    assert_eq!(
        a.list(1, Origin::Automation, Some(&cursor), |_| 1)
            .unwrap_err(),
        QueryError::CursorExpired
    );
    let page = a
        .actions(1, Origin::Automation, b.short(), Some(&cursor), |_| 1)
        .unwrap();
    assert_eq!(page.entries, vec![Visible::Restricted]);
    assert!(page.next.is_none());
}
#[test]
fn settings_are_editor_only_and_remain_published_until_applied() {
    let c = fixture();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let wanted = SettingsData {
        brightness_percent: 12,
        sleep_ms: 0,
        press_threshold_ms: 17,
    };
    assert_eq!(a.settings(Origin::Editor), Some(SettingsData::default()));
    assert_eq!(a.settings(Origin::Automation), None);
    let t = a.reserve(1, Origin::Editor).unwrap();
    let e = a.submit(t, vec![Edit::SetSettings(wanted)]);
    assert_eq!(a.settings(Origin::Editor), Some(SettingsData::default()));
    let id = e
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
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert_eq!(a.settings(Origin::Editor), Some(wanted));
    assert_eq!(a.next_deadline(), None);
}
#[test]
fn installed_executor_availability_changes_only_visible_projection() {
    let c = fixture();
    let mut data = c.configuration().data().clone();
    let kind = ExternalType::new("p".into(), "a".into()).unwrap();
    data.buttons[0].actions[0].kind = ActionKind::External {
        type_key: kind.clone(),
        parameters: vec![],
    };
    let c = c
        .configuration()
        .prepare_restore(c.registry(), Origin::Editor, true, data)
        .unwrap();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let before = a.view_revision(Origin::Automation);
    a.installed_types(vec![kind]);
    assert_ne!(a.view_revision(Origin::Automation), before);
    let visible = a.view_revision(Origin::Automation);
    a.installed_types(vec![
        ExternalType::new("p".into(), "a".into()).unwrap(),
        ExternalType::new("unrelated".into(), "hidden".into()).unwrap(),
    ]);
    assert_eq!(a.view_revision(Origin::Automation), visible);
}
#[test]
fn cursor_fixed_encoding_roundtrips_and_rejects_noncanonical_bytes_without_actor_mutation() {
    let c = fixture();
    let a = Authority::new(c, 1, std::num::NonZeroU128::new(u128::MAX).unwrap());
    let revision = a.view_revision(Origin::Automation).unwrap();
    assert!(revision.value() > 0);
    let page = a
        .list(u64::MAX, Origin::Automation, None, |_| 30 * 1024)
        .unwrap();
    let cursor = page.next.unwrap();
    let bytes = cursor.to_bytes();
    assert!(bytes.len() * 2 <= 128);
    let decoded = Cursor::from_bytes(&bytes).unwrap();
    assert_eq!(decoded, cursor);
    assert!(
        a.list(u64::MAX, Origin::Automation, Some(&decoded), |_| 1)
            .unwrap()
            .next
            .is_none()
    );
    assert!(Cursor::from_bytes(&bytes[..50]).is_err());
    let mut bad = bytes;
    bad[24] = 2;
    assert!(Cursor::from_bytes(&bad).is_err());
    let mut bad = bytes;
    bad[50] = 1;
    assert!(Cursor::from_bytes(&bad).is_err());
    assert_eq!(a.view_revision(Origin::Automation).unwrap(), revision);
}
