use super::*;

pub(super) fn validate_data(
    data: &ConfigurationData,
    registry: &IdentityRegistry,
) -> Result<(), ConfigurationError> {
    Settings::from_data(data.settings)?;
    let principal = data
        .folders
        .iter()
        .find(|f| f.binding.identity() == data.principal)
        .ok_or(ConfigurationError::InvalidTree)?;
    if principal.parent.is_some() {
        return Err(ConfigurationError::InvalidTree);
    }
    let mut bindings = Vec::new();
    for folder in &data.folders {
        if folder.slots.len() != 32 || folder.binding.short().kind() != EntityKind::Folder {
            return Err(ConfigurationError::InvalidTree);
        }
        bindings.push(folder.binding);
        let mut current = folder;
        let mut visited = std::collections::BTreeSet::new();
        while let Some(parent) = current.parent {
            if !visited.insert(current.binding.identity()) {
                return Err(ConfigurationError::InvalidTree);
            }
            current = data
                .folders
                .iter()
                .find(|f| f.binding.identity() == parent)
                .ok_or(ConfigurationError::InvalidTree)?;
        }
        if current.binding.identity() != data.principal {
            return Err(ConfigurationError::InvalidTree);
        }
        let entries = data
            .buttons
            .iter()
            .filter(|b| {
                b.actions
                    .iter()
                    .any(|a| a.kind == ActionKind::EnterFolder(folder.binding.identity()))
            })
            .count();
        let returns = data
            .buttons
            .iter()
            .filter(|b| {
                folder.slots.contains(&Some(b.binding.identity()))
                    && b.actions.iter().any(|a| a.kind == ActionKind::Return)
            })
            .count();
        if (folder.parent.is_some() && (entries != 1 || returns != 1))
            || (folder.parent.is_none() && (entries != 0 || returns != 0))
        {
            return Err(ConfigurationError::InvalidTree);
        }
    }
    let occupied: Vec<_> = data
        .folders
        .iter()
        .flat_map(|f| f.slots.iter().flatten().copied())
        .collect();
    if occupied.len() != data.buttons.len() {
        return Err(ConfigurationError::InvalidTree);
    }
    for button in &data.buttons {
        if button.binding.short().kind() != EntityKind::Button
            || occupied
                .iter()
                .filter(|id| **id == button.binding.identity())
                .count()
                != 1
        {
            return Err(ConfigurationError::InvalidTree);
        }
        bindings.push(button.binding);
        if button.actions.len() > MAX_ACTIONS {
            return Err(ConfigurationError::InvalidActions);
        }
        for action in &button.actions {
            if action.binding.short().kind() != EntityKind::Action {
                return Err(ConfigurationError::InvalidActions);
            }
            bindings.push(action.binding);
            match action.kind {
                ActionKind::Launcher(_) | ActionKind::External { .. } | ActionKind::Wait(_) => {}
                ActionKind::EnterFolder(target) => {
                    let owner = data
                        .folders
                        .iter()
                        .find(|f| f.slots.contains(&Some(button.binding.identity())))
                        .ok_or(ConfigurationError::InvalidTree)?;
                    if button.actions.len() != 1
                        || !data.folders.iter().any(|f| {
                            f.binding.identity() == target
                                && f.parent == Some(owner.binding.identity())
                        })
                    {
                        return Err(ConfigurationError::InvalidTree);
                    }
                }
                ActionKind::Return => {
                    if button.actions.len() != 1 {
                        return Err(ConfigurationError::InvalidActions);
                    }
                }
            }
        }
    }
    let mut identities = std::collections::BTreeSet::new();
    let mut shorts = std::collections::BTreeSet::new();
    if bindings
        .iter()
        .any(|b| !identities.insert(b.identity()) || !shorts.insert(b.short()))
    {
        return Err(ConfigurationError::InvalidTree);
    }
    let active: Vec<_> = registry
        .records()
        .filter(|r| !r.retired())
        .map(|r| r.binding())
        .collect();
    if active.len() != bindings.len() || bindings.iter().any(|b| !active.contains(b)) {
        return Err(ConfigurationError::RegistryMismatch);
    }
    Ok(())
}
