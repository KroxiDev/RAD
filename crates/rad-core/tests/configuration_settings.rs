use rad_core::{configuration::*, identity::IdentityRegistry};
fn initial() -> Candidate {
    Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap()
}

#[test]
fn invalid_settings_step_cannot_be_hidden_by_later_valid_step_and_batch_allocations_roll_back() {
    let c = initial();
    let root = c.configuration().data().principal;
    let create = Edit::CreateButton {
        folder: root,
        position: 0,
        name: "new".into(),
    };
    let invalid = SettingsData {
        brightness_percent: 101,
        ..SettingsData::default()
    };
    assert_eq!(
        c.configuration().prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                create.clone(),
                Edit::SetSettings(invalid),
                Edit::SetSettings(SettingsData::default())
            ]
        ),
        Err(ConfigurationError::InvalidValue)
    );
    assert!(c.configuration().data().buttons.is_empty());
    let changed = SettingsData {
        ..SettingsData::default()
    };
    let good = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[create.clone(), Edit::SetSettings(changed)],
        )
        .unwrap();
    let simple = c
        .configuration()
        .prepare_edit(c.registry(), Origin::Editor, create.clone())
        .unwrap();
    assert_eq!(good.registry(), simple.registry());
    assert_eq!(good.configuration().settings().data(), changed);
    assert_eq!(
        c.configuration().prepare_edits(
            c.registry(),
            Origin::Editor,
            &[Edit::SetSettings(changed), create.clone(), create]
        ),
        Err(ConfigurationError::Occupied)
    );
    assert_eq!(c.configuration().settings(), Settings::default());
}
