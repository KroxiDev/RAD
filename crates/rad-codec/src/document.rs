use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Document {
    pub settings: Settings,
    pub version: u8,
    pub grid: [u8; 2],
    #[serde(
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty",
        deserialize_with = "unique_map"
    )]
    pub plugin_dependencies: std::collections::BTreeMap<String, PluginDependency>,
    #[serde(
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty",
        deserialize_with = "unique_map"
    )]
    pub plugin_settings: std::collections::BTreeMap<String, String>,
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
    #[serde(default, skip_serializing_if = "Gesture::is_press")]
    pub gesture: Gesture,
    pub actions: Vec<Action>,
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
    #[serde(
        default = "default_long_threshold",
        skip_serializing_if = "is_default_long_threshold"
    )]
    pub long_press_threshold_ms: String,
}

fn unique_map<'de, D: serde::Deserializer<'de>, V: Deserialize<'de>>(
    d: D,
) -> Result<std::collections::BTreeMap<String, V>, D::Error> {
    struct Visitor<V>(std::marker::PhantomData<V>);
    impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<V> {
        type Value = std::collections::BTreeMap<String, V>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("unique plugin settings")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut result = std::collections::BTreeMap::new();
            while let Some((k, v)) = map.next_entry::<String, V>()? {
                if result.insert(k, v).is_some() {
                    return Err(serde::de::Error::custom("duplicate plugin"));
                }
            }
            Ok(result)
        }
    }
    d.deserialize_map(Visitor(std::marker::PhantomData))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PluginDependency {
    pub version: Option<PluginVersion>,
    pub origin: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PluginVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Gesture {
    #[default]
    Press,
    Hold,
    LongPress,
}
impl Gesture {
    fn is_press(&self) -> bool {
        matches!(self, Self::Press)
    }
}
fn default_long_threshold() -> String {
    "500".into()
}
fn is_default_long_threshold(value: &str) -> bool {
    value == "500"
}
