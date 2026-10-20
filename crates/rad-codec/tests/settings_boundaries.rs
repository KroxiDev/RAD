use rad_core::{configuration::*, identity::*};
#[test]
fn all_brightness_bytes_and_failed_restore_preserve_original_and_ledger() {
    let base = Configuration::prepare_new(&IdentityRegistry::new(11).unwrap()).unwrap();
    let before = base.configuration().clone();
    let ledger = base.registry().clone();
    for brightness in 0..=u8::MAX {
        let settings = SettingsData {
            brightness_percent: brightness,
            sleep_ms: u64::MAX,
            press_threshold_ms: u64::MAX,
            gui_language: Language::Es,
        };
        let changed = base.configuration().prepare_edit(
            base.registry(),
            Origin::Editor,
            Edit::SetSettings(settings),
        );
        assert_eq!(changed.is_ok(), brightness <= 100);
        if let Ok(candidate) = changed {
            let bytes = rad_codec::encode(candidate.configuration()).unwrap();
            let data = rad_codec::decode_data(&bytes).unwrap();
            assert_eq!(data.settings, settings);
            assert_eq!(
                Configuration::from_data(data, candidate.registry()).unwrap(),
                *candidate.configuration()
            );
        }
        let mut data = before.data().clone();
        data.settings = settings;
        let restored = before.prepare_restore(&ledger, Origin::Editor, true, data);
        assert_eq!(restored.is_ok(), brightness <= 100);
        assert_eq!(base.configuration(), &before);
        assert_eq!(base.registry(), &ledger);
    }
}
#[test]
fn invalid_restore_does_not_reserve_foreign_identity_or_publish_settings() {
    let base = Configuration::prepare_new(&IdentityRegistry::new(11).unwrap()).unwrap();
    let mut incoming = base.configuration().data().clone();
    let new_id = EntityIdentity::new(u128::MAX, u64::MAX).unwrap();
    incoming.principal = new_id;
    incoming.folders[0].binding =
        Binding::new(ShortId::new(EntityKind::Folder, 17).unwrap(), new_id);
    incoming.settings.brightness_percent = 101;
    let before = base.registry().clone();
    assert_eq!(
        base.configuration()
            .prepare_restore(base.registry(), Origin::Editor, true, incoming)
            .unwrap_err(),
        ConfigurationError::InvalidValue
    );
    assert_eq!(base.registry(), &before);
    let mut registry = before;
    assert_eq!(
        registry
            .allocate(EntityKind::Folder)
            .unwrap()
            .short()
            .number(),
        2
    );
    assert_eq!(base.configuration().settings().brightness_percent(), 100);
}
