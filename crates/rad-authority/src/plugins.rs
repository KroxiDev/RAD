//! Serialized, pure version reservations; never spawns or waits on a plugin.
pub use crate::plugin_credentials::CredentialSnapshot;
use crate::*;
use rad_core::configuration::{ActionKind, ExternalType, VisualSource};
use rad_plugin_contract::{Document, Manifest};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginError {
    InUse,
    PermissionDenied,
    InvalidPermit,
    Residual,
    Incompatible,
    PluginLimit,
    PublicationPending,
}
pub(super) struct Plugin {
    pub(super) manifest: Manifest,
    pub(super) selection: u64,
    pub(super) generation: Option<u64>,
    pub(super) credential_revision: u64,
    pub(super) credentials: Option<CredentialSnapshot>,
    failed: bool,
}
#[derive(Clone)]
pub(super) struct ExecutionBinding {
    pub selection: u64,
    pub generation: Option<u64>,
    pub credentials: Option<CredentialSnapshot>,
    pub settings: Option<Vec<u8>>,
}
pub(super) struct ExecutionSelection {
    pub bindings: BTreeMap<String, ExecutionBinding>,
    pub budgets: BTreeMap<EntityIdentity, rad_core::execution::ResponseBudget>,
}
impl Authority {
    /// Register an explicit selection without launching a process. Even reinstalling
    /// identical metadata creates a new identity; ordinary wake uses activate_plugin.
    pub fn register_plugin(&mut self, manifest: Manifest, target: &str) -> Result<(), PluginError> {
        manifest
            .validate(rad_plugin_contract::HOST_API, target)
            .map_err(|_| PluginError::Incompatible)?;
        self.check_plugin_candidate(&manifest, target)?;
        let selection = self.next_plugin_selection;
        self.next_plugin_selection = selection.checked_add(1).ok_or(PluginError::PluginLimit)?;
        let id = manifest.id.clone();
        self.installed.retain(|t| t.plugin() != id);
        self.installed.extend(
            manifest.actions.iter().map(|d| {
                ExternalType::new(id.clone(), d.id.clone()).expect("validated identifiers")
            }),
        );
        self.plugins.insert(
            id,
            Plugin {
                manifest,
                selection,
                generation: None,
                failed: false,
                credential_revision: 0,
                credentials: None,
            },
        );
        self.refresh_views();
        Ok(())
    }
    /// Compatibility port for callers that already completed the host handshake.
    pub fn install_plugin(
        &mut self,
        manifest: Manifest,
        generation: u64,
        target: &str,
    ) -> Result<(), PluginError> {
        if generation == 0 {
            return Err(PluginError::Incompatible);
        }
        let id = manifest.id.clone();
        if !self
            .plugins
            .get(&id)
            .is_some_and(|p| p.manifest == manifest)
        {
            self.register_plugin(manifest, target)?;
        } else {
            manifest
                .validate(rad_plugin_contract::HOST_API, target)
                .map_err(|_| PluginError::Incompatible)?;
        }
        let selection = self.plugins[&id].selection;
        self.activate_plugin(&id, selection, generation)
    }
    /// Caller must have completed the handshake and reaped any previous process.
    /// Only never-bound admitted work may acquire this process generation.
    pub fn activate_plugin(
        &mut self,
        id: &str,
        selection: u64,
        generation: u64,
    ) -> Result<(), PluginError> {
        if generation == 0 {
            return Err(PluginError::Incompatible);
        }
        let p = self
            .plugins
            .get_mut(id)
            .filter(|p| p.selection == selection)
            .ok_or(PluginError::Incompatible)?;
        if let Some(old) = p.generation {
            if old == generation {
                return if p.failed {
                    Err(PluginError::Incompatible)
                } else {
                    Ok(())
                };
            }
            if !p.failed {
                return Err(PluginError::InUse);
            }
        }
        p.generation = Some(generation);
        p.failed = false;
        self.installed.retain(|t| t.plugin() != id);
        self.installed.extend(p.manifest.actions.iter().map(|d| {
            ExternalType::new(id.to_owned(), d.id.clone()).expect("validated identifiers")
        }));
        for binding in self
            .plugin_runs
            .values_mut()
            .filter_map(|run| run.get_mut(id))
            .chain(
                self.plugin_jobs
                    .values_mut()
                    .filter_map(|(plugin, binding)| (plugin == id).then_some(binding)),
            )
        {
            if binding.selection == selection && binding.generation.is_none() {
                binding.generation = Some(generation);
            }
        }
        self.refresh_views();
        Ok(())
    }
    /// Detach the quiescent generation before requesting ordinary stop. The host
    /// still owns/reaps that process; new admissions bind only after a later wake.
    pub fn begin_plugin_stop(&mut self, id: &str, generation: u64) -> Result<(), PluginError> {
        self.deactivate_plugin(id, generation)
    }
    /// Detach a quiescent process; metadata stays admissible while dormant.
    /// Prefer begin_plugin_stop before signaling, so admissions during reap wait
    /// for a new process. This does not release the host's process ownership.
    pub fn deactivate_plugin(&mut self, id: &str, generation: u64) -> Result<(), PluginError> {
        if self
            .plugin_runs
            .values()
            .filter_map(|run| run.get(id))
            .any(|binding| binding.generation == Some(generation))
        {
            return Err(PluginError::InUse);
        }
        if self
            .plugin_jobs
            .values()
            .any(|(plugin, binding)| plugin == id && binding.generation == Some(generation))
        {
            return Err(PluginError::Residual);
        }
        let p = self
            .plugins
            .get_mut(id)
            .filter(|p| p.generation == Some(generation))
            .ok_or(PluginError::Incompatible)?;
        p.generation = None;
        p.failed = false;
        self.installed.retain(|t| t.plugin() != id);
        self.installed.extend(p.manifest.actions.iter().map(|d| {
            ExternalType::new(id.to_owned(), d.id.clone()).expect("validated identifiers")
        }));
        self.refresh_views();
        Ok(())
    }
    pub fn plugin_generation(&self, id: &str) -> Option<u64> {
        self.plugins.get(id)?.generation
    }
    pub fn plugin_available(&self, id: &str, selection: u64) -> bool {
        self.plugins
            .get(id)
            .is_some_and(|p| p.selection == selection && !p.failed)
    }
    pub fn fail_plugin_selection(&mut self, id: &str, selection: u64) -> Result<(), PluginError> {
        let p = self
            .plugins
            .get_mut(id)
            .filter(|p| p.selection == selection)
            .ok_or(PluginError::Incompatible)?;
        p.failed = true;
        self.installed.retain(|t| t.plugin() != id);
        self.refresh_views();
        Ok(())
    }
    /// Shared process demand: visible folder plus all admitted/future/residual work.
    pub fn plugin_demand(&self) -> std::collections::BTreeSet<String> {
        let mut demand = std::collections::BTreeSet::new();
        for (id, p) in &self.plugins {
            if p.manifest.lifecycle == rad_plugin_contract::PluginLifecycle::AlwaysLoaded {
                demand.insert(id.clone());
            }
        }
        let data = self.published.configuration().data();
        if let Some(folder) = data
            .folders
            .iter()
            .find(|f| f.binding.identity() == self.active)
        {
            for button in data
                .buttons
                .iter()
                .filter(|b| folder.slots.contains(&Some(b.binding.identity())))
            {
                for action in &button.actions {
                    if let ActionKind::External { type_key, .. } = &action.kind {
                        demand.insert(type_key.plugin().to_owned());
                    }
                }
                if let VisualSource::External { type_key, .. } = &button.visual {
                    demand.insert(type_key.plugin().to_owned());
                }
            }
        }
        demand.extend(
            self.plugin_runs
                .values()
                .flat_map(|run| run.keys().cloned()),
        );
        demand.extend(self.plugin_jobs.values().map(|(id, _)| id.clone()));
        demand.retain(|id| self.plugins.contains_key(id));
        demand
    }
    pub fn check_plugin_candidate(
        &self,
        manifest: &Manifest,
        target: &str,
    ) -> Result<(), PluginError> {
        manifest
            .validate(rad_plugin_contract::HOST_API, target)
            .map_err(|_| PluginError::Incompatible)?;
        let selected = self.plugin_activation && self.plugin_barrier.as_ref().is_some_and(|b|
            matches!(&b.change, PluginChange::Select { manifest: candidate, .. } if candidate == manifest));
        if !selected {
            self.plugin_change_allowed(&manifest.id)?;
        }
        if !self.plugins.contains_key(&manifest.id)
            && self.plugins.len() >= rad_plugin_contract::MAX_PLUGINS
        {
            return Err(PluginError::PluginLimit);
        }
        Ok(())
    }
    /// Guards a selection change only: an incompatible rollback cannot publish first.
    /// A version that is already selected registers anyway, and `plugin_selection`
    /// rejects just the buttons whose configuration it does not understand.
    fn check_published_configuration(&self, manifest: &Manifest) -> Result<(), PluginError> {
        for button in &self.published.configuration().data().buttons {
            for action in &button.actions {
                if let ActionKind::External {
                    type_key,
                    parameters,
                } = &action.kind
                    && type_key.plugin() == manifest.id
                {
                    validate(manifest, type_key, parameters, false)?;
                }
            }
            if let VisualSource::External {
                type_key,
                configuration,
            } = &button.visual
                && type_key.plugin() == manifest.id
            {
                validate(manifest, type_key, configuration, true)?;
            }
        }
        Ok(())
    }
    pub fn plugin_change_allowed(&self, id: &str) -> Result<(), PluginError> {
        if self
            .plugin_barrier
            .as_ref()
            .is_some_and(|b| b.change.id() == id)
        {
            return Err(PluginError::PublicationPending);
        }
        if self.plugin_runs.values().any(|p| p.contains_key(id)) {
            return Err(PluginError::InUse);
        }
        if self.plugin_jobs.values().any(|(p, _)| p == id) {
            return Err(PluginError::Residual);
        }
        if self.pending.is_some() || self.state != StorageState::Ready {
            return Err(PluginError::PublicationPending);
        }
        Ok(())
    }
    pub fn remove_plugin(&mut self, id: &str) -> Result<(), PluginError> {
        self.plugin_change_allowed(id)?;
        self.plugins.remove(id);
        self.installed.retain(|t| t.plugin() != id);
        self.refresh_views();
        Ok(())
    }
    pub fn plugin_failed(&mut self, id: &str, generation: u64) {
        if let Some(p) = self.plugins.get_mut(id)
            && p.generation == Some(generation)
        {
            p.failed = true;
            self.installed.retain(|t| t.plugin() != id);
            self.refresh_views();
        }
    }
    pub fn job_plugin(&self, id: DispatchId) -> Option<(&str, u64)> {
        self.plugin_jobs.get(&id).and_then(|(id, binding)| {
            binding
                .generation
                .map(|generation| (id.as_str(), generation))
        })
    }
    pub fn job_settings(&self, id: DispatchId) -> Option<Vec<u8>> {
        self.plugin_jobs
            .get(&id)
            .and_then(|(_, b)| b.settings.clone())
    }
    pub fn job_plugin_selection(&self, id: DispatchId) -> Option<(&str, u64)> {
        self.plugin_jobs
            .get(&id)
            .map(|(id, binding)| (id.as_str(), binding.selection))
    }
    pub(super) fn plugin_selection(
        &self,
        target: Target,
    ) -> Result<ExecutionSelection, ExecutionError> {
        let mut bindings = BTreeMap::new();
        let mut budgets = BTreeMap::new();
        let data = self.published.configuration().data();
        let button = data
            .buttons
            .iter()
            .find(|b| match target {
                Target::Button(id) => b.binding.identity() == id,
                Target::Action(id) => b.actions.iter().any(|a| a.binding.identity() == id),
            })
            .ok_or(ExecutionError::NotFound)?;
        for action in &button.actions {
            if let Target::Action(id) = target
                && action.binding.identity() != id
            {
                continue;
            }
            if let ActionKind::External {
                type_key,
                parameters,
            } = &action.kind
            {
                if self
                    .plugin_barrier
                    .as_ref()
                    .is_some_and(|b| b.change.id() == type_key.plugin())
                {
                    return Err(ExecutionError::ExecutorUnavailable);
                }
                let p = self
                    .plugins
                    .get(type_key.plugin())
                    .filter(|p| !p.failed)
                    .ok_or(ExecutionError::ExecutorUnavailable)?;
                if !p.manifest.credential_slots().is_empty() && p.credentials.is_none() {
                    return Err(ExecutionError::ExecutorUnavailable);
                }
                let definition = p
                    .manifest
                    .actions
                    .iter()
                    .find(|d| d.id == type_key.name())
                    .ok_or(ExecutionError::ExecutorUnavailable)?;
                let budget = Document::new(parameters)
                    .and_then(|d| d.response_budget_ms(definition))
                    .map_err(|_| ExecutionError::ExecutorUnavailable)?;
                budgets.insert(
                    action.binding.identity(),
                    rad_core::execution::ResponseBudget::new(budget)?,
                );
                bindings.insert(
                    type_key.plugin().into(),
                    ExecutionBinding {
                        selection: p.selection,
                        generation: p.generation,
                        credentials: p.credentials.clone(),
                        settings: data
                            .plugin_settings
                            .get(type_key.plugin())
                            .map(|s| s.bytes().to_vec()),
                    },
                );
            }
        }
        Ok(ExecutionSelection { bindings, budgets })
    }
    pub(super) fn validate_plugin_changes(&self, candidate: &Candidate) -> Result<(), PluginError> {
        let old = self.published.configuration().data();
        for (id, settings) in &candidate.configuration().data().plugin_settings {
            if old.plugin_settings.get(id) == Some(settings) {
                continue;
            }
            let schema = self
                .plugins
                .get(id)
                .and_then(|p| p.manifest.settings.as_ref())
                .ok_or(PluginError::Incompatible)?;
            let definition = rad_plugin_contract::Definition {
                id: "settings".into(),
                label: "Settings".into(),
                schema: schema.clone(),
                credential_slots: vec![],
                response_timeout: None,
                gestures: rad_plugin_contract::default_gestures(),
            };
            Document::new(settings.bytes())
                .and_then(|d| d.parameters(&definition))
                .map_err(|_| PluginError::Incompatible)?;
        }
        for b in &candidate.configuration().data().buttons {
            let previous = old.buttons.iter().find(|o| o.binding == b.binding);
            for a in &b.actions {
                if let ActionKind::External {
                    type_key,
                    parameters,
                } = &a.kind
                {
                    let unchanged = previous.is_some_and(|o| {
                        o.actions
                            .iter()
                            .any(|p| p.binding == a.binding && p.kind == a.kind)
                    });
                    if previous.is_some_and(|old| old.gesture != b.gesture)
                        || (!unchanged && b.gesture != rad_core::configuration::Gesture::Press)
                    {
                        let gesture = match b.gesture {
                            rad_core::configuration::Gesture::Press => {
                                rad_plugin_contract::Gesture::Press
                            }
                            rad_core::configuration::Gesture::Hold => {
                                rad_plugin_contract::Gesture::Hold
                            }
                            rad_core::configuration::Gesture::LongPress => {
                                rad_plugin_contract::Gesture::LongPress
                            }
                        };
                        let def = self
                            .plugins
                            .get(type_key.plugin())
                            .and_then(|p| {
                                p.manifest.actions.iter().find(|d| d.id == type_key.name())
                            })
                            .ok_or(PluginError::Incompatible)?;
                        if !def.gestures.contains(&gesture) {
                            return Err(PluginError::Incompatible);
                        }
                    }
                    if !unchanged {
                        let p = self
                            .plugins
                            .get(type_key.plugin())
                            .ok_or(PluginError::Incompatible)?;
                        validate(&p.manifest, type_key, parameters, false)?;
                    }
                }
            }
            if let VisualSource::External {
                type_key,
                configuration,
            } = &b.visual
                && previous.is_none_or(|p| p.visual != b.visual)
            {
                let p = self
                    .plugins
                    .get(type_key.plugin())
                    .ok_or(PluginError::Incompatible)?;
                validate(&p.manifest, type_key, configuration, true)?;
            }
        }
        Ok(())
    }
}
fn validate(
    manifest: &Manifest,
    key: &ExternalType,
    bytes: &[u8],
    visual: bool,
) -> Result<(), PluginError> {
    let defs = if visual {
        &manifest.visuals
    } else {
        &manifest.actions
    };
    let definition = defs
        .iter()
        .find(|d| d.id == key.name())
        .ok_or(PluginError::Incompatible)?;
    Document::new(bytes)
        .and_then(|d| d.parameters(definition))
        .map_err(|_| PluginError::Incompatible)
}
impl Authority {
    /// Internal catalog: stable selection identity (NOT process generation), and readiness.
    /// Dormant metadata remains available; adapters enforce consultation permission.
    pub fn plugin_manifests(&self) -> impl Iterator<Item = (&Manifest, u64, bool)> {
        self.plugins.values().map(|p| {
            (
                &p.manifest,
                p.selection,
                p.generation.is_some() && !p.failed,
            )
        })
    }
}

/// Administrative intent, validated in the Authority turn before durable I/O.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginChange {
    Select {
        operation: Selection,
        manifest: Manifest,
    },
    Uninstall {
        id: String,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Install,
    Update,
    Reinstall,
    Rollback,
}

impl PluginChange {
    pub fn id(&self) -> &str {
        match self {
            Self::Select { manifest, .. } => &manifest.id,
            Self::Uninstall { id } => id,
        }
    }
}
pub(super) struct Barrier {
    change: PluginChange,
    revision: i64,
}
/// Move to the worker and return unchanged. Dropping does NOT release the barrier.
pub struct PluginPermit(std::sync::Arc<Barrier>);
impl PluginPermit {
    pub fn change(&self) -> &PluginChange {
        &self.0.change
    }
    pub fn revision(&self) -> i64 {
        self.0.revision
    }
}
impl Authority {
    pub fn begin_plugin_change(
        &mut self,
        origin: Origin,
        change: PluginChange,
        target: &str,
    ) -> Result<PluginPermit, PluginError> {
        if origin != Origin::Editor {
            return Err(PluginError::PermissionDenied);
        }
        if self.plugin_barrier.is_some() || self.collection.is_some() {
            return Err(PluginError::PublicationPending);
        }
        match &change {
            PluginChange::Select { manifest, .. } => {
                self.check_plugin_candidate(manifest, target)?;
                self.check_published_configuration(manifest)?
            }
            PluginChange::Uninstall { id } => self.plugin_change_allowed(id)?,
        }
        let barrier = std::sync::Arc::new(Barrier {
            change,
            revision: self.revision,
        });
        self.plugin_barrier = Some(std::sync::Arc::clone(&barrier));
        Ok(PluginPermit(barrier))
    }
    /// Call only for a KNOWN durable result. Uncertain retains the permit and barrier.
    /// Applied retires logical executors; Host Ready must register the selected version later.
    pub fn finish_plugin_change(
        &mut self,
        permit: &mut Option<PluginPermit>,
        applied: bool,
    ) -> Result<(), PluginError> {
        let p = permit.as_ref().ok_or(PluginError::InvalidPermit)?;
        let barrier = self
            .plugin_barrier
            .as_ref()
            .ok_or(PluginError::InvalidPermit)?;
        if !std::sync::Arc::ptr_eq(barrier, &p.0) || barrier.revision != self.revision {
            return Err(PluginError::InvalidPermit);
        }
        if applied && !self.plugin_activation {
            let id = barrier.change.id();
            self.plugins.remove(id);
            self.installed.retain(|t| t.plugin() != id);
        }
        self.plugin_barrier = None;
        self.plugin_activation = false;
        permit.take();
        self.refresh_views();
        Ok(())
    }
}

impl Authority {
    /// Ordinary opaque configuration only, filtered through the entity's consultation.
    pub fn plugin_configuration(
        &self,
        origin: Origin,
        id: rad_core::identity::ShortId,
    ) -> Option<(&ExternalType, &[u8], bool)> {
        self.get(origin, id)?;
        for b in &self.published.configuration().data().buttons {
            if b.binding.short() == id
                && let VisualSource::External {
                    type_key,
                    configuration,
                } = &b.visual
            {
                return Some((type_key, configuration, true));
            }
            for a in &b.actions {
                if a.binding.short() == id
                    && let ActionKind::External {
                        type_key,
                        parameters,
                    } = &a.kind
                {
                    return Some((type_key, parameters, false));
                }
            }
        }
        None
    }
}

impl Authority {
    pub fn allow_plugin_activation(&mut self, permit: &PluginPermit) -> Result<(), PluginError> {
        let barrier = self
            .plugin_barrier
            .as_ref()
            .ok_or(PluginError::InvalidPermit)?;
        if !std::sync::Arc::ptr_eq(barrier, &permit.0) || barrier.revision != self.revision {
            return Err(PluginError::InvalidPermit);
        }
        // A KNOWN applied selector retires old logical types, while the barrier remains.
        let id = barrier.change.id();
        self.plugins.remove(id);
        self.installed.retain(|t| t.plugin() != id);
        self.plugin_activation = true;
        self.refresh_views();
        Ok(())
    }
}
