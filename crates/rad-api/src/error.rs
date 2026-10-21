use serde::{Deserialize, Serialize};
macro_rules! codes {
    ($($variant:ident => $wire:literal),* $(,)?)=>{
        #[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize)]
        pub enum ErrorCode { $(#[serde(rename=$wire)] $variant),* }
        impl ErrorCode {
            pub const fn as_str(self)->&'static str {match self {$(Self::$variant=>$wire),*}}
            pub fn parse(value:&str)->Option<Self>{match value {$($wire=>Some(Self::$variant)),*,_=>None}}
        }
    }
}
codes! {
    InvalidRequest => "invalid_request",
    UnsupportedVersion => "unsupported_version",
    NotFound => "not_found",
    PermissionDenied => "permission_denied",
    OperationNotAllowed => "operation_not_allowed",
    InvalidConfiguration => "invalid_configuration",
    IdentityCollision => "identity_collision",
    Busy => "busy",
    ResourceExhausted => "resource_exhausted",
    ExecutorUnavailable => "executor_unavailable",
    ServiceUnavailable => "service_unavailable",
    ActionFailed => "action_failed",
    ActionTimeout => "action_timeout",
    StorageError => "storage_error",
    StorageUncertain => "storage_uncertain",
    RecoveryRequired => "recovery_required",
    CursorExpired => "cursor_expired",
    ResultExpired => "result_expired",
    DeadlineExceeded => "deadline_exceeded",
    RuntimeUnavailable => "runtime_unavailable",
    WaitTimeout => "wait_timeout",
    PluginLimit => "plugin_limit",
    OptionsTimeout => "options_timeout",
    Cancelled => "cancelled",
    PluginInUse => "plugin_in_use",
    PluginResidual => "plugin_residual",
    PublicationPending => "publication_pending",
    IncompatiblePlugin => "incompatible_plugin",
    InvalidSource => "invalid_source",
    InvalidPackage => "invalid_package",
    Network => "network",
    Deadline => "deadline",
    Io => "io",
    ConfirmationRequired => "confirmation_required",
    PluginAlreadyInstalled => "plugin_already_installed",
    PluginMismatch => "plugin_mismatch",
    PluginSameVersion => "plugin_same_version",
    PluginFailed => "plugin_failed",
    SourceChanged => "source_changed",
}
/// Stable CLI process status, versioned together with CLI_VERSION.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitCode {
    Success = 0,
    Invalid = 2,
    Denied = 3,
    NotFound = 4,
    Unavailable = 5,
    ActionFailed = 6,
    Storage = 7,
    Unknown = 8,
    WaitTimeout = 9,
}
impl ErrorCode {
    pub const fn exit_code(self) -> ExitCode {
        use ErrorCode::*;
        match self {
            InvalidRequest | UnsupportedVersion | InvalidConfiguration | IdentityCollision
            | CursorExpired => ExitCode::Invalid,
            PermissionDenied | OperationNotAllowed => ExitCode::Denied,
            NotFound | ResultExpired => ExitCode::NotFound,
            Busy | ResourceExhausted | ExecutorUnavailable | ServiceUnavailable
            | OptionsTimeout => ExitCode::Unavailable,
            ActionFailed | ActionTimeout => ExitCode::ActionFailed,
            StorageError | StorageUncertain | RecoveryRequired => ExitCode::Storage,
            WaitTimeout => ExitCode::WaitTimeout,
            _ => ExitCode::Unknown,
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
impl PartialEq<str> for ErrorCode {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}
impl PartialEq<&str> for ErrorCode {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}
