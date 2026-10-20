use rad_core::{configuration::*, identity::*};
fn initial(ns: u128) -> Candidate {
    Configuration::prepare_new(&IdentityRegistry::new(ns).unwrap()).unwrap()
}
#[test]
fn backup_adopts_its_ids_in_another_never_edited_installation() {
    let source = initial(77);
    let target = initial(88);
    let before = target.clone();
    let restored = target
        .configuration()
        .prepare_portable_restore(
            target.registry(),
            Origin::Editor,
            true,
            source.configuration().data().clone(),
            source.registry().installation(),
            source.registry().counters(),
        )
        .unwrap()
        .candidate;
    assert_eq!(restored.configuration(), source.configuration());
    assert_eq!(target, before);
}
fn button(c: Candidate, position: usize) -> Candidate {
    c.configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position,
                name: format!("Button {position}"),
            },
        )
        .unwrap()
}
fn restore(target: &Candidate, source: &Candidate) -> PortableRestore {
    target
        .configuration()
        .prepare_portable_restore(
            target.registry(),
            Origin::Editor,
            true,
            source.configuration().data().clone(),
            source.registry().installation(),
            source.registry().counters(),
        )
        .unwrap()
}
#[test]
fn adoption_preserves_an_equivalent_empty_undo_root_and_redo_ids() {
    let source = button(initial(77), 0);
    let target = initial(88);
    let restored = restore(&target, &source);
    let empty = restored.history_base.clone().unwrap();
    assert!(empty.buttons.is_empty());
    let adoption = restored.candidate.root_adoption().unwrap();
    assert_eq!(
        adoption.previous,
        target.configuration().data().folders[0].binding
    );
    assert_eq!(empty.folders[0].binding, adoption.replacement);
    assert!(adoption.replacement.short().number() > source.registry().counters().folders);
    let c = restored.candidate;
    assert_eq!(c.configuration(), source.configuration());
    let undo = c
        .configuration()
        .prepare_restore(c.registry(), Origin::Editor, true, empty)
        .unwrap();
    assert!(undo.configuration().data().buttons.is_empty());
    let redo = undo
        .configuration()
        .prepare_restore(
            undo.registry(),
            Origin::Editor,
            true,
            c.configuration().data().clone(),
        )
        .unwrap();
    assert_eq!(redo.configuration(), c.configuration());
    assert_eq!(redo.registry().counters(), c.registry().counters());
    let next = button(redo, 1);
    assert_eq!(
        next.configuration().data().buttons[1]
            .binding
            .identity()
            .installation(),
        88
    );
    assert!(
        next.configuration().data().buttons[1]
            .binding
            .short()
            .number()
            > source.registry().counters().buttons
    );
}
#[test]
fn edited_then_undone_target_remaps_above_both_ranges_and_can_undo() {
    let source = button(button(button(initial(77), 0), 1), 2);
    let empty = initial(88);
    let edited = button(empty.clone(), 0);
    let target = edited
        .configuration()
        .prepare_restore(
            edited.registry(),
            Origin::Editor,
            true,
            empty.configuration().data().clone(),
        )
        .unwrap();
    let restored = restore(&target, &source);
    assert!(restored.history_base.is_none());
    assert!(restored.candidate.root_adoption().is_none());
    for b in &restored.candidate.configuration().data().buttons {
        assert!(b.binding.short().number() > 3);
        assert_eq!(b.binding.identity().installation(), 88);
    }
    let c = restored.candidate;
    assert_eq!(
        c.configuration()
            .data()
            .buttons
            .iter()
            .map(|b| b.binding.short().number())
            .collect::<Vec<_>>(),
        vec![4, 5, 6]
    );
    let undo = c
        .configuration()
        .prepare_restore(
            c.registry(),
            Origin::Editor,
            true,
            target.configuration().data().clone(),
        )
        .unwrap();
    assert_eq!(undo.configuration(), target.configuration());
    assert_eq!(undo.registry().counters(), c.registry().counters());
}
#[test]
fn same_installation_reactivates_pruned_ids_without_rolling_back_counters() {
    let source = button(initial(77), 0);
    let mut target = button(source.clone(), 1);
    let deleted = target
        .configuration()
        .prepare_edit(
            target.registry(),
            Origin::Editor,
            Edit::DeleteButton {
                button: source.configuration().data().buttons[0].binding.identity(),
            },
        )
        .unwrap();
    let (config, mut registry) = deleted.into_parts();
    registry.retain_bindings(&Default::default());
    target = config
        .prepare_edit(
            &registry,
            Origin::Editor,
            Edit::SetSettings(SettingsData::default()),
        )
        .unwrap();
    let restored = restore(&target, &source);
    assert_eq!(restored.candidate.configuration(), source.configuration());
    assert!(restored.history_base.is_none());
    assert_eq!(
        restored.candidate.registry().counters(),
        target.registry().counters()
    );
    assert_eq!(
        button(restored.candidate, 1).configuration().data().buttons[1]
            .binding
            .short()
            .number(),
        3
    );
}
#[test]
fn imported_navigation_and_plugin_settings_remap_together() {
    let source = initial(77);
    let source = source
        .configuration()
        .prepare_edit(
            source.registry(),
            Origin::Editor,
            Edit::CreateFolder {
                parent: source.configuration().data().principal,
                position: 0,
                name: "Folder".into(),
            },
        )
        .unwrap();
    let mut data = source.configuration().data().clone();
    data.plugin_settings.insert(
        "missing.plugin".into(),
        PluginSettings::new(vec![0, 255]).unwrap(),
    );
    data.plugin_dependencies.insert(
        "missing.plugin".into(),
        PluginDependency {
            version: Some(PluginVersion {
                major: 1,
                minor: 2,
                patch: 3,
            }),
            origin: Some("owner/repository".into()),
        },
    );
    let source = source
        .configuration()
        .prepare_restore(source.registry(), Origin::Editor, true, data)
        .unwrap();
    let target = button(initial(88), 0);
    let result = restore(&target, &source);
    assert_eq!(
        result.candidate.configuration().data().plugin_settings,
        source.configuration().data().plugin_settings
    );
    assert_eq!(
        result.candidate.configuration().data().plugin_dependencies,
        source.configuration().data().plugin_dependencies
    );
    Configuration::from_data(
        result.candidate.configuration().data().clone(),
        result.candidate.registry(),
    )
    .unwrap();
}
#[test]
fn rejected_restore_never_mutates_destination_or_reserves_a_partial_range() {
    let source = button(initial(77), 0);
    let target = initial(88);
    let before = target.clone();
    for (origin, confirmed, counters) in [
        (Origin::Automation, true, source.registry().counters()),
        (Origin::Editor, false, source.registry().counters()),
        (Origin::Editor, true, IdentityCounters::default()),
        (
            Origin::Editor,
            true,
            IdentityCounters {
                folders: u64::MAX,
                ..source.registry().counters()
            },
        ),
    ] {
        assert!(
            target
                .configuration()
                .prepare_portable_restore(
                    target.registry(),
                    origin,
                    confirmed,
                    source.configuration().data().clone(),
                    77,
                    counters
                )
                .is_err()
        );
        assert_eq!(target, before);
    }
}

#[test]
fn initial_local_backup_restores_only_its_root_after_foreign_adoption() {
    let local = initial(88);
    let mut data = local.configuration().data().clone();
    data.settings.brightness_percent = 30;
    data.plugin_settings.insert(
        "missing.plugin".into(),
        PluginSettings::new(vec![7]).unwrap(),
    );
    data.plugin_dependencies.insert(
        "missing.plugin".into(),
        PluginDependency {
            version: Some(PluginVersion {
                major: 1,
                minor: 2,
                patch: 3,
            }),
            origin: Some("owner/repository".into()),
        },
    );
    let backup = local
        .configuration()
        .prepare_restore(local.registry(), Origin::Editor, true, data)
        .unwrap();
    let foreign = button(initial(77), 0);
    let adopted = restore(&backup, &foreign).candidate;
    let restored = restore(&adopted, &backup);
    assert!(restored.history_base.is_none());
    assert!(restored.candidate.root_adoption().is_none());
    let root = restored.candidate.configuration().data().folders[0].binding;
    assert_eq!(root.identity().installation(), 88);
    assert!(root.short().number() > adopted.registry().counters().folders);
    assert!(root.identity().serial() > adopted.registry().counters().serial);
    let mut expected = backup.configuration().data().clone();
    expected.principal = root.identity();
    expected.folders[0].binding = root;
    assert_eq!(restored.candidate.configuration().data(), &expected);
    let undo = restored
        .candidate
        .configuration()
        .prepare_restore(
            restored.candidate.registry(),
            Origin::Editor,
            true,
            adopted.configuration().data().clone(),
        )
        .unwrap();
    assert_eq!(undo.configuration(), adopted.configuration());
}

#[test]
fn initial_root_never_reclaims_f1_after_repetition_or_pruning() {
    let backup = initial(88);
    assert_eq!(restore(&backup, &backup).candidate, backup);
    let adopted = restore(&backup, &button(initial(77), 0)).candidate;
    let once = restore(&adopted, &backup).candidate;
    let twice = restore(&once, &backup).candidate;
    assert!(
        twice.configuration().data().folders[0]
            .binding
            .short()
            .number()
            > once.registry().counters().folders
    );
    let (configuration, mut registry) = twice.into_parts();
    registry.retain_bindings(&Default::default());
    assert!(
        registry
            .records()
            .all(|r| r.binding().short().number() != 1)
    );
    let restored = configuration
        .prepare_portable_restore(
            &registry,
            Origin::Editor,
            true,
            backup.configuration().data().clone(),
            88,
            backup.registry().counters(),
        )
        .unwrap()
        .candidate;
    assert!(
        restored.configuration().data().folders[0]
            .binding
            .short()
            .number()
            > registry.counters().folders
    );
    assert_eq!(
        restored.configuration().data().folders[0]
            .binding
            .identity()
            .installation(),
        88
    );
}

#[test]
fn initial_root_exception_rejects_content_history_invalid_data_and_exhaustion() {
    let backup = initial(88);
    let adopted = restore(&backup, &button(initial(77), 0)).candidate;
    let with_content = button(backup.clone(), 0);
    for (data, counters) in [
        (
            with_content.configuration().data().clone(),
            with_content.registry().counters(),
        ),
        (
            with_content.configuration().data().clone(),
            backup.registry().counters(),
        ),
        (
            backup.configuration().data().clone(),
            IdentityCounters {
                buttons: 1,
                serial: 2,
                ..backup.registry().counters()
            },
        ),
        (
            {
                let mut data = backup.configuration().data().clone();
                data.settings.brightness_percent = 101;
                data
            },
            backup.registry().counters(),
        ),
    ] {
        assert!(
            adopted
                .configuration()
                .prepare_portable_restore(
                    adopted.registry(),
                    Origin::Editor,
                    true,
                    data,
                    88,
                    counters
                )
                .is_err()
        );
    }
    for exhausted in [
        IdentityCounters {
            folders: u64::MAX,
            ..adopted.registry().counters()
        },
        IdentityCounters {
            serial: u64::MAX,
            ..adopted.registry().counters()
        },
    ] {
        let mut registry = adopted.registry().clone();
        registry.reserve_counters(exhausted);
        let before = registry.clone();
        assert!(
            adopted
                .configuration()
                .prepare_portable_restore(
                    &registry,
                    Origin::Editor,
                    true,
                    backup.configuration().data().clone(),
                    88,
                    backup.registry().counters()
                )
                .is_err()
        );
        assert_eq!(registry, before);
    }
}
