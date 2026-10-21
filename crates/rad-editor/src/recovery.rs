//! The screen the editor shows while the runtime is in recovery mode (ADR 0011): what
//! happened, and only the ways out the runtime offers.
use crate::texts::tr;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    Unreadable,
    NewerVersion,
    UnsafeFolder,
    /// A cause this editor does not know: only the technical details can be offered.
    Other,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Restore,
    Reset,
    Details,
}
impl Action {
    pub fn label(self, es: impl Into<crate::preferences::Language> + Copy) -> &'static str {
        tr(
            es,
            match self {
                Self::Restore => "Restore a copy",
                Self::Reset => "Start over",
                Self::Details => "View details",
            },
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub cause: Cause,
    pub actions: Vec<Action>,
    pub code: String,
    pub counters_read: bool,
}
impl Screen {
    /// From the JSON of the runtime's `recovery` view; actions this editor does not know are
    /// left out, never guessed.
    pub fn from_value(view: &Value) -> Self {
        Self {
            cause: match view["cause"].as_str() {
                Some("unreadable") => Cause::Unreadable,
                Some("newer_version") => Cause::NewerVersion,
                Some("unsafe_folder") => Cause::UnsafeFolder,
                _ => Cause::Other,
            },
            actions: view["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|action| match action.as_str()? {
                    "restore" => Some(Action::Restore),
                    "reset" => Some(Action::Reset),
                    "details" => Some(Action::Details),
                    _ => None,
                })
                .collect(),
            code: view["detail"]["code"].as_str().unwrap_or_default().into(),
            counters_read: view["detail"]["counters_read"] == true,
        }
    }
    pub fn offers(&self, action: Action) -> bool {
        self.actions.contains(&action)
    }
    pub fn title(&self, es: impl Into<crate::preferences::Language> + Copy) -> &'static str {
        tr(
            es,
            match self.cause {
                Cause::Unreadable => "Your configuration could not be opened",
                Cause::NewerVersion => "Update RAD to open this configuration",
                Cause::UnsafeFolder => "RAD cannot use its data folder",
                Cause::Other => "Something went wrong",
            },
        )
    }
    /// Technical text, for «View details» only.
    pub fn details(&self, es: impl Into<crate::preferences::Language> + Copy) -> String {
        let counters = tr(
            es,
            if self.counters_read {
                "Identity counters were read"
            } else {
                "Identity counters could not be read"
            },
        );
        format!("{}\n{counters}", self.code)
    }
}

/// Whether the editor is in recovery mode, and what it knows about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    /// The runtime refused a request: its view of the situation is on its way.
    Asking,
    Active(Screen),
}
impl Mode {
    pub fn is_normal(&self) -> bool {
        *self == Self::Normal
    }
}
