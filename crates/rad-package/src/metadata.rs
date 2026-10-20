use crate::PackageError;
use rad_core::{
    configuration::Configuration,
    identity::{IdentityCounters, IdentityRecord, IdentityRegistry},
};

/// Counters belong to the exporting installation, not to the revision's root token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityMetadata {
    pub installation: u128,
    pub counters: IdentityCounters,
}
impl IdentityMetadata {
    pub(crate) fn encode(registry: &IdentityRegistry) -> [u8; 48] {
        let mut bytes = [0; 48];
        bytes[..16].copy_from_slice(&registry.installation().to_be_bytes());
        let counters = registry.counters();
        for (chunk, value) in bytes[16..].as_chunks_mut::<8>().0.iter_mut().zip([
            counters.folders,
            counters.buttons,
            counters.actions,
            counters.serial,
        ]) {
            chunk.copy_from_slice(&value.to_be_bytes());
        }
        bytes
    }
    pub(crate) fn decode(
        bytes: &[u8; 48],
        configuration: &Configuration,
    ) -> Result<Self, PackageError> {
        let number = |offset| u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let metadata = Self {
            installation: u128::from_be_bytes(bytes[..16].try_into().unwrap()),
            counters: IdentityCounters {
                folders: number(16),
                buttons: number(24),
                actions: number(32),
                serial: number(40),
            },
        };
        let data = configuration.data();
        let bindings =
            data.folders
                .iter()
                .map(|f| f.binding)
                .chain(data.buttons.iter().flat_map(|b| {
                    std::iter::once(b.binding).chain(b.actions.iter().map(|a| a.binding))
                }));
        IdentityRegistry::from_records_with_counters(
            metadata.installation,
            bindings.map(|b| IdentityRecord::new(b, false)),
            metadata.counters,
        )
        .map_err(|_| PackageError::InvalidConfiguration)?;
        Ok(metadata)
    }
    pub(crate) fn legacy(configuration: &Configuration) -> Result<Self, PackageError> {
        let data = configuration.data();
        let bindings =
            data.folders
                .iter()
                .map(|f| f.binding)
                .chain(data.buttons.iter().flat_map(|b| {
                    std::iter::once(b.binding).chain(b.actions.iter().map(|a| a.binding))
                }));
        let registry = IdentityRegistry::from_records(
            data.principal.installation(),
            bindings.map(|b| IdentityRecord::new(b, false)),
        )
        .map_err(|_| PackageError::InvalidConfiguration)?;
        Ok(Self {
            installation: registry.installation(),
            counters: registry.counters(),
        })
    }
}
