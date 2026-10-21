//! Recent runtime events for Advanced → Log (ADR 0011). Codes, never prose: the Editor
//! words them. Integers travel as decimal strings like everywhere else in the client API.
use serde::{Deserialize, Serialize};

macro_rules! event_codes {
    ($($variant:ident => $wire:literal),* $(,)?) => {
        /// Closed vocabulary of what the runtime reports about itself.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum EventCode { $(#[serde(rename = $wire)] $variant),* }
        impl EventCode {
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),* }
            }
        }
    };
}
event_codes! {
    Started => "started",
    PreviousExit => "previous_exit",
    ShutdownRequested => "shutdown_requested",
    DeckConnected => "deck_connected",
    DeckDisconnected => "deck_disconnected",
    DeckInaccessible => "deck_inaccessible",
    DeckSpawnFailed => "deck_spawn_failed",
    PluginFailed => "plugin_failed",
    LauncherLost => "launcher_lost",
    StorageUncertain => "storage_uncertain",
    StorageRecoveryRequired => "storage_recovery_required",
    StorageReady => "storage_ready",
    DisplayClockOverflow => "display_clock_overflow",
    PluginIoUnregistrable => "plugin_io_unregistrable",
    ResponseRefused => "response_refused",
    ResponseUnknownRequest => "response_unknown_request",
    ResponseDropped => "response_dropped",
    NoticeDropped => "notice_dropped",
    ResponseUnbuildable => "response_unbuildable",
    PluginResponseInvalid => "plugin_response_invalid",
    CatalogResponseInvalid => "catalog_response_invalid",
    ConnectionFailed => "connection_failed",
    ExportFailed => "export_failed",
    RecoveryMode => "recovery_mode",
    RecoveryReset => "recovery_reset",
    MachineSettingsUnreadable => "machine_settings_unreadable",
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEvent {
    /// Grows by one per event for the life of the process; a gap means entries were dropped.
    pub seq: String,
    pub at_unix_ms: String,
    pub code: EventCode,
    /// What the event is about (a plugin id, an exit reason); at most 128 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLogView {
    /// Oldest first.
    pub entries: Vec<RuntimeEvent>,
    /// Events discarded because the buffer was full, since the process started.
    pub dropped: String,
}
