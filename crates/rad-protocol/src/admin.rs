use super::*;
use crate::edits::{button, position, resolve};
use rad_core::{
    configuration::{ActionKind, ConfigurationError, Edit},
    permissions::LocalPermissions,
};
pub(super) fn command(op: &str, m: &Map<String, Value>) -> Checked<Command> {
    match op {
        "undo" | "redo" => {
            closed(m, &[])?;
            Ok(if op == "undo" {
                Command::Undo
            } else {
                Command::Redo
            })
        }
        "duplicate" | "move_folder" => {
            let folder_key = if op == "duplicate" {
                "folder_id"
            } else {
                "parent_id"
            };
            closed(m, &["id", folder_key, "position"])?;
            let target = id(m, "id")?;
            let folder = id(m, folder_key)?;
            let position = position(m)?;
            let duplicate = op == "duplicate";
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
        "delete" => {
            closed(m, &["id", "confirm_contents"])?;
            let target = id(m, "id")?;
            let confirmed = m
                .get("confirm_contents")
                .and_then(Value::as_bool)
                .ok_or("invalid_request")?;
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
        "set_permissions" => {
            closed(m, &["target", "value"])?;
            let target = string(m, "target")?;
            let target = if target == "global" {
                None
            } else {
                Some(parse_id(target)?)
            };
            let value = local(m.get("value").ok_or("invalid_request")?)?;
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
        "set_settings" => {
            closed(m, &["changes"])?;
            let m = m
                .get("changes")
                .and_then(Value::as_object)
                .ok_or("invalid_request")?;
            closed(
                m,
                &[
                    "brightness_percent",
                    "sleep_ms",
                    "press_threshold_ms",
                    "gui_language",
                ],
            )?;
            if m.is_empty() {
                return Err("invalid_request");
            };
            let brightness = m
                .get("brightness_percent")
                .map(|v| {
                    v.as_u64()
                        .filter(|n| *n <= 100)
                        .map(|n| n as u8)
                        .ok_or("invalid_request")
                })
                .transpose()?;
            let sleep = m
                .get("sleep_ms")
                .map(|v| decimal(v.as_str().ok_or("invalid_request")?))
                .transpose()?;
            let threshold = m
                .get("press_threshold_ms")
                .map(|v| decimal(v.as_str().ok_or("invalid_request")?))
                .transpose()?;
            let language = m
                .get("gui_language")
                .map(|v| match v.as_str() {
                    Some("en") => Ok(Language::En),
                    Some("es") => Ok(Language::Es),
                    _ => Err("invalid_request"),
                })
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
                if let Some(n) = language {
                    s.gui_language = n
                }
                Ok(vec![Edit::SetSettings(s)])
            })]))
        }
        _ => Err("invalid_request"),
    }
}
fn local(v: &Value) -> Checked<LocalPermissions> {
    let m = v.as_object().ok_or("invalid_request")?;
    if m.len() != 1 {
        return Err("invalid_request");
    };
    if m.get("inherit") == Some(&Value::Bool(true)) {
        return Ok(LocalPermissions::default());
    };
    let own = m
        .get("own")
        .and_then(Value::as_object)
        .ok_or("invalid_request")?;
    closed(own, &["consult", "modify", "execute"])?;
    let b = |key| {
        own.get(key)
            .and_then(Value::as_bool)
            .ok_or("invalid_request")
    };
    Ok(LocalPermissions::own(
        Permissions::new(b("consult")?, b("modify")?, b("execute")?)
            .map_err(|_| "invalid_request")?,
    ))
}
