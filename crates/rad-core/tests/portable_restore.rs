use rad_core::{configuration::*, identity::*};
fn initial(ns: u128) -> Candidate {
    Configuration::prepare_new(&IdentityRegistry::new(ns).unwrap()).unwrap()
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
