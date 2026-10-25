use super::*;

impl Configuration {
    pub fn prepare_edit(
        &self,
        registry: &IdentityRegistry,
        origin: Origin,
        edit: Edit,
    ) -> Result<Candidate, ConfigurationError> {
        self.prepare_edits(registry, origin, &[edit])
    }
    pub fn prepare_edits(
        &self,
        registry: &IdentityRegistry,
        origin: Origin,
        edits: &[Edit],
    ) -> Result<Candidate, ConfigurationError> {
        Self::from_data(self.data.clone(), registry)?;
        let mut data = self.data.clone();
        let mut registry = registry.clone();
        for edit in edits {
            if origin == Origin::Automation
                && matches!(
                    edit,
                    Edit::SetSettings(_)
                        | Edit::SetPluginSettings { .. }
                        | Edit::SetGesture { .. }
                        | Edit::RemoveAction { .. }
                        | Edit::ReplaceActionType { .. }
                        | Edit::DuplicateButton { .. }
                        | Edit::CreateButton { .. }
                        | Edit::CreateFolder { .. }
                        | Edit::AddAction { .. }
                        | Edit::DeleteButton { .. }
                        | Edit::DeleteFolder { .. }
                        | Edit::SetGlobalPermissions(_)
                        | Edit::SetButtonPermissions { .. }
                        | Edit::SetActionPermissions { .. }
                )
            {
                return Err(ConfigurationError::NotAllowed);
            }
            if origin == Origin::Automation {
                authorize_change(&data, edit)?;
            }
            match edit {
                Edit::SetGesture { button, gesture } => {
                    button_mut(&mut data, *button)?.gesture = *gesture;
                }
                Edit::SetPluginSettings { plugin, settings } => {
                    ExternalType::new(plugin.clone(), "settings".into())?;
                    if let Some(settings) = settings {
                        data.plugin_settings
                            .insert(plugin.clone(), settings.clone());
                    } else {
                        data.plugin_settings.remove(plugin);
                    }
                }
                Edit::SetSettings(settings) => {
                    Settings::from_data(*settings)?;
                    data.settings = *settings;
                }
                Edit::DuplicateButton {
                    button,
                    folder,
                    position,
                } => {
                    let mut duplicate = get_button(&data, *button)?.clone();
                    if is_navigation(&duplicate) {
                        return Err(ConfigurationError::ProtectedNavigation);
                    }
                    let slot = data
                        .folders
                        .iter_mut()
                        .find(|f| f.binding.identity() == *folder)
                        .ok_or(ConfigurationError::NotFound)?
                        .slots
                        .get_mut(*position)
                        .ok_or(ConfigurationError::InvalidPosition)?;
                    if slot.is_some() {
                        return Err(ConfigurationError::Occupied);
                    }
                    duplicate.binding = registry.allocate(EntityKind::Button)?;
                    for action in &mut duplicate.actions {
                        action.binding = registry.allocate(EntityKind::Action)?;
                    }
                    *slot = Some(duplicate.binding.identity());
                    data.buttons.push(duplicate);
                }
                Edit::RemoveAction { action } => {
                    let b = data
                        .buttons
                        .iter_mut()
                        .find(|b| b.actions.iter().any(|a| a.binding.identity() == *action))
                        .ok_or(ConfigurationError::NotFound)?;
                    if is_navigation(b) {
                        return Err(ConfigurationError::ProtectedNavigation);
                    }
                    b.actions.retain(|a| a.binding.identity() != *action);
                }
                Edit::ReplaceActionType { action, kind } => {
                    let a = action_mut(&mut data, *action)?;
                    if matches!(a.kind, ActionKind::Return | ActionKind::EnterFolder(_))
                        || matches!(kind, ActionKind::Return | ActionKind::EnterFolder(_))
                    {
                        return Err(ConfigurationError::ProtectedNavigation);
                    }
                    a.kind = kind.clone();
                }
                Edit::SetParameters { action, parameters } => {
                    let a = action_mut(&mut data, *action)?;
                    match (&mut a.kind, parameters) {
                        (ActionKind::Brightness(current), ActionParameters::Brightness(next)) => {
                            *current = *next
                        }
                        (ActionKind::Launcher(current), ActionParameters::Launcher(next)) => {
                            *current = next.clone()
                        }
                        (
                            ActionKind::External {
                                parameters: current,
                                ..
                            },
                            ActionParameters::External(next),
                        ) => *current = next.clone(),
                        (ActionKind::Wait(current), ActionParameters::Wait(next)) => {
                            *current = *next
                        }
                        _ => return Err(ConfigurationError::InvalidActions),
                    }
                }
                Edit::SetName { button, name } => {
                    button_mut(&mut data, *button)?.name = name.clone()
                }
                Edit::SetAppearance { button, appearance } => {
                    button_mut(&mut data, *button)?.appearance = appearance.clone()
                }
                Edit::SetVisual { button, visual } => {
                    button_mut(&mut data, *button)?.visual = visual.clone()
                }
                Edit::SetPlan {
                    button,
                    mode,
                    order,
                } => {
                    let b = button_mut(&mut data, *button)?;
                    let unique: std::collections::BTreeSet<_> = order.iter().copied().collect();
                    if order.len() != b.actions.len() || unique.len() != order.len() {
                        return Err(ConfigurationError::InvalidActions);
                    }
                    let actions = order
                        .iter()
                        .map(|id| {
                            b.actions
                                .iter()
                                .find(|a| a.binding.identity() == *id)
                                .cloned()
                                .ok_or(ConfigurationError::InvalidActions)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    b.actions = actions;
                    b.mode = *mode;
                }
                Edit::SetGlobalPermissions(permissions) => data.global_permissions = *permissions,
                Edit::SetButtonPermissions {
                    button,
                    permissions,
                } => button_mut(&mut data, *button)?.permissions = *permissions,
                Edit::SetActionPermissions {
                    action,
                    permissions,
                } => action_mut(&mut data, *action)?.permissions = *permissions,
                Edit::DeleteFolder {
                    folder,
                    confirm_contents,
                } => {
                    if *folder == data.principal {
                        return Err(ConfigurationError::ProtectedNavigation);
                    }
                    if !confirm_contents {
                        return Err(ConfigurationError::ConfirmationRequired);
                    }
                    if !data.folders.iter().any(|f| f.binding.identity() == *folder) {
                        return Err(ConfigurationError::NotFound);
                    }
                    let mut removed = std::collections::BTreeSet::from([*folder]);
                    loop {
                        let before = removed.len();
                        for f in &data.folders {
                            if f.parent.is_some_and(|p| removed.contains(&p)) {
                                removed.insert(f.binding.identity());
                            }
                        }
                        if before == removed.len() {
                            break;
                        }
                    }
                    let buttons: std::collections::BTreeSet<_> = data
                        .folders
                        .iter()
                        .filter(|f| removed.contains(&f.binding.identity()))
                        .flat_map(|f| f.slots.iter().flatten().copied())
                        .chain(
                            data.buttons
                                .iter()
                                .filter(|b| {
                                    b.actions
                                        .iter()
                                        .any(|a| a.kind == ActionKind::EnterFolder(*folder))
                                })
                                .map(|b| b.binding.identity()),
                        )
                        .collect();
                    data.folders
                        .retain(|f| !removed.contains(&f.binding.identity()));
                    data.buttons
                        .retain(|b| !buttons.contains(&b.binding.identity()));
                    for f in &mut data.folders {
                        for slot in &mut f.slots {
                            if slot.is_some_and(|b| buttons.contains(&b)) {
                                *slot = None;
                            }
                        }
                    }
                }
                Edit::DeleteButton { button } => {
                    let index = data
                        .buttons
                        .iter()
                        .position(|b| b.binding.identity() == *button)
                        .ok_or(ConfigurationError::NotFound)?;
                    if is_navigation(&data.buttons[index]) {
                        return Err(ConfigurationError::ProtectedNavigation);
                    }
                    let (f, p) = locate(&data, *button)?;
                    data.folders[f].slots[p] = None;
                    data.buttons.remove(index);
                }
                Edit::MoveButton {
                    button,
                    folder,
                    position,
                } => {
                    let (source, source_position) = locate(&data, *button)?;
                    let destination = data
                        .folders
                        .iter()
                        .position(|f| f.binding.identity() == *folder)
                        .ok_or(ConfigurationError::NotFound)?;
                    let displaced = *data.folders[destination]
                        .slots
                        .get(*position)
                        .ok_or(ConfigurationError::InvalidPosition)?;
                    if source != destination
                        && [Some(*button), displaced].into_iter().flatten().any(|id| {
                            data.buttons.iter().any(|b| {
                                b.binding.identity() == id
                                    && b.actions.iter().any(|a| a.kind == ActionKind::Return)
                            })
                        })
                    {
                        return Err(ConfigurationError::ProtectedNavigation);
                    }
                    data.folders[source].slots[source_position] = displaced;
                    data.folders[destination].slots[*position] = Some(*button);
                    for id in [Some(*button), displaced].into_iter().flatten() {
                        let b = data
                            .buttons
                            .iter()
                            .find(|b| b.binding.identity() == id)
                            .ok_or(ConfigurationError::NotFound)?;
                        if let Some(ActionData {
                            kind: ActionKind::EnterFolder(child),
                            ..
                        }) = b.actions.first()
                        {
                            let (owner, _) = locate(&data, id)?;
                            let parent = data.folders[owner].binding.identity();
                            let child = *child;
                            data.folders
                                .iter_mut()
                                .find(|f| f.binding.identity() == child)
                                .ok_or(ConfigurationError::NotFound)?
                                .parent = Some(parent);
                        }
                    }
                }
                Edit::AddAction { button, kind } => {
                    let b = data
                        .buttons
                        .iter_mut()
                        .find(|b| b.binding.identity() == *button)
                        .ok_or(ConfigurationError::NotFound)?;
                    if b.actions.len() >= MAX_ACTIONS {
                        return Err(ConfigurationError::InvalidActions);
                    }
                    let binding = registry.allocate(EntityKind::Action)?;
                    b.actions.push(ActionData {
                        binding,
                        kind: kind.clone(),
                        permissions: LocalPermissions::default(),
                    });
                }
                Edit::CreateFolder {
                    parent,
                    position,
                    name,
                } => {
                    let folder_index = data
                        .folders
                        .iter()
                        .position(|f| f.binding.identity() == *parent)
                        .ok_or(ConfigurationError::NotFound)?;
                    let slot = data.folders[folder_index]
                        .slots
                        .get(*position)
                        .ok_or(ConfigurationError::InvalidPosition)?;
                    if slot.is_some() {
                        return Err(ConfigurationError::Occupied);
                    }
                    let folder = registry.allocate(EntityKind::Folder)?;
                    let entry = registry.allocate(EntityKind::Button)?;
                    let enter_action = registry.allocate(EntityKind::Action)?;
                    let back = registry.allocate(EntityKind::Button)?;
                    let back_action = registry.allocate(EntityKind::Action)?;
                    data.folders[folder_index].slots[*position] = Some(entry.identity());
                    let mut slots = vec![None; data.grid.key_count()];
                    slots[0] = Some(back.identity());
                    data.folders.push(FolderData {
                        binding: folder,
                        parent: Some(*parent),
                        slots,
                    });
                    let mut entry_button = ButtonData::empty(entry, name.clone());
                    entry_button.actions.push(ActionData {
                        binding: enter_action,
                        kind: ActionKind::EnterFolder(folder.identity()),
                        permissions: LocalPermissions::default(),
                    });
                    let mut back_button = ButtonData::empty(back, "Return".into());
                    back_button.actions.push(ActionData {
                        binding: back_action,
                        kind: ActionKind::Return,
                        permissions: LocalPermissions::default(),
                    });
                    data.buttons.extend([entry_button, back_button]);
                }
                Edit::CreateButton {
                    folder,
                    position,
                    name,
                } => {
                    let slot = data
                        .folders
                        .iter_mut()
                        .find(|f| f.binding.identity() == *folder)
                        .ok_or(ConfigurationError::NotFound)?
                        .slots
                        .get_mut(*position)
                        .ok_or(ConfigurationError::InvalidPosition)?;
                    if slot.is_some() {
                        return Err(ConfigurationError::Occupied);
                    }
                    let binding = registry.allocate(EntityKind::Button)?;
                    *slot = Some(binding.identity());
                    data.buttons.push(ButtonData::empty(binding, name.clone()));
                }
            }
        }
        if data.entity_count() > MAX_LIVE_IDENTITIES
            && data.entity_count() > self.data.entity_count()
        {
            return Err(ConfigurationError::ResourceExhausted);
        }
        let active = active_bindings(&data);
        let registry = registry.restore(&active)?;
        Ok(Candidate {
            configuration: Box::new(Self::from_data(data, &registry)?),
            registry,
            root_adoption: None,
        })
    }
}

impl ButtonData {
    fn empty(binding: Binding, name: String) -> Self {
        Self {
            binding,
            name,
            appearance: Appearance::default(),
            visual: VisualSource::Manual,
            permissions: LocalPermissions::default(),
            mode: Mode::default(),
            gesture: Gesture::Press,
            actions: vec![],
        }
    }
}

fn locate(
    data: &ConfigurationData,
    button: EntityIdentity,
) -> Result<(usize, usize), ConfigurationError> {
    data.folders
        .iter()
        .enumerate()
        .find_map(|(i, f)| {
            f.slots
                .iter()
                .position(|id| *id == Some(button))
                .map(|p| (i, p))
        })
        .ok_or(ConfigurationError::NotFound)
}

fn is_navigation(button: &ButtonData) -> bool {
    button
        .actions
        .iter()
        .any(|a| matches!(a.kind, ActionKind::EnterFolder(_) | ActionKind::Return))
}

pub(super) fn active_bindings(data: &ConfigurationData) -> Vec<Binding> {
    data.folders
        .iter()
        .map(|f| f.binding)
        .chain(
            data.buttons.iter().flat_map(|b| {
                std::iter::once(b.binding).chain(b.actions.iter().map(|a| a.binding))
            }),
        )
        .collect()
}

fn button_mut(
    data: &mut ConfigurationData,
    id: EntityIdentity,
) -> Result<&mut ButtonData, ConfigurationError> {
    data.buttons
        .iter_mut()
        .find(|b| b.binding.identity() == id)
        .ok_or(ConfigurationError::NotFound)
}

fn action_mut(
    data: &mut ConfigurationData,
    id: EntityIdentity,
) -> Result<&mut ActionData, ConfigurationError> {
    data.buttons
        .iter_mut()
        .flat_map(|b| b.actions.iter_mut())
        .find(|a| a.binding.identity() == id)
        .ok_or(ConfigurationError::NotFound)
}

fn authorize_change(data: &ConfigurationData, edit: &Edit) -> Result<(), ConfigurationError> {
    let button_change = match edit {
        Edit::MoveButton {
            button,
            folder,
            position,
        } => {
            let (source, old_position) = locate(data, *button)?;
            if data.folders[source].binding.identity() != *folder {
                return Err(ConfigurationError::NotAllowed);
            }
            let displaced = *data.folders[source]
                .slots
                .get(*position)
                .ok_or(ConfigurationError::InvalidPosition)?;
            if old_position != *position {
                for id in [Some(*button), displaced].into_iter().flatten() {
                    if !get_button(data, id)?
                        .permissions
                        .resolve(data.global_permissions)
                        .modify()
                    {
                        return Err(ConfigurationError::PermissionDenied);
                    }
                }
            }
            None
        }
        Edit::SetName { button, name } => Some((*button, get_button(data, *button)?.name != *name)),
        Edit::SetAppearance { button, appearance } => Some((
            *button,
            get_button(data, *button)?.appearance != *appearance,
        )),
        Edit::SetVisual { button, visual } => {
            Some((*button, get_button(data, *button)?.visual != *visual))
        }
        Edit::SetPlan {
            button,
            mode,
            order,
        } => {
            let b = get_button(data, *button)?;
            Some((
                *button,
                b.mode != *mode
                    || b.actions
                        .iter()
                        .map(|a| a.binding.identity())
                        .collect::<Vec<_>>()
                        != *order,
            ))
        }
        Edit::SetParameters { action, parameters } => {
            let (b, a) = data
                .buttons
                .iter()
                .find_map(|b| {
                    b.actions
                        .iter()
                        .find(|a| a.binding.identity() == *action)
                        .map(|a| (b, a))
                })
                .ok_or(ConfigurationError::NotFound)?;
            let changed = match (&a.kind, parameters) {
                (ActionKind::Brightness(old), ActionParameters::Brightness(new)) => old != new,
                (ActionKind::Launcher(old), ActionParameters::Launcher(new)) => old != new,
                (
                    ActionKind::External {
                        parameters: old, ..
                    },
                    ActionParameters::External(new),
                ) => old != new,
                (ActionKind::Wait(old), ActionParameters::Wait(new)) => old != new,
                _ => return Err(ConfigurationError::InvalidActions),
            };
            if changed
                && !a
                    .permissions
                    .resolve(b.permissions.resolve(data.global_permissions))
                    .modify()
            {
                return Err(ConfigurationError::PermissionDenied);
            }
            None
        }
        _ => None,
    };
    if let Some((id, true)) = button_change
        && !get_button(data, id)?
            .permissions
            .resolve(data.global_permissions)
            .modify()
    {
        return Err(ConfigurationError::PermissionDenied);
    }
    Ok(())
}

fn get_button(
    data: &ConfigurationData,
    id: EntityIdentity,
) -> Result<&ButtonData, ConfigurationError> {
    data.buttons
        .iter()
        .find(|b| b.binding.identity() == id)
        .ok_or(ConfigurationError::NotFound)
}

impl Configuration {
    /// Human-confirmed replacement; the complete registry is never rolled back.
    pub fn prepare_restore(
        &self,
        registry: &IdentityRegistry,
        origin: Origin,
        confirmed: bool,
        data: ConfigurationData,
    ) -> Result<Candidate, ConfigurationError> {
        if origin != Origin::Editor {
            return Err(ConfigurationError::NotAllowed);
        }
        if !confirmed {
            return Err(ConfigurationError::ConfirmationRequired);
        }
        validate_data(&self.data, registry)?;
        let registry = registry.restore(&active_bindings(&data))?;
        Ok(Candidate {
            configuration: Box::new(Self::from_data(data, &registry)?),
            registry,
            root_adoption: None,
        })
    }
}
