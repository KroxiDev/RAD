use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Document {
    pub settings: Settings,
    pub version: u8,
    pub principal: Identity,
    pub global_permissions: Permissions,
    pub folders: Vec<Folder>,
    pub buttons: Vec<Button>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub installation: String,
    pub serial: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    pub kind: Kind,
    pub number: String,
    pub identity: Identity,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Folder,
    Button,
    Action,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Permissions {
    pub consult: bool,
    pub modify: bool,
    pub execute: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Folder {
    pub binding: Binding,
    pub parent: Option<Identity>,
    pub slots: Vec<Option<Identity>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Button {
    pub binding: Binding,
    pub name: String,
    pub appearance: Appearance,
    pub visual: Visual,
    pub permissions: Local,
    pub mode: Mode,
    pub actions: Vec<Action>,
    /// Gap list that buttons carried before waiting became an action. Read, never written.
    #[serde(default, skip_serializing)]
    pub pauses_ms: Option<Vec<String>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Appearance {
    pub text: String,
    pub image: Option<String>,
    /// Absent while the position follows the image, as in every earlier document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_position: Option<TextPosition>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TextPosition {
    Top,
    Middle,
    Bottom,
    AboveImage,
    BelowImage,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Local {
    Inherit,
    Own(Permissions),
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    Sequential,
    Parallel,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExternalType {
    pub plugin: String,
    pub name: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Action {
    pub binding: Binding,
    pub kind: ActionKind,
    pub permissions: Local,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ActionKind {
    Launcher(String),
    External {
        type_key: ExternalType,
        parameters: String,
    },
    EnterFolder(Identity),
    Return,
    Wait(String),
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Visual {
    Manual,
    External {
        type_key: ExternalType,
        configuration: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    pub brightness_percent: u8,
    pub sleep_ms: String,
    pub press_threshold_ms: String,
    pub gui_language: String,
}
