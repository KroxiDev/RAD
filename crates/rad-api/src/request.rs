use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Dynamic plugin parameters are the only open-ended payload. Their schema is
/// owned by the selected plugin, not by the client API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginParameters {
    pub schema_version: u32,
    pub values: std::collections::BTreeMap<String, Value>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ActionParameters {
    Plugin(PluginParameters),
    Launcher(LauncherParameters),
    Wait(WaitParameters),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LauncherParameters {
    pub desktop_entry: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitParameters {
    pub milliseconds: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Sequential,
    Parallel,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextPosition {
    Top,
    Middle,
    Bottom,
    AboveImage,
    BelowImage,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Appearance {
    pub text: String,
    #[serde(deserialize_with = "required_nullable")]
    pub image: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "nullable_position"
    )]
    pub text_position: Option<Option<TextPosition>>,
}
fn nullable_position<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Option<TextPosition>>, D::Error> {
    Option::<TextPosition>::deserialize(d).map(Some)
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualSource {
    #[serde(rename = "type")]
    pub type_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<PluginParameters>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "field",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FieldEdit {
    Gesture(Gesture),
    Name(String),
    Mode(Mode),
    Appearance(Appearance),
    Visual(VisualSource),
    DesktopEntry(String),
    Milliseconds(u64),
    Parameters(ActionParameters),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    Set {
        id: String,
        #[serde(flatten)]
        edit: FieldEdit,
    },
    Order {
        id: String,
        indices: Vec<usize>,
    },
    Move {
        id: String,
        position: usize,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    pub consult: bool,
    pub modify: bool,
    pub execute: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalPermissions {
    Inherit(bool),
    Own(Permissions),
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsChanges {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_press_threshold_ms: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleep_ms: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press_threshold_ms: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferKind {
    ImageImport,
    Restore,
    Export,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginOperation {
    Install,
    Update,
    Reinstall,
    Uninstall,
    Rollback,
}
impl PluginOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Reinstall => "reinstall",
            Self::Uninstall => "uninstall",
            Self::Rollback => "rollback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Appearance,
    Actions,
}

// One declaration generates operation names, mutation classification, and payload
// variants. No client has to maintain a second string-operation table.
macro_rules! requests {
    ($( $variant:ident => $wire:literal, $mutating:literal { $( $(#[$attr:meta])* $field:ident : $ty:ty ),* $(,)? } ),* $(,)?) => {
        #[derive(Clone,PartialEq,Serialize,Deserialize)]
        #[serde(tag="op",content="args",deny_unknown_fields)]
        pub enum Request { $( #[serde(rename=$wire)] $variant { $( $(#[$attr])* $field:$ty ),* } ),* }
        #[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize)]
        pub enum Operation { $( #[serde(rename=$wire)] $variant ),* }
        impl Operation {
            pub const fn as_str(self)->&'static str { match self { $(Self::$variant=>$wire),* } }
            pub const fn is_mutation(self)->bool { match self { $(Self::$variant=>$mutating),* } }
            pub fn parse(value:&str)->Option<Self> { match value { $($wire=>Some(Self::$variant)),*, _=>None } }
        }
        impl Request {
            pub const fn operation(&self)->Operation { match self { $(Self::$variant{..}=>Operation::$variant),* } }
        }
    }
}
requests! {
    Hello => "hello", false {}, Status => "status", false {},
    Board => "board", false { folder_id:Option<String> },
    Inspector => "inspector", false { id:String },
    Overview => "overview", false {}, Subscribe => "subscribe", false {},
    Get => "get", false { id:String },
    GetSection => "get_section", false { id:String, section:Section, #[serde(deserialize_with="required_nullable")] cursor:Option<String> },
    List => "list", false { #[serde(deserialize_with="required_nullable")] cursor:Option<String>, limit:usize },
    Schema => "schema", false { #[serde(rename="type")] type_key:String, #[serde(deserialize_with="required_nullable")] cursor:Option<String> },
    Options => "options", false {
        #[serde(default,skip_serializing_if="Option::is_none")] id:Option<String>,
        #[serde(rename="type",default,skip_serializing_if="Option::is_none")] type_key:Option<String>,
        field:String, #[serde(deserialize_with="required_nullable")] cursor:Option<String>,
        #[serde(default,skip_serializing_if="Option::is_none",deserialize_with="present_non_null")] context:Option<std::collections::BTreeMap<String,String>>
    },
    PluginCatalog => "plugin_catalog", false { #[serde(deserialize_with="required_nullable")] cursor:Option<String> },
    PluginConfiguration => "plugin_configuration", false { id:String, #[serde(deserialize_with="required_nullable")] cursor:Option<String> },
    PluginDependencies => "plugin_dependencies", false { #[serde(deserialize_with="required_nullable")] cursor:Option<String> },
    PluginManifest => "plugin_manifest", false { plugin_id:String },
    PluginSettings => "plugin_settings", false { plugin_id:String },
    SetPluginSettings => "set_plugin_settings", true { plugin_id:String, parameters:Option<PluginParameters> },
    PluginLog => "plugin_log", false { plugin_id:String },
    PluginRetry => "plugin_retry", true { plugin_id:String },
    PluginStatus => "plugin_status", false {},
    PluginVisual => "plugin_visual", false { id:String },
    PluginVisualRead => "plugin_visual_read", false { id:String, revision:String, offset:String },
    PluginAdmin => "plugin_admin", true {
        operation:PluginOperation,
        #[serde(default,skip_serializing_if="Option::is_none")] plugin_id:Option<String>,
        #[serde(default,skip_serializing_if="Option::is_none")] source:Option<String>,
        #[serde(default,skip_serializing_if="Option::is_none")] trusted:Option<bool>
    },
    CredentialStatus => "credential_status", false { plugin_id:String, slot:String },
    CredentialReplace => "credential_replace", true { plugin_id:String, slot:String, secret:String },
    CredentialRemove => "credential_remove", true { plugin_id:String, slot:String },
    CreateButton => "create_button", true { folder_id:String, position:usize, name:String },
    CreateFolder => "create_folder", true { parent_id:String, position:usize, name:String },
    AddAction => "add_action", true { button_id:String, #[serde(rename="type")] type_key:String, parameters:ActionParameters },
    Edit => "edit", true { changes:Vec<Change> },
    Duplicate => "duplicate", true { id:String, folder_id:String, position:usize },
    Delete => "delete", true { id:String, confirm_contents:bool },
    MoveFolder => "move_folder", true { id:String, parent_id:String, position:usize },
    SetPermissions => "set_permissions", true { target:String, value:LocalPermissions },
    SetSettings => "set_settings", true { changes:SettingsChanges },
    Navigate => "navigate", true { folder_id:String }, Undo => "undo", true {}, Redo => "redo", true {},
    Run => "run", true { id:String }, Result => "result", false { execution_id:String },
    TransferBegin => "transfer_begin", true { kind:TransferKind, #[serde(deserialize_with="required_nullable")] size:Option<String> },
    TransferWrite => "transfer_write", true { transfer_id:String, offset:String, base64:String },
    TransferRead => "transfer_read", false { transfer_id:String, offset:String },
    TransferFinish => "transfer_finish", true { transfer_id:String },
    TransferDrop => "transfer_drop", true { transfer_id:String },
    RestoreCommit => "restore_commit", true { transfer_id:String, confirm_replace:bool },
    MediaRead => "media_read", false { id:String, offset:String },
    LauncherCatalog => "launcher_catalog", false { query:String, #[serde(deserialize_with="required_nullable")] cursor:Option<String> }
}
impl Request {
    /// Compatibility boundary for external JSON callers. Applications construct
    /// variants directly; this parser is the sole structural request definition.
    pub fn parse(operation: &str, arguments: Value) -> Result<Self, serde_json::Error> {
        crate::strict::from_value(serde_json::json!({"op":operation,"args":arguments}))
    }
    pub fn arguments(&self) -> Value {
        serde_json::to_value(self).expect("request serialization")["args"].take()
    }
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("operation", &self.operation())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gesture {
    #[default]
    Press,
    Hold,
    LongPress,
}
pub fn default_gestures() -> Vec<Gesture> {
    vec![Gesture::Press]
}
pub fn default_long_press_threshold() -> String {
    "500".into()
}

fn required_nullable<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d)
}
fn present_non_null<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}
