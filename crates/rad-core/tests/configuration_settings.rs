use rad_core::{configuration::*, identity::IdentityRegistry};
fn initial() -> Candidate {
    Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap()
}
#[test]
fn new_configuration_has_complete_normative_settings_defaults() {
    let c = initial();
    let s = c.configuration().settings();
    assert_eq!(s.brightness_percent(), 100);
    assert_eq!(s.sleep_ms(), 300_000);
    assert!(s.sleep_enabled());
    assert_eq!(s.press_threshold_ms(), 0);
    assert_eq!(s.data(), c.configuration().data().settings);
}
#[test]
fn settings_constructor_rejects_bad_brightness_and_preserves_full_durations_and_language() {
    for brightness_percent in [0, 100, 101, 255] {
        let data = SettingsData {
            brightness_percent,
            sleep_ms: u64::MAX,
            press_threshold_ms: u64::MAX,
            long_press_threshold_ms: 500,
        };
        let result = Settings::from_data(data);
        if !(10..=100).contains(&brightness_percent) {
            assert_eq!(result, Err(ConfigurationError::InvalidValue));
        } else {
            assert_eq!(result.unwrap().data(), data);
        }
    }
    let s = Settings::from_data(SettingsData {
        sleep_ms: 0,
        ..SettingsData::default()
    })
    .unwrap();
    assert!(!s.sleep_enabled());
    assert_eq!(s.sleep_ms(), 0);
}
#[test]
fn reconstruction_validates_settings_without_silently_defaulting() {
    let c = initial();
    let mut data = c.configuration().data().clone();
    data.settings.brightness_percent = 101;
    assert_eq!(
        Configuration::from_data(data.clone(), c.registry()),
        Err(ConfigurationError::InvalidValue)
    );
    data.settings = SettingsData {
        brightness_percent: 10,
        sleep_ms: 0,
        press_threshold_ms: u64::MAX,
        long_press_threshold_ms: 500,
    };
    let restored = Configuration::from_data(data.clone(), c.registry()).unwrap();
    assert_eq!(restored.settings().data(), data.settings);
    assert_eq!(c.configuration().settings(), Settings::default());
}
#[test]
fn editor_settings_edit_produces_atomic_candidate_without_new_ids() {
    let c = initial();
    let settings = SettingsData {
        brightness_percent: 42,
        sleep_ms: 0,
        press_threshold_ms: 73,
        long_press_threshold_ms: 500,
    };
    let next = c
        .configuration()
        .prepare_edit(c.registry(), Origin::Editor, Edit::SetSettings(settings))
        .unwrap();
    assert_eq!(next.configuration().settings().data(), settings);
    assert_eq!(next.registry(), c.registry());
    assert_eq!(c.configuration().settings(), Settings::default());
    let (config, registry) = next.into_parts();
    assert_eq!(
        Configuration::from_data(config.data().clone(), &registry).unwrap(),
        config
    );
}
#[test]
fn automation_cannot_administer_settings_even_noop_or_with_global_modify() {
    let c = initial();
    for settings in [
        SettingsData::default(),
        SettingsData {
            brightness_percent: 10,
            ..SettingsData::default()
        },
    ] {
        assert_eq!(
            c.configuration().prepare_edit(
                c.registry(),
                Origin::Automation,
                Edit::SetSettings(settings)
            ),
            Err(ConfigurationError::NotAllowed)
        );
    }
    let noop = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData::default()),
        )
        .unwrap();
    assert_eq!(noop, c);
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
#[test]
fn restore_preserves_settings_and_current_ledger_without_merging_or_defaulting() {
    let c = initial();
    let edited = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                brightness_percent: 17,
                sleep_ms: u64::MAX,
                press_threshold_ms: 0,
                long_press_threshold_ms: 500,
            }),
        )
        .unwrap();
    let restored = edited
        .configuration()
        .prepare_restore(
            edited.registry(),
            Origin::Editor,
            true,
            c.configuration().data().clone(),
        )
        .unwrap();
    assert_eq!(restored.configuration(), c.configuration());
    assert_eq!(restored.registry(), edited.registry());
    let mut invalid = c.configuration().data().clone();
    invalid.settings.brightness_percent = 255;
    assert_eq!(
        edited
            .configuration()
            .prepare_restore(edited.registry(), Origin::Editor, true, invalid),
        Err(ConfigurationError::InvalidValue)
    );
    assert_eq!(edited.configuration().settings().brightness_percent(), 17);
}
#[test]
fn automation_batch_cannot_hide_settings_after_allowed_change() {
    let c = initial();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "old".into(),
            },
        )
        .unwrap();
    let b = c.configuration().data().buttons[0].binding.identity();
    assert_eq!(
        c.configuration().prepare_edits(
            c.registry(),
            Origin::Automation,
            &[
                Edit::SetName {
                    button: b,
                    name: "new".into()
                },
                Edit::SetSettings(SettingsData::default())
            ]
        ),
        Err(ConfigurationError::NotAllowed)
    );
    assert_eq!(c.configuration().data().buttons[0].name, "old");
    assert_eq!(c.configuration().settings(), Settings::default());
}
