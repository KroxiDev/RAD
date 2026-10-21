use super::*;
use crate::admin;
use rad_core::configuration::{
    ActionKind, ActionParameters, Appearance, ButtonData, Configuration, ConfigurationError,
    DesktopEntry, Edit, ExternalType, Mode, TextPosition, VisualSource, Wait,
};
use rad_core::identity::EntityIdentity;
pub(super) fn command(request: &rad_api::Request) -> Checked<Command> {
    use rad_api::Request as R;
    match request {
        R::CreateButton {
            folder_id: folder,
            position: pos,
            name,
        }
        | R::CreateFolder {
            parent_id: folder,
            position: pos,
            name,
        } => {
            let folder = parse_id(folder)?;
            if folder.kind() != EntityKind::Folder {
                return Err("invalid_request");
            }
            let position = position(*pos)?;
            let name = name.clone();
            let is_folder = matches!(request, R::CreateFolder { .. });
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
        R::AddAction {
            button_id,
            type_key,
            parameters,
        } => {
            let id = parse_id(button_id)?;
            let kind = match (type_key.as_str(), parameters) {
                ("sleep", rad_api::ActionParameters::None) => ActionKind::Sleep,
                ("launcher", rad_api::ActionParameters::Launcher(p)) => {
                    ActionKind::Launcher(launcher(&p.desktop_entry)?)
                }
                ("wait", rad_api::ActionParameters::Wait(p)) => {
                    ActionKind::Wait(milliseconds(p.milliseconds)?)
                }
                (name, rad_api::ActionParameters::Plugin(p)) => ActionKind::External {
                    type_key: external_key(name, ':')?,
                    parameters: parameter_bytes(p)?,
                },
                _ => return Err("invalid_request"),
            };
            Ok(Command::PrepareEdits(vec![Box::new(move |cfg| {
                Ok(vec![Edit::AddAction {
                    button: button(cfg, id)?.binding.identity(),
                    kind,
                }])
            })]))
        }
        R::Edit { changes } => {
            if changes.is_empty() || changes.len() > 256 {
                return Err("invalid_request");
            }
            let steps = changes
                .iter()
                .map(convert_change)
                .collect::<Checked<Vec<_>>>()?
                .into_iter()
                .map(|change| {
                    Box::new(move |cfg: &Configuration| change.prepare(cfg))
                        as rad_authority::EditPreparation
                })
                .collect();
            Ok(Command::PrepareEdits(steps))
        }
        _ => admin::command(request),
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
pub(super) fn position(position: usize) -> Checked<usize> {
    if position < 32 {
        Ok(position)
    } else {
        Err("invalid_request")
    }
}

pub(super) fn button(cfg: &Configuration, id: ShortId) -> Result<&ButtonData, ConfigurationError> {
    cfg.data()
        .buttons
        .iter()
        .find(|b| b.binding.short() == id)
        .ok_or(ConfigurationError::NotFound)
}
fn launcher(entry: &str) -> Checked<DesktopEntry> {
    DesktopEntry::new(entry.into()).map_err(|_| "invalid_request")
}
fn milliseconds(value: u64) -> Checked<Wait> {
    Wait::new(value).map_err(|_| "invalid_request")
}
fn parameters(value: &rad_api::ActionParameters) -> Checked<ActionParameters> {
    Ok(match value {
        rad_api::ActionParameters::None => return Err("invalid_request"),
        rad_api::ActionParameters::Plugin(p) => ActionParameters::External(parameter_bytes(p)?),
        rad_api::ActionParameters::Launcher(p) => {
            ActionParameters::Launcher(launcher(&p.desktop_entry)?)
        }
        rad_api::ActionParameters::Wait(p) => ActionParameters::Wait(milliseconds(p.milliseconds)?),
    })
}
enum Change {
    Gesture(ShortId, rad_core::configuration::Gesture),
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
fn convert_change(change: &rad_api::Change) -> Checked<Change> {
    use rad_api::{Change as C, FieldEdit as F};
    Ok(match change {
        C::Order { id, indices } => Change::Order(parse_id(id)?, indices.clone()),
        C::Move { id, position: pos } => Change::Move(parse_id(id)?, position(*pos)?),
        C::Set { id, edit } => {
            let target = parse_id(id)?;
            let max = if matches!(edit, F::Parameters(_) | F::Visual(_)) {
                17408
            } else {
                8192
            };
            // Account for the value, not the discriminator added by the typed edit.
            if serde_json::to_vec(
                &serde_json::to_value(edit).map_err(|_| "invalid_request")?["value"],
            )
            .map_err(|_| "invalid_request")?
            .len()
                > max
            {
                return Err("invalid_request");
            }
            match edit {
                F::Gesture(v) => Change::Gesture(
                    target,
                    match v {
                        rad_api::Gesture::Press => rad_core::configuration::Gesture::Press,
                        rad_api::Gesture::Hold => rad_core::configuration::Gesture::Hold,
                        rad_api::Gesture::LongPress => rad_core::configuration::Gesture::LongPress,
                    },
                ),
                F::Name(name) => Change::Name(target, name.clone()),
                F::Visual(v) => Change::Visual(
                    target,
                    if v.type_key == "manual" {
                        VisualSource::Manual
                    } else {
                        VisualSource::External {
                            type_key: external_key(&v.type_key, '#')?,
                            configuration: parameter_bytes(
                                v.parameters.as_ref().ok_or("invalid_request")?,
                            )?,
                        }
                    },
                ),
                F::Mode(mode) => Change::Mode(
                    target,
                    match mode {
                        rad_api::Mode::Sequential => Mode::Sequential,
                        rad_api::Mode::Parallel => Mode::Parallel,
                    },
                ),
                F::Appearance(v) => Change::Appearance {
                    id: target,
                    text: v.text.clone(),
                    image: v.image.clone(),
                    text_position: v.text_position.map(|p| {
                        p.map(|p| match p {
                            rad_api::TextPosition::Top => TextPosition::Top,
                            rad_api::TextPosition::Middle => TextPosition::Middle,
                            rad_api::TextPosition::Bottom => TextPosition::Bottom,
                            rad_api::TextPosition::AboveImage => TextPosition::AboveImage,
                            rad_api::TextPosition::BelowImage => TextPosition::BelowImage,
                        })
                    }),
                },
                F::DesktopEntry(v) => {
                    Change::Parameters(target, ActionParameters::Launcher(launcher(v)?))
                }
                F::Milliseconds(v) => {
                    Change::Parameters(target, ActionParameters::Wait(milliseconds(*v)?))
                }
                F::Parameters(v) => Change::Parameters(target, parameters(v)?),
            }
        }
    })
}
impl Change {
    fn prepare(self, cfg: &Configuration) -> Result<Vec<Edit>, ConfigurationError> {
        let e = match self {
            Self::Gesture(id, gesture) => Edit::SetGesture {
                button: button(cfg, id)?.binding.identity(),
                gesture,
            },
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
fn parameter_bytes(value: &rad_api::PluginParameters) -> Checked<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(|_| "invalid_request")?;
    if bytes.len() > 16384 {
        return Err("invalid_request");
    }
    // Manifest-specific validation is performed once by Authority before Persist.
    serde_json::from_slice::<rad_plugin_contract::Parameters>(&bytes)
        .map_err(|_| "invalid_request")?;
    Ok(bytes)
}
