use rad_plugin_sdk::{ChoiceList, ChoiceOption, MAX_CHOICES, MAX_LABEL, is_label};
use serde::Deserialize;

/// The value that selects whatever device is the default at the time, following changes.
pub const DEFAULT_VALUE: &str = "@default";
const DEFAULT_LABEL: &str = "Default device (follows changes)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Output,
    /// Sources that are not monitors: microphones.
    Microphone,
}

impl Kind {
    pub fn from_definition(id: &str) -> Option<Self> {
        match id {
            "output" => Some(Self::Output),
            "microphone" => Some(Self::Microphone),
            _ => None,
        }
    }

    pub(crate) fn mute_command(self) -> &'static str {
        match self {
            Self::Output => "set-sink-mute",
            Self::Microphone => "set-source-mute",
        }
    }

    pub(crate) fn default_alias(self) -> &'static str {
        match self {
            Self::Output => "@DEFAULT_SINK@",
            Self::Microphone => "@DEFAULT_SOURCE@",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub description: String,
    pub muted: bool,
    /// A source that records what a sink plays; never offered as a microphone.
    pub is_monitor: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// Whatever is the default, now and after it changes.
    Default,
    Named(String),
}

impl Selector {
    /// The selector for a stored value; `None` if it cannot name a device.
    pub fn from_value(value: &str) -> Option<Self> {
        match value {
            "" => None,
            DEFAULT_VALUE => Some(Self::Default),
            name => Some(Self::Named(name.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub sinks: Vec<Device>,
    pub sources: Vec<Device>,
    pub default_sink: Option<String>,
    pub default_source: Option<String>,
}

impl Snapshot {
    /// The devices a definition can pick, in the server's order.
    pub fn devices(&self, kind: Kind) -> impl Iterator<Item = &Device> {
        let all = match kind {
            Kind::Output => &self.sinks,
            Kind::Microphone => &self.sources,
        };
        all.iter().filter(move |d| !d.is_monitor)
    }

    pub fn find(&self, kind: Kind, name: &str) -> Option<&Device> {
        self.devices(kind).find(|d| d.name == name)
    }

    pub fn default_device(&self, kind: Kind) -> Option<&Device> {
        let name = match kind {
            Kind::Output => self.default_sink.as_deref(),
            Kind::Microphone => self.default_source.as_deref(),
        }?;
        self.find(kind, name)
    }

    pub fn resolve(&self, kind: Kind, selector: &Selector) -> Option<&Device> {
        match selector {
            Selector::Default => self.default_device(kind),
            Selector::Named(name) => self.find(kind, name),
        }
    }
}

pub(crate) fn looks_like_option(name: &str) -> bool {
    name.starts_with('-')
}

/// The `choices` for the `device` field: the default first, then up to 63 devices. A name the
/// host could not store or `pactl` could take for an option is left out, and so is a repeat of a
/// name already offered (the first is the one `pactl` would find).
pub fn options_for(kind: Kind, snapshot: &Snapshot) -> ChoiceList {
    let mut options = vec![ChoiceOption {
        value: DEFAULT_VALUE.into(),
        label: DEFAULT_LABEL.into(),
    }];
    for device in snapshot.devices(kind) {
        if options.len() == MAX_CHOICES {
            break;
        }
        let usable = is_label(&device.name)
            && !looks_like_option(&device.name)
            && options.iter().all(|offered| offered.value != device.name);
        if usable {
            options.push(ChoiceOption {
                value: device.name.clone(),
                label: label_for(device),
            });
        }
    }
    ChoiceList::Available { options }
}

fn label_for(device: &Device) -> String {
    let cleaned = |text: &str| -> String {
        text.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut label = cleaned(&device.description);
    if label.is_empty() {
        label = cleaned(&device.name);
    }
    if label.is_empty() {
        return device.name.clone();
    }
    let mut end = label.len().min(MAX_LABEL);
    while !label.is_char_boundary(end) {
        end -= 1;
    }
    label.truncate(end);
    label
}

#[derive(Debug)]
pub struct ParseError;

#[derive(Deserialize)]
struct RawDevice {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    mute: bool,
    #[serde(default)]
    monitor_source: Option<String>,
    #[serde(default)]
    monitor_of_sink: Option<serde_json::Value>,
    #[serde(default)]
    monitor_of_sink_name: Option<String>,
}

impl RawDevice {
    fn is_monitor(&self, kind: Kind) -> bool {
        kind == Kind::Microphone
            && (self.name.ends_with(".monitor")
                || self
                    .monitor_source
                    .as_deref()
                    .is_some_and(|s| !s.is_empty())
                || self.monitor_of_sink.as_ref().is_some_and(|v| !v.is_null())
                || self
                    .monitor_of_sink_name
                    .as_deref()
                    .is_some_and(|s| !s.is_empty()))
    }
}

fn from_object<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, ParseError> {
    if !value.is_object() {
        return Err(ParseError);
    }
    serde_json::from_value(value).map_err(|_| ParseError)
}

/// Parses the output of `pactl -f json list sinks|sources`.
pub fn parse_devices(json: &[u8], kind: Kind) -> Result<Vec<Device>, ParseError> {
    let items: Vec<serde_json::Value> = serde_json::from_slice(json).map_err(|_| ParseError)?;
    items
        .into_iter()
        .map(|item| {
            let raw: RawDevice = from_object(item)?;
            Ok(Device {
                is_monitor: raw.is_monitor(kind),
                name: raw.name,
                description: raw.description,
                muted: raw.mute,
            })
        })
        .collect()
}

/// Parses the output of `pactl -f json info` into the default sink and source names.
pub fn parse_defaults(json: &[u8]) -> Result<(Option<String>, Option<String>), ParseError> {
    #[derive(Deserialize)]
    struct Info {
        #[serde(default)]
        default_sink_name: Option<String>,
        #[serde(default)]
        default_source_name: Option<String>,
    }
    let info: Info = from_object(serde_json::from_slice(json).map_err(|_| ParseError)?)?;
    let named = |name: Option<String>| name.filter(|n| !n.is_empty());
    Ok((
        named(info.default_sink_name),
        named(info.default_source_name),
    ))
}
