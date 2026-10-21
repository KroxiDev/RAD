//! Concrete v1 response payloads. Untagged encoding preserves the legacy wire
//! representation; the operation and envelope status supply the discriminator.
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
macro_rules! dto {
    ($name:ident { $($(#[$attr:meta])* $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $( $(#[$attr])* pub $field: $ty ),* }
    }
}
dto!(Empty {});
dto!(Applied {
    changed: bool,
    revision: String
});
dto!(Admitted {
    execution_id: String
});
dto!(Hello { protocol: u8, session_id: String, capabilities: Vec<Operation>, limits: Limits });
dto!(Limits {
    frame_bytes: usize,
    page_items: usize,
    page_bytes: usize,
    value_bytes: usize,
    json_depth: usize,
    string_bytes: usize,
    array_items: usize,
    connections: usize,
    automation_connections: usize,
    requests_per_connection: usize,
    requests_global: usize,
    output_frames_per_connection: usize,
    output_bytes_per_connection: usize,
    hello_ms: u64,
    partial_frame_ms: u64,
    idle_ms: u64,
    write_ms: u64,
    request_ms: u64,
    queued_request_ms: u64,
    active_executions: usize,
    live_jobs: usize,
    terminal_results: usize,
    terminal_bytes: usize,
    terminal_ttl_ms: u64,
    result_bytes: usize,
    error_message_bytes: usize
});
dto!(StatusView {
    storage_state: StorageState, view_revision: Option<String>, active_folder_id: Option<String>, grid: Grid,
    #[serde(default,skip_serializing_if="Option::is_none")] settings: Option<Settings>,
    #[serde(default,skip_serializing_if="Option::is_none")] global_permissions: Option<Permissions>,
    #[serde(default,skip_serializing_if="Option::is_none")] history_operations: Option<usize>,
    #[serde(default,skip_serializing_if="Option::is_none")] deck_state: Option<DeckState>,
    #[serde(default,skip_serializing_if="Option::is_none")] recovered_previous: Option<bool>,
    #[serde(default,skip_serializing_if="Option::is_none")] previous_exit: Option<PreviousExit>
});
dto!(EntityPage { items: Vec<EntityView>, next_cursor: Option<String>, view_revision: String });
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "name",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AppearanceField {
    Text(String),
    Image(Option<String>),
    TextPosition(TextPosition),
}
dto!(AppearanceSection { section: AppearanceSectionName, items: Vec<AppearanceField>, next_cursor: Option<String>, view_revision: String, visual_type: Option<String> });
dto!(ActionsSection { section: ActionsSectionName, items: Vec<ActionView>, next_cursor: Option<String>, view_revision: String });
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceSectionName {
    Appearance,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionsSectionName {
    Actions,
}
dto!(SectionData { #[serde(default,skip_serializing_if="Option::is_none")] appearance: Option<Vec<AppearanceField>>, #[serde(default,skip_serializing_if="Option::is_none")] actions: Option<Vec<ActionView>> });
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompoundEntity {
    #[serde(flatten)]
    pub entity: EntityView,
    pub section_data: SectionData,
}
/// Metadata belongs to the plugin contract, not the client API. This explicit
/// document transports one field definition unchanged, never an API envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginFieldDocument(pub BTreeMap<String, serde_json::Value>);
dto!(SchemaPage { #[serde(rename="type")] type_key: String, items: Vec<PluginFieldDocument>, next_cursor: Option<String>, #[serde(default,skip_serializing_if="Option::is_none")] schema_version: Option<u32>, #[serde(default,skip_serializing_if="Option::is_none")] credential_slots: Option<Vec<String>> });
dto!(CatalogItem { #[serde(default="crate::default_gestures")] gestures: Vec<Gesture>, #[serde(rename="type")] type_key: String, kind: CatalogKind, label: String, version: PluginVersion, ready: bool });
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogKind {
    Action,
    Visual,
}
dto!(CatalogPage { items: Vec<CatalogItem>, next_cursor: Option<String> });
dto!(PluginVersion {
    major: u32,
    minor: u32,
    patch: u32
});
/// Entire manifest is a document of the separately versioned plugin contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginManifestDocument(pub BTreeMap<String, serde_json::Value>);
dto!(PluginManifestView {
    plugin_id: String,
    document: PluginManifestDocument
});
dto!(PluginLogView { plugin_id:String, lines:Vec<String> });
dto!(PluginSettingsView {plugin_id:String,encoding:HexEncoding,hex:Option<String>});
dto!(ConfigurationPage { #[serde(rename="type")] type_key: String, encoding: HexEncoding, items: Vec<String>, next_cursor: Option<String> });
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HexEncoding {
    Hex,
}
dto!(Dependency { id: String, version: Option<PluginVersion>, origin: Option<String>, credential_slots: Vec<String>, needs_credentials: Option<bool>, #[serde(default)] can_retry: bool, #[serde(default)] has_settings: bool });
dto!(DependenciesPage { items: Vec<Dependency>, next_cursor: Option<String>, view_revision: String });
dto!(Choice {
    value: String,
    label: String
});
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionsAvailability {
    Available,
    Unavailable,
}
dto!(OptionsPage { #[serde(rename="type")] type_key: String, field: String, availability: OptionsAvailability, items: Vec<Choice>, next_cursor: Option<String>, #[serde(default,skip_serializing_if="Option::is_none",deserialize_with="nullable_string")] saved: Option<Option<String>>, #[serde(default,skip_serializing_if="Option::is_none")] saved_listed: Option<bool> });
fn nullable_string<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StaticOption {
    Fixed(String),
    Launcher(LauncherEntry),
}
dto!(StaticOptionsPage {items:Vec<StaticOption>,next_cursor:Option<String>});
dto!(LauncherEntry {
    desktop_entry: String,
    name: String
});
dto!(LauncherPage { items: Vec<LauncherEntry>, next_cursor: Option<String> });
dto!(PluginAdministration {
    operation_id: String,
    state: PluginAdminState
});
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginAdminState {
    Idle,
    Preparing,
    Stopping,
    Publishing,
    Starting,
    Completed,
    Failed,
    Residual,
    Uncertain,
    Committing,
    Aborting,
    Activating,
}
dto!(PluginStatusItem { id: String, selected: PluginVersion, previous: Option<PluginVersion>, source: Option<String>, source_trusted: bool, diagnostic: Option<String>, host: String, #[serde(default,skip_serializing_if="Option::is_none")] needs_credentials: Option<bool>, #[serde(default)] can_retry: bool, #[serde(default)] has_settings: bool });
dto!(PluginStatusView { state: PluginAdminState, operation_id: Option<String>, error: Option<ErrorCode>, locked: bool, items: Vec<PluginStatusItem> });
dto!(VisualChunk { hex: String, next_offset: Option<String> });
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    Absent,
    Present,
    Unavailable,
}
dto!(CredentialStatus {
    state: CredentialState
});
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Running,
    Succeeded,
    Failed,
    TimedOut,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Waiting,
    Dispatched,
    Succeeded,
    TimedOut,
    NotDispatched,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExecutionStep {
    Restricted(RestrictedStep),
    Visible(VisibleStep),
}
dto!(VisibleStep {index:usize,state:StepState,code:Option<ErrorCode>});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestrictedStep {
    pub visibility: RestrictedVisibility,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestrictedVisibility {
    Restricted,
}
dto!(ExecutionResult { execution_id: String, state: ExecutionState, steps: Vec<ExecutionStep> });
dto!(Transfer {
    transfer_id: String
});
dto!(ExportReady {
    transfer_id: String,
    size: String
});
dto!(RestorePlugin { id: String, version: Option<PluginVersion>, origin: Option<String> });
dto!(RestoreSummary { entities: usize, media_count: usize, media_bytes: u64, document_bytes: u64, plugins_count: usize, plugins_complete: bool, plugins: Vec<RestorePlugin> });
dto!(RestorePrepared {
    transfer_id: String,
    summary: RestoreSummary
});
dto!(ImagePrepared { media_ref: String });
dto!(TransferChunk {
    base64: String,
    eof: bool
});
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ResponseData {
    None,
    Hello(Hello),
    Status(StatusView),
    Board(BoardView),
    Inspector(InspectorView),
    Overview(OverviewView),
    CompoundEntity(CompoundEntity),
    Entity(EntityView),
    EntityPage(EntityPage),
    Appearance(AppearanceSection),
    Actions(ActionsSection),
    Schema(SchemaPage),
    PluginManifest(PluginManifestView),
    PluginSettings(PluginSettingsView),
    PluginLog(PluginLogView),
    RuntimeLog(RuntimeLogView),
    Catalog(CatalogPage),
    Configuration(ConfigurationPage),
    Dependencies(DependenciesPage),
    Options(OptionsPage),
    StaticOptions(StaticOptionsPage),
    Launcher(LauncherPage),
    PluginAdministration(PluginAdministration),
    PluginStatus(PluginStatusView),
    Visual(VisualView),
    VisualChunk(VisualChunk),
    Credential(CredentialStatus),
    Applied(Applied),
    Admitted(Admitted),
    Execution(ExecutionResult),
    RestorePrepared(RestorePrepared),
    ExportReady(ExportReady),
    Transfer(Transfer),
    ImagePrepared(ImagePrepared),
    TransferChunk(TransferChunk),
    Empty(Empty),
}
impl ResponseData {
    /// Decode by the actual request, not by a heuristic over overlapping shapes
    /// (in particular, all paginated lists can be empty).
    pub fn decode(
        operation: Operation,
        status: crate::response::Status,
        value: serde_json::Value,
    ) -> Result<Self, serde_json::Error> {
        use crate::response::Status;
        macro_rules! decode {
            ($variant:ident) => {
                crate::strict::from_value(value).map(Self::$variant)
            };
        }
        if status == Status::Error {
            return serde_json::from_value::<()>(value).map(|()| Self::None);
        }
        if status == Status::Applied {
            return decode!(Applied);
        }
        if status == Status::Admitted {
            return decode!(Admitted);
        }
        match operation {
            Operation::Hello => decode!(Hello),
            Operation::Status => decode!(Status),
            Operation::Board => decode!(Board),
            Operation::Inspector => decode!(Inspector),
            Operation::Overview => decode!(Overview),
            Operation::Get => decode!(Entity),
            Operation::List => decode!(EntityPage),
            Operation::GetSection => {
                if value["section"] == "appearance" {
                    decode!(Appearance)
                } else {
                    decode!(Actions)
                }
            }
            Operation::Schema => decode!(Schema),
            Operation::PluginCatalog => decode!(Catalog),
            Operation::PluginManifest => decode!(PluginManifest),
            Operation::PluginSettings => decode!(PluginSettings),
            Operation::PluginLog => decode!(PluginLog),
            Operation::RuntimeLog => decode!(RuntimeLog),
            Operation::PluginConfiguration => decode!(Configuration),
            Operation::PluginDependencies => decode!(Dependencies),
            Operation::Options => {
                if value.get("availability").is_some() {
                    decode!(Options)
                } else {
                    decode!(StaticOptions)
                }
            }
            Operation::LauncherCatalog => decode!(Launcher),
            Operation::PluginAdmin => decode!(PluginAdministration),
            Operation::PluginStatus => decode!(PluginStatus),
            Operation::PluginVisual => decode!(Visual),
            Operation::PluginVisualRead => decode!(VisualChunk),
            Operation::CredentialStatus
            | Operation::CredentialReplace
            | Operation::CredentialRemove => decode!(Credential),
            Operation::Result => decode!(Execution),
            Operation::TransferBegin => decode!(Transfer),
            Operation::TransferRead | Operation::MediaRead => decode!(TransferChunk),
            Operation::TransferFinish => {
                if value.get("summary").is_some() {
                    decode!(RestorePrepared)
                } else if value.get("size").is_some() {
                    decode!(ExportReady)
                } else {
                    decode!(ImagePrepared)
                }
            }
            _ => decode!(Empty),
        }
    }
}
