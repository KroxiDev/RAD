use super::*;
use crate::identity::{IdentityCounters, IdentityRecord};
use std::collections::BTreeMap;

impl Configuration {
    /// Prepare a portable replacement without mutating either source or destination.
    /// Counters are non-revisionable package metadata, not part of the undo snapshot.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_portable_restore(
        &self,
        registry: &IdentityRegistry,
        origin: Origin,
        confirmed: bool,
        mut data: ConfigurationData,
        source_installation: u128,
        counters: IdentityCounters,
    ) -> Result<PortableRestore, ConfigurationError> {
        if origin != Origin::Editor {
            return Err(ConfigurationError::NotAllowed);
        }
        if !confirmed {
            return Err(ConfigurationError::ConfirmationRequired);
        }
        validate_data(&self.data, registry)?;
        let bindings = edits::active_bindings(&data);
        let source = IdentityRegistry::from_records_with_counters(
            source_installation,
            bindings.iter().map(|b| IdentityRecord::new(*b, false)),
            counters,
        )?;
        validate_data(&data, &source)?;
        if source_installation == registry.installation() {
            let mut reserved = registry.clone();
            reserved.reserve_counters(counters);
            // Adoption displaced the installation's original empty root. Even after
            // its foreign replacement is pruned, f1 must not acquire another owner.
            if is_initial_root(&data, source_installation, counters)?
                && !registry
                    .records()
                    .any(|record| record.binding() == data.folders[0].binding)
            {
                let replacement = reserved.allocate(EntityKind::Folder)?;
                data.principal = replacement.identity();
                data.folders[0].binding = replacement;
            }
            let registry = reserved.restore(&edits::active_bindings(&data))?;
            return Ok(PortableRestore {
                candidate: Candidate {
                    configuration: Box::new(Self::from_data(data, &registry)?),
                    registry,
                    root_adoption: None,
                },
                history_base: None,
            });
        }
        let pristine = registry.counters()
            == IdentityCounters {
                folders: 1,
                buttons: 0,
                actions: 0,
                serial: 1,
            }
            && registry.records().count() == 1
            && self.data.folders.len() == 1
            && self.data.buttons.is_empty()
            && self.data.folders[0].binding.short().number() == 1
            && self.data.principal == EntityIdentity::new(registry.installation(), 1)?;
        let mut next = if pristine {
            IdentityRegistry::new(registry.installation())?
        } else {
            registry.clone()
        };
        next.reserve_counters(registry.counters());
        next.reserve_counters(counters);
        // A portable document can contain identities adopted from this destination earlier.
        // Its local serial is not necessarily covered by the source installation's counter.
        let local_serial = bindings
            .iter()
            .filter(|b| b.identity().installation() == registry.installation())
            .map(|b| b.identity().serial())
            .max()
            .unwrap_or(0);
        next.reserve_counters(IdentityCounters {
            serial: local_serial,
            ..IdentityCounters::default()
        });
        let mut history_base = None;
        let mut root_adoption = None;
        if pristine {
            let previous = self.data.folders[0].binding;
            let replacement = next.allocate(EntityKind::Folder)?;
            let mut previous_data = self.data.clone();
            previous_data.principal = replacement.identity();
            previous_data.folders[0].binding = replacement;
            history_base = Some(previous_data);
            root_adoption = Some(RootAdoption {
                previous,
                replacement,
            });
        } else {
            let mut mapping = BTreeMap::new();
            for binding in &bindings {
                mapping.insert(binding.identity(), next.allocate(binding.short().kind())?);
            }
            remap(&mut data, &mapping)?;
        }
        let next = next.restore(&edits::active_bindings(&data))?;
        let configuration = Self::from_data(data, &next)?;
        Ok(PortableRestore {
            candidate: Candidate {
                configuration: Box::new(configuration),
                registry: next,
                root_adoption,
            },
            history_base,
        })
    }
}
fn remap(
    data: &mut ConfigurationData,
    mapping: &BTreeMap<EntityIdentity, Binding>,
) -> Result<(), ConfigurationError> {
    let binding = |id: &EntityIdentity| {
        mapping
            .get(id)
            .copied()
            .ok_or(ConfigurationError::RegistryMismatch)
    };
    data.principal = binding(&data.principal)?.identity();
    for f in &mut data.folders {
        f.binding = binding(&f.binding.identity())?;
        f.parent = f
            .parent
            .map(|id| binding(&id).map(|b| b.identity()))
            .transpose()?;
        for slot in f.slots.iter_mut().flatten() {
            *slot = binding(slot)?.identity();
        }
    }
    for b in &mut data.buttons {
        b.binding = binding(&b.binding.identity())?;
        for a in &mut b.actions {
            a.binding = binding(&a.binding.identity())?;
            if let ActionKind::EnterFolder(id) = &mut a.kind {
                *id = binding(id)?.identity();
            }
        }
    }
    Ok(())
}

fn is_initial_root(
    data: &ConfigurationData,
    installation: u128,
    counters: IdentityCounters,
) -> Result<bool, ConfigurationError> {
    Ok(counters
        == IdentityCounters {
            folders: 1,
            buttons: 0,
            actions: 0,
            serial: 1,
        }
        && data.folders.len() == 1
        && data.buttons.is_empty()
        && data.folders[0].binding.short().number() == 1
        && data.folders[0].binding.identity() == EntityIdentity::new(installation, 1)?
        && data.folders[0].parent.is_none()
        && data.folders[0].slots.iter().all(Option::is_none))
}
