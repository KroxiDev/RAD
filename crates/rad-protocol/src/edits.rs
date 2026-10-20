use super::*;
use crate::admin;
use rad_core::configuration::{
    ActionKind, ActionParameters, Appearance, ButtonData, Configuration, ConfigurationError,
    DesktopEntry, Edit, ExternalType, Mode, TextPosition, VisualSource, Wait,
};
use rad_core::identity::EntityIdentity;
pub(super) fn command(op: &str, args: &Map<String, Value>) -> Checked<Command> {
    match op {
        "create_button" | "create_folder" => {
            let key = if op == "create_button" {
                "folder_id"
            } else {
                "parent_id"
            };
            closed(args, &[key, "position", "name"])?;
            let folder = id(args, key)?;
            if folder.kind() != EntityKind::Folder {
                return Err("invalid_request");
            };
            let position = position(args)?;
            let name = string(args, "name")?.to_owned();
            let is_folder = op == "create_folder";
            Ok(Command::PrepareEdits(vec![Box::new(move |cfg| {
                let folder = resolve(cfg, folder)?;
                Ok(vec![if is_folder {
                    Edit::CreateFolder {
                        parent: folder,
                        position,
                        name,
                    }
                } else {
                    Edit::CreateButton {
                        folder,
                        position,
                        name,
                    }
                }])
            })]))
        }
        "add_action" => {
            closed(args, &["button_id", "type", "parameters"])?;
            let id = id(args, "button_id")?;
            let parameters = args.get("parameters").ok_or("invalid_request")?;
            let kind = match string(args, "type")? {
                "launcher" => ActionKind::Launcher(launcher(parameters)?),
                "wait" => ActionKind::Wait(wait(parameters)?),
                name => ActionKind::External {
                    type_key: external_key(name, ':')?,
                    parameters: parameter_bytes(parameters)?,
                },
            };
            Ok(Command::PrepareEdits(vec![Box::new(move |cfg| {
                Ok(vec![Edit::AddAction {
                    button: button(cfg, id)?.binding.identity(),
                    kind,
                }])
            })]))
        }
        "edit" => {
            closed(args, &["changes"])?;
            let changes = args
                .get("changes")
                .and_then(Value::as_array)
                .filter(|v| !v.is_empty() && v.len() <= 256)
                .ok_or("invalid_request")?;
            let steps = changes
                .iter()
                .map(parse_change)
                .collect::<Checked<Vec<_>>>()?
                .into_iter()
                .map(|change| {
                    Box::new(move |cfg: &Configuration| change.prepare(cfg))
                        as rad_authority::EditPreparation
                })
                .collect();
            Ok(Command::PrepareEdits(steps))
        }
        _ => admin::command(op, args),
    }
}
pub(super) fn resolve(
    cfg: &Configuration,
    id: ShortId,
) -> Result<EntityIdentity, ConfigurationError> {
    cfg.data()
        .folders
        .iter()
        .map(|f| f.binding)
        .chain(cfg.data().buttons.iter().map(|b| b.binding))
        .chain(
            cfg.data()
                .buttons
                .iter()
                .flat_map(|b| b.actions.iter().map(|a| a.binding)),
        )
        .find(|b| b.short() == id)
        .map(|b| b.identity())
        .ok_or(ConfigurationError::NotFound)
}
pub(super) fn position(args: &Map<String, Value>) -> Checked<usize> {
    args.get("position")
        .and_then(Value::as_u64)
        .filter(|n| *n < 32)
        .map(|n| n as usize)
        .ok_or("invalid_request")
}

pub(super) fn button(cfg: &Configuration, id: ShortId) -> Result<&ButtonData, ConfigurationError> {
    cfg.data()
        .buttons
        .iter()
        .find(|b| b.binding.short() == id)
        .ok_or(ConfigurationError::NotFound)
}
fn launcher(v: &Value) -> Checked<DesktopEntry> {
    let m = v.as_object().ok_or("invalid_request")?;
    closed(m, &["desktop_entry"])?;
    DesktopEntry::new(string(m, "desktop_entry")?.into()).map_err(|_| "invalid_request")
}
fn wait(v: &Value) -> Checked<Wait> {
    let m = v.as_object().ok_or("invalid_request")?;
    closed(m, &["milliseconds"])?;
    milliseconds(m.get("milliseconds").ok_or("invalid_request")?)
}
fn milliseconds(v: &Value) -> Checked<Wait> {
    Wait::new(v.as_u64().ok_or("invalid_request")?).map_err(|_| "invalid_request")
}
enum Change {
    Name(ShortId, String),
    Visual(ShortId, VisualSource),
    /// Text and image always; the text position only when the edit names it.
    Appearance {
        id: ShortId,
        text: String,
        image: Option<String>,
        text_position: Option<Option<TextPosition>>,
    },
    Mode(ShortId, Mode),
    Parameters(ShortId, ActionParameters),
    Order(ShortId, Vec<usize>),
    Move(ShortId, usize),
}
fn parse_change(v: &Value) -> Checked<Change> {
    let m = v.as_object().ok_or("invalid_request")?;
    let target = id(m, "id")?;
    match string(m, "kind")? {
        "set" => {
            closed(m, &["kind", "id", "field", "value"])?;
            let value = m.get("value").ok_or("invalid_request")?;
            let max = if matches!(string(m, "field")?, "parameters" | "visual") {
                17408
            } else {
                8192
            };
            if serde_json::to_vec(value).unwrap().len() > max {
                return Err("invalid_request");
            };
            match string(m, "field")? {
                "visual" => {
                    let v = value.as_object().ok_or("invalid_request")?;
                    closed(v, &["type", "parameters"])?;
                    let visual = if string(v, "type")? == "manual" {
                        VisualSource::Manual
                    } else {
                        VisualSource::External {
                            type_key: external_key(string(v, "type")?, '#')?,
                            configuration: parameter_bytes(
                                v.get("parameters").ok_or("invalid_request")?,
                            )?,
                        }
                    };
                    Ok(Change::Visual(target, visual))
                }
                "name" => Ok(Change::Name(
                    target,
                    value.as_str().ok_or("invalid_request")?.into(),
                )),
                "mode" => Ok(Change::Mode(
                    target,
                    match value.as_str() {
                        Some("sequential") => Mode::Sequential,
                        Some("parallel") => Mode::Parallel,
                        _ => return Err("invalid_request"),
                    },
                )),
                "appearance" => {
                    let m = value.as_object().ok_or("invalid_request")?;
                    closed(m, &["text", "image", "text_position"])?;
                    let image = match m.get("image") {
                        Some(Value::Null) => None,
                        Some(Value::String(s)) => Some(s.clone()),
                        _ => return Err("invalid_request"),
                    };
                    let text_position = match m.get("text_position") {
                        None => None,
                        Some(Value::Null) => Some(None),
                        Some(Value::String(s)) => Some(Some(match s.as_str() {
                            "top" => TextPosition::Top,
                            "middle" => TextPosition::Middle,
                            "bottom" => TextPosition::Bottom,
                            "above_image" => TextPosition::AboveImage,
                            "below_image" => TextPosition::BelowImage,
                            _ => return Err("invalid_request"),
                        })),
                        Some(_) => return Err("invalid_request"),
                    };
                    Ok(Change::Appearance {
                        id: target,
                        text: string(m, "text")?.into(),
                        image,
                        text_position,
                    })
                }
                "desktop_entry" => Ok(Change::Parameters(
                    target,
                    ActionParameters::Launcher(
                        DesktopEntry::new(value.as_str().ok_or("invalid_request")?.into())
                            .map_err(|_| "invalid_request")?,
                    ),
                )),
                "milliseconds" => Ok(Change::Parameters(
                    target,
                    ActionParameters::Wait(milliseconds(value)?),
                )),
                // The object's own field says which kind of action it is meant for.
                "parameters" => Ok(Change::Parameters(
                    target,
                    if value.get("schema_version").is_some() {
                        ActionParameters::External(parameter_bytes(value)?)
                    } else if value.get("milliseconds").is_some() {
                        ActionParameters::Wait(wait(value)?)
                    } else {
                        ActionParameters::Launcher(launcher(value)?)
                    },
                )),
                _ => Err("invalid_request"),
            }
        }
        "order" => {
            closed(m, &["kind", "id", "indices"])?;
            let indices = m
                .get("indices")
                .and_then(Value::as_array)
                .ok_or("invalid_request")?
                .iter()
                .map(|v| {
                    v.as_u64()
                        .and_then(|n| usize::try_from(n).ok())
                        .ok_or("invalid_request")
                })
                .collect::<Checked<_>>()?;
            Ok(Change::Order(target, indices))
        }
        "move" => {
            closed(m, &["kind", "id", "position"])?;
            Ok(Change::Move(target, position(m)?))
        }
        _ => Err("invalid_request"),
    }
}
impl Change {
    fn prepare(self, cfg: &Configuration) -> Result<Vec<Edit>, ConfigurationError> {
        let e = match self {
            Self::Visual(id, visual) => Edit::SetVisual {
                button: button(cfg, id)?.binding.identity(),
                visual,
            },
            Self::Name(id, name) => Edit::SetName {
                button: button(cfg, id)?.binding.identity(),
                name,
            },
            Self::Appearance {
                id,
                text,
                image,
                text_position,
            } => {
                let b = button(cfg, id)?;
                Edit::SetAppearance {
                    button: b.binding.identity(),
                    appearance: Appearance {
                        text,
                        image,
                        text_position: text_position.unwrap_or(b.appearance.text_position),
                    },
                }
            }
            Self::Parameters(id, parameters) => Edit::SetParameters {
                action: resolve(cfg, id)?,
                parameters,
            },
            Self::Mode(id, mode) => {
                let b = button(cfg, id)?;
                Edit::SetPlan {
                    button: b.binding.identity(),
                    mode,
                    order: b.actions.iter().map(|a| a.binding.identity()).collect(),
                }
            }
            Self::Order(id, indices) => {
                let b = button(cfg, id)?;
                let order = indices
                    .into_iter()
                    .map(|i| {
                        b.actions
                            .get(i)
                            .map(|a| a.binding.identity())
                            .ok_or(ConfigurationError::InvalidActions)
                    })
                    .collect::<Result<_, _>>()?;
                Edit::SetPlan {
                    button: b.binding.identity(),
                    mode: b.mode,
                    order,
                }
            }
            Self::Move(id, position) => {
                let b = button(cfg, id)?;
                let folder = cfg
                    .data()
                    .folders
                    .iter()
                    .find(|f| f.slots.contains(&Some(b.binding.identity())))
                    .ok_or(ConfigurationError::NotFound)?;
                Edit::MoveButton {
                    button: b.binding.identity(),
                    folder: folder.binding.identity(),
                    position,
                }
            }
        };
        Ok(vec![e])
    }
}

fn external_key(name: &str, separator: char) -> Checked<ExternalType> {
    let (plugin, kind) = name.split_once(separator).ok_or("invalid_request")?;
    ExternalType::new(plugin.into(), kind.into()).map_err(|_| "invalid_request")
}
fn parameter_bytes(value: &Value) -> Checked<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(|_| "invalid_request")?;
    if bytes.len() > 16384 {
        return Err("invalid_request");
    }
    // Manifest-specific validation is performed once by Authority before Persist.
    serde_json::from_value::<rad_plugin_contract::Parameters>(value.clone())
        .map_err(|_| "invalid_request")?;
    Ok(bytes)
}
