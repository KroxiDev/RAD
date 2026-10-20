use super::ConfigurationError;
/// Non-secret, opaque plugin-owned settings. Availability does not affect retention.
/// The plugin contract will define their schema; credentials belong to the local vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSettings(Vec<u8>);
impl PluginSettings {
    pub fn new(bytes: Vec<u8>) -> Result<Self, ConfigurationError> {
        if bytes.len() > 4095 {
            return Err(ConfigurationError::InvalidValue);
        }
        Ok(Self(bytes))
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A portable hint, not an installed plugin or an authorization to install one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDependency {
    pub version: Option<PluginVersion>,
    pub origin: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}
