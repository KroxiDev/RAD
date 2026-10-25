use rad_core::configuration::{ConfigurationError, Settings, SettingsData};
#[test]
fn configurable_brightness_has_a_ten_percent_floor_without_quantizing_values() {
    for brightness_percent in 0..10 {
        assert_eq!(
            Settings::from_data(SettingsData {
                brightness_percent,
                ..SettingsData::default()
            }),
            Err(ConfigurationError::InvalidValue)
        );
    }
    for brightness_percent in 10..=100 {
        assert_eq!(
            Settings::from_data(SettingsData {
                brightness_percent,
                ..SettingsData::default()
            })
            .unwrap()
            .brightness_percent(),
            brightness_percent
        );
    }
}
