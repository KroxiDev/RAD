//! Plugin package management. Blocking APIs belong on the bounded runtime worker.
mod prerelease;
mod source;
pub use source::GithubSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidSource,
    InvalidPackage,
    Incompatible,
    AmbiguousAsset,
    Capacity,
    PluginLimit,
    Network,
    Deadline,
    LimitExceeded,
    Io,
    CorruptState,
    Busy,
    WrongOwner,
    InvalidOperation,
    RecoveryRequired,
    Cancelled,
    AlreadyInstalled,
    PluginMismatch,
    SameVersion,
    /// The new source is another repository than the registered one, and that change
    /// was not confirmed against the registered origin.
    SourceChanged,
}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}

mod store;
pub use store::{
    ChangeKey, CommitOutcome, InstalledPlugin, Manager, ManagerState, Operation, PreparedChange,
};

mod files;
mod ledger;

mod download;
pub use download::{Cancellation, Downloader, Progress};

#[cfg(feature = "fixture-tests")]
#[doc(hidden)]
pub use store::fail_next_prepare_after_manifest;
