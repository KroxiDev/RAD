use super::*;
use crate::edits::{button, position, resolve};
use rad_core::{
    configuration::{ActionKind, ConfigurationError, Edit},
    permissions::LocalPermissions,
};
pub(super) fn command(request: &rad_api::Request) -> Checked<Command> {
    use rad_api::Request as R;
    match request {
        R::Undo {} => Ok(Command::Undo),
        R::Redo {} => Ok(Command::Redo),
        R::Duplicate {
            id,
            folder_id: folder,
            position: pos,
        }
        | R::MoveFolder {
            id,
            parent_id: folder,
            position: pos,
        } => {
            let target = parse_id(id)?;
            let folder = parse_id(folder)?;
            let position = position(*pos)?;
            let duplicate = matches!(request, R::Duplicate { .. });
            Ok(Command::PrepareEdits(vec![Box::new(move |cfg| {
                let folder = resolve(cfg, folder)?;
                let edit = if duplicate {
                    Edit::DuplicateButton {
                        button: button(cfg, target)?.binding.identity(),
                        folder,
                        position,
                    }
                } else {
                    let f = resolve(cfg, target)?;
                    let entry = cfg
                        .data()
                        .buttons
                        .iter()
                        .find(|b| {
                            b.actions
                                .iter()
                                .any(|a| matches!(a.kind,ActionKind::EnterFolder(id) if id==f))
                        })
                        .ok_or(ConfigurationError::NotFound)?;
                    Edit::MoveButton {
                        button: entry.binding.identity(),
                        folder,
                        position,
                    }
                };
                Ok(vec![edit])
            })]))
        }
        R::Delete {
            id,
            confirm_contents,
        } => {
            let target = parse_id(id)?;
            let confirmed = *confirm_contents;
            Ok(Command::PrepareEdits(vec![Box::new(move |cfg| {
                let entity = resolve(cfg, target)?;
                Ok(vec![match target.kind() {
                    EntityKind::Folder => Edit::DeleteFolder {
                        folder: entity,
                        confirm_contents: confirmed,
                    },
                    EntityKind::Button => Edit::DeleteButton { button: entity },
                    EntityKind::Action => Edit::RemoveAction { action: entity },
                }])
            })]))
        }
        R::SetPermissions { target, value } => {
            let target = if target == "global" {
                None
            } else {
                Some(parse_id(target)?)
            };
            let value = local(value)?;
            if target.is_none() && value.own_value().is_none() {
                return Err("invalid_request");
            };
            Ok(Command::PrepareEdits(vec![Box::new(move |cfg| {
                Ok(vec![match target {
                    None => Edit::SetGlobalPermissions(value.own_value().unwrap()),
                    Some(id) => match id.kind() {
                        EntityKind::Button => Edit::SetButtonPermissions {
                            button: resolve(cfg, id)?,
                            permissions: value,
                        },
                        EntityKind::Action => Edit::SetActionPermissions {
                            action: resolve(cfg, id)?,
                            permissions: value,
                        },
                        EntityKind::Folder => return Err(ConfigurationError::NotAllowed),
                    },
                }])
            })]))
        }
        R::SetPluginSettings {
            plugin_id,
            parameters,
        } => {
            let plugin = plugin_id.clone();
            let settings = parameters
                .as_ref()
                .map(|p| {
                    let bytes = serde_json::to_vec(p).map_err(|_| "invalid_request")?;
                    rad_core::configuration::PluginSettings::new(bytes)
                        .map_err(|_| "invalid_request")
                })
                .transpose()?;
            Ok(Command::PrepareEdits(vec![Box::new(move |_| {
                Ok(vec![Edit::SetPluginSettings { plugin, settings }])
            })]))
        }
        R::SetSettings { changes } => {
            if changes == &rad_api::SettingsChanges::default() {
                return Err("invalid_request");
            }
            let brightness = changes.brightness_percent;
            if brightness.is_some_and(|n| n > 100) {
                return Err("invalid_request");
            }
            let sleep = changes.sleep_ms.as_deref().map(decimal).transpose()?;
            let threshold = changes
                .press_threshold_ms
                .as_deref()
                .map(decimal)
                .transpose()?;
            let long_threshold = changes
                .long_press_threshold_ms
                .as_deref()
                .map(decimal)
                .transpose()?;
            Ok(Command::PrepareEdits(vec![Box::new(move |cfg| {
                let mut s = cfg.settings().data();
                if let Some(n) = brightness {
                    s.brightness_percent = n
                }
                if let Some(n) = sleep {
                    s.sleep_ms = n
                }
                if let Some(n) = threshold {
                    s.press_threshold_ms = n
                }
                if let Some(n) = long_threshold {
                    s.long_press_threshold_ms = n;
                }
                Ok(vec![Edit::SetSettings(s)])
            })]))
        }
        _ => Err("invalid_request"),
    }
}
fn local(value: &rad_api::LocalPermissions) -> Checked<LocalPermissions> {
    match value {
        rad_api::LocalPermissions::Inherit(true) => Ok(LocalPermissions::default()),
        rad_api::LocalPermissions::Inherit(false) => Err("invalid_request"),
        rad_api::LocalPermissions::Own(p) => Ok(LocalPermissions::own(
            Permissions::new(p.consult, p.modify, p.execute).map_err(|_| "invalid_request")?,
        )),
    }
}
