//! Private execution credential ownership. No configuration, view or persistence DTOs.
use crate::{Authority, plugins::PluginError};
use rad_core::execution::DispatchId;
use rad_plugin_contract::CredentialValues;
use std::sync::Arc;

/// Immutable trusted runtime handle. Never put this in a consultation response.
#[derive(Clone)]
pub struct CredentialSnapshot {
    revision: u64,
    values: Arc<CredentialValues>,
}
impl std::fmt::Debug for CredentialSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialSnapshot([redacted])")
    }
}
impl CredentialSnapshot {
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn values(&self) -> &CredentialValues {
        &self.values
    }
}
impl Authority {
    /// Publish only after durable vault mutation/reconciliation, in the admission turn.
    /// `selection` identifies installed metadata, independently of process lifetime.
    /// Replacing the current handle never rewrites an admitted execution's snapshot.
    pub fn publish_plugin_credentials(
        &mut self,
        id: &str,
        selection: u64,
        revision: u64,
        values: Arc<CredentialValues>,
    ) -> Result<(), PluginError> {
        let plugin = self
            .plugins
            .get_mut(id)
            .filter(|p| p.selection == selection)
            .ok_or(PluginError::Incompatible)?;
        let declared = plugin.manifest.credential_slots();
        if revision == 0
            || revision <= plugin.credential_revision
            || values
                .as_slice()
                .iter()
                .any(|v| !declared.contains(v.slot.as_str()))
        {
            return Err(PluginError::Incompatible);
        }
        plugin.credential_revision = revision;
        plugin.credentials = Some(CredentialSnapshot { revision, values });
        Ok(())
    }
    /// An uncertain vault cannot supply new snapshots; admitted work keeps its handles.
    pub fn invalidate_plugin_credentials(
        &mut self,
        id: &str,
        selection: u64,
    ) -> Result<(), PluginError> {
        let plugin = self
            .plugins
            .get_mut(id)
            .filter(|p| p.selection == selection)
            .ok_or(PluginError::Incompatible)?;
        plugin.credentials = None;
        Ok(())
    }
    pub fn current_plugin_credentials(
        &self,
        id: &str,
        selection: u64,
    ) -> Option<CredentialSnapshot> {
        self.plugins
            .get(id)
            .filter(|p| p.selection == selection)?
            .credentials
            .clone()
    }
    pub fn job_credentials(&self, id: DispatchId) -> Option<CredentialSnapshot> {
        self.plugin_jobs.get(&id)?.1.credentials.clone()
    }
    /// Runtime uses this bounded set to release only revisions without execution owners.
    /// Returned clones must not become a separate history of plaintext values.
    pub fn retained_plugin_credentials(&self, id: &str, selection: u64) -> Vec<CredentialSnapshot> {
        let current = self.current_plugin_credentials(id, selection);
        let bindings = self
            .plugin_runs
            .values()
            .filter_map(|run| run.get(id))
            .chain(
                self.plugin_jobs
                    .values()
                    .filter_map(|(plugin, binding)| (plugin == id).then_some(binding)),
            );
        current
            .into_iter()
            .chain(
                bindings
                    .filter(|binding| binding.selection == selection)
                    .filter_map(|binding| binding.credentials.clone()),
            )
            .map(|snapshot| (snapshot.revision(), snapshot))
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_values()
            .collect()
    }
}
