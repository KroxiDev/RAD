//! Pure configuration candidates. No publication, persistence or executor availability.
//!
//! `*Data` values are untrusted reconstruction DTOs, not mutable domain entities.
//! Only validated `Configuration` and coherent `Candidate` values cross this boundary.
//! The authority owns Origin and publication: these APIs are not a client protocol.
use crate::identity::{Binding, EntityIdentity, EntityKind, IdentityError, IdentityRegistry};
use crate::permissions::{LocalPermissions, Permissions};

mod settings;
pub use settings::{Language, Settings, SettingsData};

mod edits;
mod validation;
use validation::validate_data;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigurationError {
    Identity(IdentityError),
    InvalidTree,
    RegistryMismatch,
    NotFound,
    Occupied,
    InvalidPosition,
    NotAllowed,
    PermissionDenied,
    InvalidActions,
    ProtectedNavigation,
    ConfirmationRequired,
    InvalidValue,
}
impl From<IdentityError> for ConfigurationError {
    fn from(error: IdentityError) -> Self {
        Self::Identity(error)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderData {
    pub binding: Binding,
    pub parent: Option<EntityIdentity>,
    pub slots: Vec<Option<EntityIdentity>>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationData {
    pub settings: SettingsData,
    pub principal: EntityIdentity,
    pub global_permissions: Permissions,
    pub folders: Vec<FolderData>,
    pub buttons: Vec<ButtonData>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    data: ConfigurationData,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    configuration: Configuration,
    registry: IdentityRegistry,
}
impl Candidate {
    pub fn configuration(&self) -> &Configuration {
        &self.configuration
    }
    pub fn registry(&self) -> &IdentityRegistry {
        &self.registry
    }
    pub fn into_parts(self) -> (Configuration, IdentityRegistry) {
        (self.configuration, self.registry)
    }
}
impl Configuration {
    /// Prepare the initial Principal only for a genuinely empty identity registry.
    pub fn prepare_new(registry: &IdentityRegistry) -> Result<Candidate, ConfigurationError> {
        if registry.records().next().is_some() {
            return Err(ConfigurationError::RegistryMismatch);
        }
        let mut registry = registry.clone();
        let binding = registry.allocate(EntityKind::Folder)?;
        Ok(Candidate {
            configuration: Self {
                data: ConfigurationData {
                    settings: SettingsData::default(),
                    principal: binding.identity(),
                    global_permissions: Permissions::default(),
                    folders: vec![FolderData {
                        binding,
                        parent: None,
                        slots: vec![None; 32],
                    }],
                    buttons: vec![],
                },
            },
            registry,
        })
    }
    pub fn settings(&self) -> Settings {
        Settings::validated(self.data.settings)
    }
    pub fn data(&self) -> &ConfigurationData {
        &self.data
    }
}
impl Configuration {
    /// Validate Store-facing data against the complete, current identity ledger.
    pub fn from_data(
        data: ConfigurationData,
        registry: &IdentityRegistry,
    ) -> Result<Self, ConfigurationError> {
        validate_data(&data, registry)?;
        Ok(Self { data })
    }
}

/// Origin is assigned by the future authority, never trusted from a client field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Editor,
    Automation,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Sequential,
    Parallel,
}
/// Where a key's text sits on its face. At the top, in the middle or at the bottom it is
/// drawn over the image; above or below it, the image makes room instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextPosition {
    Top,
    Middle,
    Bottom,
    AboveImage,
    BelowImage,
}
#[derive(Debug, Clone, PartialEq, Eq, Default)]
/// Logical owned-media reference only: Store validates bytes, never a source path.
pub struct Appearance {
    pub text: String,
    pub image: Option<String>,
    /// `None` follows the image: centred on a key without one, at the bottom over one.
    pub text_position: Option<TextPosition>,
}
impl Appearance {
    /// What the key reads: its own text, or `name` while it has neither text nor image.
    /// A key with an image and no text of its own shows the image alone.
    pub fn label<'a>(&'a self, name: &'a str) -> &'a str {
        if !self.text.is_empty() {
            &self.text
        } else if self.image.is_none() {
            name
        } else {
            ""
        }
    }
    /// Where the text is drawn once the automatic choice is resolved.
    pub fn effective_text_position(&self) -> TextPosition {
        self.text_position.unwrap_or(if self.image.is_some() {
            TextPosition::Bottom
        } else {
            TextPosition::Middle
        })
    }
}
/// Most actions one button holds. A product decision, not a hardware bound.
pub const MAX_ACTIONS: usize = 20;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonData {
    pub binding: Binding,
    pub name: String,
    pub appearance: Appearance,
    pub visual: VisualSource,
    pub permissions: LocalPermissions,
    pub mode: Mode,
    pub actions: Vec<ActionData>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionData {
    pub binding: Binding,
    pub kind: ActionKind,
    pub permissions: LocalPermissions,
}
#[derive(Debug, Clone, PartialEq, Eq)]
/// External payload is opaque non-secret configuration; no schema host or execution.
pub enum ActionKind {
    Launcher(DesktopEntry),
    External {
        type_key: ExternalType,
        parameters: Vec<u8>,
    },
    EnterFolder(EntityIdentity),
    Return,
    Wait(Wait),
}
#[derive(Debug, Clone, PartialEq, Eq)]
/// Typed edits: no generic setter, raw-document patch, or client-supplied bypass.
pub enum Edit {
    SetSettings(SettingsData),
    RemoveAction {
        action: EntityIdentity,
    },
    ReplaceActionType {
        action: EntityIdentity,
        kind: ActionKind,
    },
    DuplicateButton {
        button: EntityIdentity,
        folder: EntityIdentity,
        position: usize,
    },
    SetName {
        button: EntityIdentity,
        name: String,
    },
    SetAppearance {
        button: EntityIdentity,
        appearance: Appearance,
    },
    SetVisual {
        button: EntityIdentity,
        visual: VisualSource,
    },
    SetPlan {
        button: EntityIdentity,
        mode: Mode,
        order: Vec<EntityIdentity>,
    },
    SetParameters {
        action: EntityIdentity,
        parameters: ActionParameters,
    },
    SetGlobalPermissions(Permissions),
    SetButtonPermissions {
        button: EntityIdentity,
        permissions: LocalPermissions,
    },
    SetActionPermissions {
        action: EntityIdentity,
        permissions: LocalPermissions,
    },
    DeleteFolder {
        folder: EntityIdentity,
        confirm_contents: bool,
    },
    DeleteButton {
        button: EntityIdentity,
    },
    MoveButton {
        button: EntityIdentity,
        folder: EntityIdentity,
        position: usize,
    },
    AddAction {
        button: EntityIdentity,
        kind: ActionKind,
    },
    CreateFolder {
        parent: EntityIdentity,
        position: usize,
        name: String,
    },
    CreateButton {
        folder: EntityIdentity,
        position: usize,
        name: String,
    },
}

/// Desktop-entry ID, not an executable command or shell fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry(String);
impl DesktopEntry {
    pub fn new(id: String) -> Result<Self, ConfigurationError> {
        if !id.ends_with(".desktop")
            || id.len() <= ".desktop".len()
            || id.contains(['/', '\\'])
            || id.chars().any(char::is_control)
        {
            return Err(ConfigurationError::InvalidValue);
        }
        Ok(Self(id))
    }
    pub fn id(&self) -> &str {
        &self.0
    }
}
/// Longest deliberate wait. Nothing cancels an admitted execution, so the bound is what
/// keeps a mistyped value from holding its button busy for hours.
pub const MAX_WAIT_MS: u64 = 600_000;
/// Deliberate pause inside a button's plan, timed by the engine itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wait(u64);
impl Wait {
    pub fn new(milliseconds: u64) -> Result<Self, ConfigurationError> {
        if milliseconds > MAX_WAIT_MS {
            return Err(ConfigurationError::InvalidValue);
        }
        Ok(Self(milliseconds))
    }
    pub fn milliseconds(self) -> u64 {
        self.0
    }
}
/// Namespaced external type; availability and schema belong to a later adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalType {
    plugin: String,
    name: String,
}
impl ExternalType {
    pub fn new(plugin: String, name: String) -> Result<Self, ConfigurationError> {
        if plugin.is_empty()
            || name.is_empty()
            || plugin.chars().chain(name.chars()).any(char::is_control)
        {
            return Err(ConfigurationError::InvalidValue);
        }
        Ok(Self { plugin, name })
    }
    pub fn plugin(&self) -> &str {
        &self.plugin
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisualSource {
    Manual,
    External {
        type_key: ExternalType,
        configuration: Vec<u8>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionParameters {
    Launcher(DesktopEntry),
    External(Vec<u8>),
    Wait(Wait),
}
