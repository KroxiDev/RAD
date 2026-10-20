//! Persistible settings only: no clocks, timers, hardware or locale probing.
use super::ConfigurationError;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    En,
    Es,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Untrusted reconstruction/edit input. Defaults are for new configurations only.
pub struct SettingsData {
    pub brightness_percent: u8,
    pub sleep_ms: u64,
    pub press_threshold_ms: u64,
    pub gui_language: Language,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Validated complete settings; no universal mutable setters.
pub struct Settings {
    data: SettingsData,
}
impl Settings {
    pub fn from_data(data: SettingsData) -> Result<Self, ConfigurationError> {
        if data.brightness_percent > 100 {
            return Err(ConfigurationError::InvalidValue);
        }
        Ok(Self { data })
    }
    // Only Configuration calls this after validating its private data.
    pub(super) fn validated(data: SettingsData) -> Self {
        Self { data }
    }
    pub fn data(self) -> SettingsData {
        self.data
    }
    pub fn brightness_percent(self) -> u8 {
        self.data.brightness_percent
    }
    pub fn sleep_ms(self) -> u64 {
        self.data.sleep_ms
    }
    pub fn press_threshold_ms(self) -> u64 {
        self.data.press_threshold_ms
    }
    pub fn gui_language(self) -> Language {
        self.data.gui_language
    }
    /// Zero explicitly disables sleep, rather than denoting an infinite deadline.
    pub fn sleep_enabled(self) -> bool {
        self.data.sleep_ms != 0
    }
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            data: SettingsData {
                brightness_percent: 100,
                sleep_ms: 300_000,
                press_threshold_ms: 0,
                gui_language: Language::En,
            },
        }
    }
}
impl Default for SettingsData {
    fn default() -> Self {
        Settings::default().data()
    }
}
