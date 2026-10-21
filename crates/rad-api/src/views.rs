use crate::{LocalPermissions, Mode, Permissions, Section, TextPosition};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grid {
    pub columns: usize,
    pub rows: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "visibility", rename_all = "snake_case", deny_unknown_fields)]
pub enum Slot {
    // Empty structs enforce deny_unknown_fields; tagged unit variants do not.
    Restricted {},
    Visible { id: String },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "fields",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum EntityFields {
    Folder(FolderFields),
    Button(ButtonFields),
    Action(ActionFields),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderFields {
    pub name: Option<String>,
    pub entry_button_id: Option<String>,
    pub parent_id: Option<String>,
    pub slots: Vec<Option<Slot>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ButtonFields {
    #[serde(default)]
    pub gesture: crate::Gesture,
    pub name: String,
    pub mode: Mode,
    pub permissions: LocalPermissions,
    pub effective_permissions: Permissions,
    pub folder_id: Option<String>,
    pub position: Option<usize>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionFields {
    #[serde(rename = "type")]
    pub type_key: String,
    pub permissions: LocalPermissions,
    pub effective_permissions: Permissions,
    pub parent_id: Option<String>,
    pub parameters: Option<crate::ActionParameters>,
    pub folder_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityView {
    pub visibility: Visibility,
    pub id: String,
    #[serde(flatten)]
    pub entity: EntityFields,
    pub view_revision: String,
    pub sections: Vec<Section>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Visible,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "visibility", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionView {
    // Keep this a struct variant so even unknown null members are rejected.
    Restricted {},
    Visible {
        id: String,
        #[serde(flatten)]
        entity: EntityFields,
        view_revision: String,
        sections: Vec<Section>,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppearanceView {
    pub text: String,
    pub image: Option<String>,
    pub text_position: TextPosition,
    pub visual_type: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Current,
    Stale,
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualView {
    pub availability: Availability,
    pub revision: String,
    pub text: Option<String>,
    pub text_position: TextPosition,
    pub color: Option<[u8; 4]>,
    pub image: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyView {
    pub entity: EntityView,
    pub appearance: AppearanceView,
    pub visual: Option<VisualView>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardView {
    pub view_revision: String,
    pub grid: Grid,
    pub folder: EntityView,
    pub keys: Vec<Option<KeyView>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConfiguration {
    #[serde(rename = "type")]
    pub type_key: String,
    pub parameters: crate::PluginParameters,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectorView {
    pub view_revision: String,
    pub entity: EntityView,
    pub appearance: Option<AppearanceView>,
    pub actions: Vec<ActionView>,
    pub plugin: Option<PluginConfiguration>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageState {
    Ready,
    Uncertain,
    RecoveryRequired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeckState {
    Connected,
    Disconnected,
    Inaccessible,
    Disabled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartState {
    Clean,
    RecoveredPrevious,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default = "crate::default_long_press_threshold")]
    pub long_press_threshold_ms: String,
    pub brightness_percent: u8,
    pub sleep_ms: String,
    pub press_threshold_ms: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginAttention {
    pub id: String,
    pub code: PluginAttentionCode,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginAttentionCode {
    NeedsUpdate,
    NeedsCredentials,
    Unavailable,
    Failed,
    Busy,
}
/// Why the previous run of the runtime ended unexpectedly: a stable code, never prose.
/// `at_unix_ms` is a decimal string, like every 64-bit counter of the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviousExit {
    pub code: String,
    pub at_unix_ms: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverviewView {
    pub view_revision: String,
    pub active_folder_id: Option<String>,
    pub storage: StorageState,
    pub deck: DeckState,
    pub plugins: Vec<PluginAttention>,
    pub last_restart: RestartState,
    pub settings: Settings,
    pub global_permissions: Permissions,
    pub history_operations: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_exit: Option<PreviousExit>,
}

/// Why the runtime is in recovery mode (ADR 0011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryCause {
    /// The configuration has no valid revision.
    Unreadable,
    /// The configuration was written by a newer version of RAD.
    NewerVersion,
    /// The data folder or the database file is not private and owned by the user.
    UnsafeFolder,
}
impl RecoveryCause {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::NewerVersion => "newer_version",
            Self::UnsafeFolder => "unsafe_folder",
        }
    }
}
/// What the Editor may ask the runtime to do about it; closed, so the Editor shows a
/// control only for what exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    /// «Restaurar una copia»: the transfer operations with `kind: "restore"` and
    /// `restore_commit`.
    Restore,
    /// `recovery_reset`: start again with an empty configuration.
    Reset,
    /// Show the detail; always available.
    Details,
}
/// The finer reason behind a [`RecoveryCause`], for «Ver detalles».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryDetailCode {
    Corrupt,
    Invalid,
    UnsupportedVersion,
    UnsafeDirectory,
    UnsafeDatabaseFile,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryDetail {
    pub code: RecoveryDetailCode,
    /// Whether the identity counters of the damaged file could be read, so that a new
    /// configuration does not reuse its identifiers.
    pub counters_read: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryView {
    pub cause: RecoveryCause,
    pub actions: Vec<RecoveryAction>,
    pub detail: RecoveryDetail,
}
