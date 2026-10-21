//! Configuration document codec. Decoded data is UNTRUSTED, not Configuration.
use rad_core::{configuration::*, identity, permissions};
mod document;
mod v1;
mod v1_document;
mod v2;
mod v2_document;
mod v3;
mod v3_document;
use document as d;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    InvalidDocument,
    UnsupportedVersion,
    InvalidValue,
    TooLarge,
}
/// Encode the canonical v4 deck document.
/// No historical ledger, media bytes or runtime state.
pub fn encode(config: &Configuration) -> Result<Vec<u8>, CodecError> {
    let data = config.data();
    let doc = d::Document {
        settings: write_settings(data.settings),
        version: 4,
        grid: [data.grid.columns(), data.grid.rows()],
        plugin_dependencies: data
            .plugin_dependencies
            .iter()
            .map(|(id, p)| {
                (
                    id.clone(),
                    d::PluginDependency {
                        version: p.version.map(|v| d::PluginVersion {
                            major: v.major,
                            minor: v.minor,
                            patch: v.patch,
                        }),
                        origin: p.origin.clone(),
                    },
                )
            })
            .collect(),
        plugin_settings: data
            .plugin_settings
            .iter()
            .map(|(id, s)| (id.clone(), hex(s.bytes())))
            .collect(),
        principal: write_identity(data.principal),
        global_permissions: write_permissions(data.global_permissions),
        folders: data
            .folders
            .iter()
            .map(|f| d::Folder {
                binding: write_binding(f.binding),
                parent: f.parent.map(write_identity),
                slots: f.slots.iter().map(|s| s.map(write_identity)).collect(),
            })
            .collect(),
        buttons: data.buttons.iter().map(write_button).collect(),
    };
    check_strings(&doc)?;
    let mut writer = LimitedWriter(Vec::new());
    serde_json::to_writer(&mut writer, &doc).map_err(|_| CodecError::TooLarge)?;
    Ok(writer.0)
}
/// Caller MUST use Configuration::from_data or prepare_restore before publication.
/// Version 1 is read only through the isolated importer; future versions fail explicitly.
pub fn decode_data(bytes: &[u8]) -> Result<ConfigurationData, CodecError> {
    if bytes.len() > 1024 * 1024 {
        return Err(CodecError::TooLarge);
    }
    let version = document_version(bytes)?;
    let doc: d::Document = match version {
        1 => v1::import(bytes)?,
        2 => v2::import(bytes)?,
        3 => v3::import(bytes)?,
        4 => serde_json::from_slice(bytes).map_err(|_| CodecError::InvalidDocument)?,
        _ => return Err(CodecError::UnsupportedVersion),
    };
    check_strings(&doc)?;
    Ok(ConfigurationData {
        grid: Grid::new(doc.grid[0], doc.grid[1]).map_err(|_| CodecError::InvalidValue)?,
        plugin_dependencies: doc
            .plugin_dependencies
            .into_iter()
            .map(|(id, p)| {
                (
                    id,
                    PluginDependency {
                        version: p.version.map(|v| PluginVersion {
                            major: v.major,
                            minor: v.minor,
                            patch: v.patch,
                        }),
                        origin: p.origin,
                    },
                )
            })
            .collect(),
        plugin_settings: doc
            .plugin_settings
            .into_iter()
            .map(|(id, v)| {
                Ok((
                    id,
                    PluginSettings::new(unhex(&v)?).map_err(|_| CodecError::InvalidValue)?,
                ))
            })
            .collect::<Result<_, CodecError>>()?,
        settings: read_settings(doc.settings)?,
        principal: read_identity(doc.principal)?,
        global_permissions: read_permissions(doc.global_permissions)?,
        folders: doc
            .folders
            .into_iter()
            .map(|f| {
                Ok(FolderData {
                    binding: read_binding(f.binding)?,
                    parent: f.parent.map(read_identity).transpose()?,
                    slots: f
                        .slots
                        .into_iter()
                        .map(|s| s.map(read_identity).transpose())
                        .collect::<Result<_, _>>()?,
                })
            })
            .collect::<Result<_, CodecError>>()?,
        buttons: doc
            .buttons
            .into_iter()
            .map(read_button)
            .collect::<Result<_, _>>()?,
    })
}
fn write_identity(v: identity::EntityIdentity) -> d::Identity {
    d::Identity {
        installation: v.installation().to_string(),
        serial: v.serial().to_string(),
    }
}
fn read_identity(v: d::Identity) -> Result<identity::EntityIdentity, CodecError> {
    identity::EntityIdentity::new(decimal(&v.installation)?, decimal(&v.serial)?)
        .map_err(|_| CodecError::InvalidValue)
}
fn write_binding(v: identity::Binding) -> d::Binding {
    d::Binding {
        kind: match v.short().kind() {
            identity::EntityKind::Folder => d::Kind::Folder,
            identity::EntityKind::Button => d::Kind::Button,
            identity::EntityKind::Action => d::Kind::Action,
        },
        number: v.short().number().to_string(),
        identity: write_identity(v.identity()),
    }
}
fn read_binding(v: d::Binding) -> Result<identity::Binding, CodecError> {
    let kind = match v.kind {
        d::Kind::Folder => identity::EntityKind::Folder,
        d::Kind::Button => identity::EntityKind::Button,
        d::Kind::Action => identity::EntityKind::Action,
    };
    Ok(identity::Binding::new(
        identity::ShortId::new(kind, decimal(&v.number)?).map_err(|_| CodecError::InvalidValue)?,
        read_identity(v.identity)?,
    ))
}
fn write_permissions(v: permissions::Permissions) -> d::Permissions {
    d::Permissions {
        consult: v.consult(),
        modify: v.modify(),
        execute: v.execute(),
    }
}
fn read_permissions(v: d::Permissions) -> Result<permissions::Permissions, CodecError> {
    permissions::Permissions::new(v.consult, v.modify, v.execute)
        .map_err(|_| CodecError::InvalidValue)
}

fn write_local(v: permissions::LocalPermissions) -> d::Local {
    match v.own_value() {
        None => d::Local::Inherit,
        Some(p) => d::Local::Own(write_permissions(p)),
    }
}
fn read_local(v: d::Local) -> Result<permissions::LocalPermissions, CodecError> {
    Ok(match v {
        d::Local::Inherit => permissions::LocalPermissions::default(),
        d::Local::Own(p) => permissions::LocalPermissions::own(read_permissions(p)?),
    })
}
fn write_type(v: &ExternalType) -> d::ExternalType {
    d::ExternalType {
        plugin: v.plugin().into(),
        name: v.name().into(),
    }
}
fn read_type(v: d::ExternalType) -> Result<ExternalType, CodecError> {
    ExternalType::new(v.plugin, v.name).map_err(|_| CodecError::InvalidValue)
}
fn hex(v: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    v.iter()
        .flat_map(|b| {
            [
                DIGITS[usize::from(b >> 4)] as char,
                DIGITS[usize::from(b & 15)] as char,
            ]
        })
        .collect()
}
fn unhex(v: &str) -> Result<Vec<u8>, CodecError> {
    if !v.len().is_multiple_of(2) {
        return Err(CodecError::InvalidValue);
    }
    v.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| {
            let digit = |c| match c {
                b'0'..=b'9' => Ok(c - b'0'),
                b'a'..=b'f' => Ok(c - b'a' + 10),
                _ => Err(CodecError::InvalidValue),
            };
            Ok(digit(b[0])? * 16 + digit(b[1])?)
        })
        .collect()
}
fn write_button(v: &ButtonData) -> d::Button {
    d::Button {
        binding: write_binding(v.binding),
        name: v.name.clone(),
        appearance: d::Appearance {
            text: v.appearance.text.clone(),
            image: v.appearance.image.clone(),
            text_position: v.appearance.text_position.map(|position| match position {
                TextPosition::Top => d::TextPosition::Top,
                TextPosition::Middle => d::TextPosition::Middle,
                TextPosition::Bottom => d::TextPosition::Bottom,
                TextPosition::AboveImage => d::TextPosition::AboveImage,
                TextPosition::BelowImage => d::TextPosition::BelowImage,
            }),
        },
        visual: match &v.visual {
            VisualSource::Manual => d::Visual::Manual,
            VisualSource::External {
                type_key,
                configuration,
            } => d::Visual::External {
                type_key: write_type(type_key),
                configuration: hex(configuration),
            },
        },
        permissions: write_local(v.permissions),
        mode: match v.mode {
            Mode::Sequential => d::Mode::Sequential,
            Mode::Parallel => d::Mode::Parallel,
        },
        gesture: match v.gesture {
            Gesture::Press => d::Gesture::Press,
            Gesture::Hold => d::Gesture::Hold,
            Gesture::LongPress => d::Gesture::LongPress,
        },
        actions: v
            .actions
            .iter()
            .map(|a| d::Action {
                binding: write_binding(a.binding),
                permissions: write_local(a.permissions),
                kind: match &a.kind {
                    ActionKind::Launcher(entry) => d::ActionKind::Launcher(entry.id().into()),
                    ActionKind::External {
                        type_key,
                        parameters,
                    } => d::ActionKind::External {
                        type_key: write_type(type_key),
                        parameters: hex(parameters),
                    },
                    ActionKind::EnterFolder(id) => d::ActionKind::EnterFolder(write_identity(*id)),
                    ActionKind::Return => d::ActionKind::Return,
                    ActionKind::Sleep => d::ActionKind::Sleep,
                    ActionKind::Wait(wait) => d::ActionKind::Wait(wait.milliseconds().to_string()),
                },
            })
            .collect(),
    }
}
fn read_button(v: d::Button) -> Result<ButtonData, CodecError> {
    Ok(ButtonData {
        binding: read_binding(v.binding)?,
        name: v.name,
        appearance: Appearance {
            text: v.appearance.text,
            image: v.appearance.image,
            text_position: v.appearance.text_position.map(|position| match position {
                d::TextPosition::Top => TextPosition::Top,
                d::TextPosition::Middle => TextPosition::Middle,
                d::TextPosition::Bottom => TextPosition::Bottom,
                d::TextPosition::AboveImage => TextPosition::AboveImage,
                d::TextPosition::BelowImage => TextPosition::BelowImage,
            }),
        },
        visual: match v.visual {
            d::Visual::Manual => VisualSource::Manual,
            d::Visual::External {
                type_key,
                configuration,
            } => VisualSource::External {
                type_key: read_type(type_key)?,
                configuration: unhex(&configuration)?,
            },
        },
        permissions: read_local(v.permissions)?,
        mode: match v.mode {
            d::Mode::Sequential => Mode::Sequential,
            d::Mode::Parallel => Mode::Parallel,
        },
        gesture: match v.gesture {
            d::Gesture::Press => Gesture::Press,
            d::Gesture::Hold => Gesture::Hold,
            d::Gesture::LongPress => Gesture::LongPress,
        },
        actions: v
            .actions
            .into_iter()
            .map(|a| {
                Ok(ActionData {
                    binding: read_binding(a.binding)?,
                    permissions: read_local(a.permissions)?,
                    kind: match a.kind {
                        d::ActionKind::Launcher(entry) => ActionKind::Launcher(
                            DesktopEntry::new(entry).map_err(|_| CodecError::InvalidValue)?,
                        ),
                        d::ActionKind::External {
                            type_key,
                            parameters,
                        } => ActionKind::External {
                            type_key: read_type(type_key)?,
                            parameters: unhex(&parameters)?,
                        },
                        d::ActionKind::EnterFolder(id) => {
                            ActionKind::EnterFolder(read_identity(id)?)
                        }
                        d::ActionKind::Return => ActionKind::Return,
                        d::ActionKind::Sleep => ActionKind::Sleep,
                        d::ActionKind::Wait(milliseconds) => ActionKind::Wait(
                            Wait::new(decimal(&milliseconds)?)
                                .map_err(|_| CodecError::InvalidValue)?,
                        ),
                    },
                })
            })
            .collect::<Result<_, CodecError>>()?,
    })
}

fn decimal<T: std::str::FromStr>(s: &str) -> Result<T, CodecError> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(CodecError::InvalidValue);
    }
    s.parse().map_err(|_| CodecError::InvalidValue)
}

struct LimitedWriter(Vec<u8>);
impl std::io::Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > 1024 * 1024 - self.0.len() {
            return Err(std::io::Error::other("document limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn text(value: &str) -> Result<(), CodecError> {
    // Length check first prevents serialization allocating beyond the scalar bound.
    if value.len() > 8192
        || serde_json::to_vec(value)
            .map_err(|_| CodecError::InvalidValue)?
            .len()
            > 8192
    {
        return Err(CodecError::InvalidValue);
    }
    Ok(())
}
fn check_strings(doc: &d::Document) -> Result<(), CodecError> {
    for (id, p) in &doc.plugin_dependencies {
        text(id)?;
        ExternalType::new(id.clone(), "settings".into()).map_err(|_| CodecError::InvalidValue)?;
        if let Some(origin) = &p.origin {
            text(origin)?;
            if origin.is_empty() || origin.chars().any(char::is_control) {
                return Err(CodecError::InvalidValue);
            }
        }
    }
    for (id, value) in &doc.plugin_settings {
        text(id)?;
        text(value)?;
        ExternalType::new(id.clone(), "settings".into()).map_err(|_| CodecError::InvalidValue)?;
    }
    let type_key = |t: &d::ExternalType| -> Result<(), CodecError> {
        text(&t.plugin)?;
        text(&t.name)
    };
    for button in &doc.buttons {
        text(&button.name)?;
        text(&button.appearance.text)?;
        if let Some(image) = &button.appearance.image {
            text(image)?;
        }
        if let d::Visual::External {
            type_key: t,
            configuration,
        } = &button.visual
        {
            type_key(t)?;
            text(configuration)?;
        }
        for action in &button.actions {
            match &action.kind {
                d::ActionKind::Launcher(entry) | d::ActionKind::Wait(entry) => text(entry)?,
                d::ActionKind::External {
                    type_key: t,
                    parameters,
                } => {
                    type_key(t)?;
                    text(parameters)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn write_settings(s: SettingsData) -> d::Settings {
    d::Settings {
        brightness_percent: s.brightness_percent,
        sleep_ms: s.sleep_ms.to_string(),
        press_threshold_ms: s.press_threshold_ms.to_string(),
        long_press_threshold_ms: s.long_press_threshold_ms.to_string(),
    }
}
fn read_settings(s: d::Settings) -> Result<SettingsData, CodecError> {
    let data = SettingsData {
        brightness_percent: s.brightness_percent,
        sleep_ms: decimal(&s.sleep_ms)?,
        press_threshold_ms: decimal(&s.press_threshold_ms)?,
        long_press_threshold_ms: decimal(&s.long_press_threshold_ms)?,
    };
    Settings::from_data(data)
        .map(|s| s.data())
        .map_err(|_| CodecError::InvalidValue)
}

/// Inspect the version before migration; this does not validate the document.
pub fn document_version(bytes: &[u8]) -> Result<u64, CodecError> {
    if bytes.len() > 1024 * 1024 {
        return Err(CodecError::TooLarge);
    }
    #[derive(serde::Deserialize)]
    struct Version {
        version: u64,
    }
    let version: Version =
        serde_json::from_slice(bytes).map_err(|_| CodecError::InvalidDocument)?;
    Ok(version.version)
}
