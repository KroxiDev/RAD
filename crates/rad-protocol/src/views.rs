//! Atomic projections: no I/O or authority mutation between component reads.
use super::*;
use rad_api::{
    AppearanceView, BoardView, EntityFields, EntityView, Grid, InspectorView, KeyView, Slot,
};
use rad_authority::Visible;

fn revision(a: &Authority, c: IncomingContext) -> Checked<String> {
    a.view_revision(c.origin)
        .map(|r| r.value().to_string())
        .ok_or("resource_exhausted")
}
fn permission(p: rad_core::permissions::Permissions) -> rad_api::Permissions {
    rad_api::Permissions {
        consult: p.consult(),
        modify: p.modify(),
        execute: p.execute(),
    }
}
fn local(p: rad_core::permissions::LocalPermissions) -> rad_api::LocalPermissions {
    match p.own_value() {
        Some(p) => rad_api::LocalPermissions::Own(permission(p)),
        None => rad_api::LocalPermissions::Inherit(true),
    }
}
fn entity(a: &Authority, c: IncomingContext, id: ShortId, rev: &str) -> Checked<EntityView> {
    let (entity, sections) = if id.kind() == EntityKind::Folder {
        let f = a.folder(c.origin, id).ok_or("not_found")?;
        (
            EntityFields::Folder(rad_api::FolderFields {
                name: f.name,
                entry_button_id: f.entry_button.map(|b| short(b.short())),
                parent_id: f.parent_binding.map(|b| short(b.short())),
                slots: f
                    .slots
                    .into_iter()
                    .map(|s| {
                        s.map(|b| match b {
                            Visible::Restricted => Slot::Restricted {},
                            Visible::Visible(b) => Slot::Visible {
                                id: short(b.short()),
                            },
                        })
                    })
                    .collect(),
            }),
            vec![],
        )
    } else {
        match a.get(c.origin, id).ok_or("not_found")? {
            rad_authority::EntityView::Button(b) => (
                EntityFields::Button(rad_api::ButtonFields {
                    gesture: match b.gesture {
                        rad_core::configuration::Gesture::Press => rad_api::Gesture::Press,
                        rad_core::configuration::Gesture::Hold => rad_api::Gesture::Hold,
                        rad_core::configuration::Gesture::LongPress => rad_api::Gesture::LongPress,
                    },
                    name: b.name,
                    mode: match b.mode {
                        rad_core::configuration::Mode::Sequential => rad_api::Mode::Sequential,
                        rad_core::configuration::Mode::Parallel => rad_api::Mode::Parallel,
                    },
                    permissions: local(b.local_permissions),
                    effective_permissions: permission(b.permissions),
                    folder_id: b.folder.map(|b| short(b.short())),
                    position: Some(b.position),
                }),
                vec![rad_api::Section::Appearance, rad_api::Section::Actions],
            ),
            rad_authority::EntityView::Action(v) => (action_fields(v), vec![]),
        }
    };
    Ok(EntityView {
        visibility: rad_api::Visibility::Visible,
        id: short(id),
        entity,
        view_revision: rev.into(),
        sections,
    })
}
fn action_fields(v: rad_authority::ActionView) -> EntityFields {
    use rad_authority::ActionType;
    let type_key = match &v.kind {
        ActionType::Launcher => "launcher".into(),
        ActionType::Wait => "wait".into(),
        ActionType::EnterFolder => "enter_folder".into(),
        ActionType::Return => "return".into(),
        ActionType::Sleep => "sleep".into(),
        ActionType::Brightness(_) => "brightness".into(),
        ActionType::External(key) => format!("{}:{}", key.plugin(), key.name()),
    };
    let parameters = if let ActionType::Brightness(direction) = v.kind {
        Some(rad_api::ActionParameters::Brightness(
            rad_api::BrightnessParameters {
                direction: match direction {
                    rad_core::configuration::BrightnessDirection::Increase => {
                        rad_api::BrightnessDirection::Increase
                    }
                    rad_core::configuration::BrightnessDirection::Decrease => {
                        rad_api::BrightnessDirection::Decrease
                    }
                },
            },
        ))
    } else {
        match (v.desktop_entry, v.wait_ms) {
            (Some(e), _) => Some(rad_api::ActionParameters::Launcher(
                rad_api::LauncherParameters {
                    desktop_entry: e.id().into(),
                },
            )),
            (_, Some(milliseconds)) => {
                Some(rad_api::ActionParameters::Wait(rad_api::WaitParameters {
                    milliseconds,
                }))
            }
            _ => None,
        }
    };
    EntityFields::Action(rad_api::ActionFields {
        type_key,
        permissions: local(v.local_permissions),
        effective_permissions: permission(v.permissions),
        parent_id: v.parent_binding.map(|b| short(b.short())),
        parameters,
        folder_id: v.destination.map(|b| short(b.short())),
    })
}
pub(super) fn appearance(
    a: &Authority,
    c: IncomingContext,
    id: ShortId,
) -> Checked<AppearanceView> {
    use rad_core::configuration::TextPosition as P;
    let v = a.appearance(c.origin, id).ok_or("not_found")?;
    let fallback = match a.get(c.origin, id) {
        Some(rad_authority::EntityView::Button(button)) => {
            if button.actions.iter().any(|action|matches!(action,Visible::Visible(a) if a.kind==rad_authority::ActionType::Sleep)) {Some(rad_api::NativeIcon::Sleep)}
            else if button.actions.iter().any(|action|matches!(action,Visible::Visible(a) if matches!(a.kind,rad_authority::ActionType::Brightness(_)))) {Some(rad_api::NativeIcon::Brightness)}
            else {None}
        }
        _ => None,
    };
    let native_icon =
        if v.appearance.image.is_none() && matches!(v.visual, rad_authority::VisualView::Static) {
            fallback
        } else {
            None
        };
    let position = if native_icon.is_some() {
        v.appearance.text_position.unwrap_or(P::Bottom)
    } else {
        v.appearance.effective_text_position()
    };
    let text_position = match position {
        P::Top => rad_api::TextPosition::Top,
        P::Middle => rad_api::TextPosition::Middle,
        P::Bottom => rad_api::TextPosition::Bottom,
        P::AboveImage => rad_api::TextPosition::AboveImage,
        P::BelowImage => rad_api::TextPosition::BelowImage,
    };
    let visual_type = match v.visual {
        rad_authority::VisualView::Static => None,
        rad_authority::VisualView::External(key) => {
            Some(format!("{}#{}", key.plugin(), key.name()))
        }
    };
    Ok(AppearanceView {
        text: v.appearance.text,
        image: v.appearance.image,
        text_position,
        visual_type,
        native_icon,
    })
}
pub(super) fn board(
    a: &Authority,
    c: IncomingContext,
    folder_id: Option<&str>,
) -> Checked<BoardView> {
    if c.origin != Origin::Editor {
        return Err("permission_denied");
    }
    let status = a.status(c.origin);
    let id = match folder_id {
        Some(id) => parse_id(id)?,
        None => status.active.ok_or("not_found")?.short(),
    };
    let rev = revision(a, c)?;
    let folder = entity(a, c, id, &rev)?;
    let EntityFields::Folder(fields) = &folder.entity else {
        return Err("invalid_request");
    };
    let mut keys = Vec::with_capacity(fields.slots.len());
    for slot in &fields.slots {
        keys.push(match slot {
            None => None,
            Some(Slot::Restricted {}) => return Err("permission_denied"),
            Some(Slot::Visible { id }) => {
                let id = parse_id(id)?;
                Some(KeyView {
                    entity: entity(a, c, id, &rev)?,
                    appearance: appearance(a, c, id)?,
                    visual: None,
                })
            }
        });
    }
    Ok(BoardView {
        view_revision: rev,
        grid: Grid {
            columns: status.grid.columns().into(),
            rows: status.grid.rows().into(),
        },
        folder,
        keys,
    })
}
pub(super) fn inspector(a: &Authority, c: IncomingContext, id: &str) -> Checked<InspectorView> {
    if c.origin != Origin::Editor {
        return Err("permission_denied");
    }
    let id = parse_id(id)?;
    let rev = revision(a, c)?;
    let entity = entity(a, c, id, &rev)?;
    let mut appearance_view = None;
    let mut actions = Vec::new();
    if let Some(rad_authority::EntityView::Button(button)) = a.get(c.origin, id) {
        appearance_view = Some(appearance(a, c, id)?);
        actions = button
            .actions
            .into_iter()
            .map(|action| match action {
                Visible::Restricted => rad_api::ActionView::Restricted {},
                Visible::Visible(v) => rad_api::ActionView::Visible {
                    id: short(v.binding.short()),
                    entity: action_fields(v),
                    view_revision: rev.clone(),
                    sections: vec![],
                },
            })
            .collect();
    }
    let plugin = a
        .plugin_configuration(c.origin, id)
        .map(|(key, bytes, visual)| {
            Ok(rad_api::PluginConfiguration {
                type_key: format!(
                    "{}{}{}",
                    key.plugin(),
                    if visual { '#' } else { ':' },
                    key.name()
                ),
                parameters: serde_json::from_slice(bytes).map_err(|_| "invalid_configuration")?,
            })
        })
        .transpose()?;
    Ok(InspectorView {
        view_revision: rev,
        entity,
        appearance: appearance_view,
        actions,
        plugin,
    })
}
impl Protocol {
    pub fn board_view(
        &self,
        a: &Authority,
        c: IncomingContext,
        folder_id: Option<&str>,
    ) -> Checked<BoardView> {
        board(a, c, folder_id)
    }
    pub fn inspector_view(
        &self,
        a: &Authority,
        c: IncomingContext,
        id: &str,
    ) -> Checked<InspectorView> {
        inspector(a, c, id)
    }
}
