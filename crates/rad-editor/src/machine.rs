//! What this computer is set to do with RAD (ADR 0006, 0012): two switches that move
//! together. The runtime stores them; starting with the session also needs the user's
//! service manager, which only the editor reaches, so a change may have two steps.
use crate::Request;
use serde_json::Value;

/// The runtime's answer to `machine_settings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub use_stream_deck: bool,
    pub start_with_session: bool,
}
impl Settings {
    pub fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            use_stream_deck: value["use_stream_deck"].as_bool()?,
            start_with_session: value["start_with_session"].as_bool()?,
        })
    }
}

/// One switch moved by the user: the autostart step (if any) and what the runtime stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    autostart: Option<bool>,
    use_stream_deck: Option<bool>,
    start_with_session: Option<bool>,
}
impl Change {
    /// Using the Stream Deck with RAD turns on starting with the computer, since RAD must
    /// be running to own the deck; turning it off leaves the other switch alone.
    pub fn use_stream_deck(on: bool) -> Self {
        Self {
            autostart: on.then_some(true),
            use_stream_deck: Some(on),
            start_with_session: on.then_some(true),
        }
    }
    pub fn start_with_session(on: bool) -> Self {
        Self {
            autostart: Some(on),
            use_stream_deck: None,
            start_with_session: Some(on),
        }
    }
    /// Enable or disable the unit first; the runtime stores nothing until that worked.
    pub fn autostart(&self) -> Option<bool> {
        self.autostart
    }
    /// What the runtime stores.
    pub fn request(&self) -> Request {
        Request::SetMachineSettings {
            use_stream_deck: self.use_stream_deck,
            start_with_session: self.start_with_session,
        }
    }
    /// The request to send once the autostart step has finished; none when it failed, so
    /// the runtime never records a start-up that the system does not have.
    pub fn after_autostart(&self, succeeded: bool) -> Option<Request> {
        succeeded.then(|| self.request())
    }
}
