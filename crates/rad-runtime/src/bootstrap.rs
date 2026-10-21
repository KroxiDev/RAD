//! Startup never substitutes defaults for an existing invalid Store. When the Store
//! cannot be opened, [`StartupError::diagnosis`] says whether that is a case for recovery
//! mode (ADR 0011); [`start_again`] is the way out that does not need a copy, and
//! [`restored_candidate`] the one that starts from one.
use rad_api::{RecoveryCause, RecoveryDetailCode};
use rad_authority::Authority;
use rad_core::{
    configuration::{Candidate, Configuration, Origin},
    identity::IdentityRegistry,
};
use rad_package::VerifiedRestore;
use rad_store::{PeekedCounters, ReplaceError, Store, StoreError};
use std::{
    fs, io,
    num::NonZeroU128,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Component, Path},
};

/// The database file inside the data directory.
pub const DATABASE: &str = "state.sqlite3";

/// How the Store was found to be unusable, when there is no valid revision in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreadable {
    /// Not a database, or damaged beyond what the Store can read.
    Corrupt,
    /// A database with no valid revision.
    Invalid,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupError {
    /// A relative path or one with `..`: a bug of whoever calls, not of the data.
    Path,
    /// The data directory is not a private directory of the user.
    UnsafeDirectory,
    /// `state.sqlite3` is not a regular file of the user with a single link.
    UnsafeDatabaseFile,
    Unreadable(Unreadable),
    /// Written by a newer version of RAD.
    NewerVersion,
    Storage,
    Entropy,
}
/// Why the runtime cannot serve and what the Editor is told: the recovery mode's cause
/// and the finer code behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diagnosis {
    pub cause: RecoveryCause,
    pub code: RecoveryDetailCode,
}
impl Diagnosis {
    /// The data folder was refused before any file in it was looked at.
    pub const UNSAFE_DIRECTORY: Self = Self {
        cause: RecoveryCause::UnsafeFolder,
        code: RecoveryDetailCode::UnsafeDirectory,
    };
}
impl StartupError {
    /// The recovery-mode cause, when this failure is one; `None` stays an unexpected end.
    pub fn diagnosis(self) -> Option<Diagnosis> {
        let (cause, code) = match self {
            Self::UnsafeDirectory => return Some(Diagnosis::UNSAFE_DIRECTORY),
            Self::UnsafeDatabaseFile => (
                RecoveryCause::UnsafeFolder,
                RecoveryDetailCode::UnsafeDatabaseFile,
            ),
            Self::Unreadable(Unreadable::Corrupt) => {
                (RecoveryCause::Unreadable, RecoveryDetailCode::Corrupt)
            }
            Self::Unreadable(Unreadable::Invalid) => {
                (RecoveryCause::Unreadable, RecoveryDetailCode::Invalid)
            }
            Self::NewerVersion => (
                RecoveryCause::NewerVersion,
                RecoveryDetailCode::UnsupportedVersion,
            ),
            Self::Path | Self::Storage | Self::Entropy => return None,
        };
        Some(Diagnosis { cause, code })
    }
}
/// What a failed Store operation means to startup; only these three are not unexpected.
fn classify(error: StoreError) -> StartupError {
    match error {
        StoreError::Corrupt => StartupError::Unreadable(Unreadable::Corrupt),
        StoreError::Invalid => StartupError::Unreadable(Unreadable::Invalid),
        StoreError::UnsupportedVersion => StartupError::NewerVersion,
        _ => StartupError::Storage,
    }
}

pub struct Bootstrap {
    pub authority: Authority,
    pub store: Store,
    pub recovered_previous: bool,
}
/// Called only after acquiring the runtime's instance lock. IO is startup work,
/// before the actor accepts requests. Existing invalid data is never recreated.
pub fn open(directory: &Path, session: NonZeroU128, now: u64) -> Result<Bootstrap, StartupError> {
    if !directory.is_absolute()
        || directory
            .components()
            .any(|p| matches!(p, Component::ParentDir))
    {
        return Err(StartupError::Path);
    }
    match fs::symlink_metadata(directory) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(directory)
                .map_err(|_| StartupError::UnsafeDirectory)?;
        }
        Err(_) => return Err(StartupError::UnsafeDirectory),
    }
    let meta = fs::symlink_metadata(directory).map_err(|_| StartupError::UnsafeDirectory)?;
    // SAFETY: geteuid has no pointer arguments or side effects.
    let uid = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o077 != 0 {
        return Err(StartupError::UnsafeDirectory);
    }
    let path = directory.join(DATABASE);
    let store = match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() && meta.uid() == uid && meta.nlink() == 1 => {
            Store::open(&path).map_err(classify)?
        }
        Ok(_) => return Err(StartupError::UnsafeDatabaseFile),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let registry =
                IdentityRegistry::new(random_id()?.get()).map_err(|_| StartupError::Entropy)?;
            let initial =
                Configuration::prepare_new(&registry).map_err(|_| StartupError::Storage)?;
            Store::create(&path, &initial).map_err(|_| StartupError::Storage)?
        }
        Err(_) => return Err(StartupError::UnsafeDatabaseFile),
    };
    published(store, &path, session, now)
}
/// What follows opening (or creating) the Store: the file stays private, and its
/// configuration must be one the Authority accepts.
fn published(
    store: Store,
    path: &Path,
    session: NonZeroU128,
    now: u64,
) -> Result<Bootstrap, StartupError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| StartupError::Storage)?;
    let loaded = store.load().map_err(classify)?;
    let candidate = loaded
        .configuration
        .prepare_edits(&loaded.registry, Origin::Editor, &[])
        .map_err(|_| StartupError::Unreadable(Unreadable::Invalid))?;
    Ok(Bootstrap {
        authority: Authority::new_at(candidate, loaded.revision, session, now),
        store,
        recovered_previous: loaded.recovered_previous,
    })
}

/// Why a replacement of the damaged Store did not finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaceFailed {
    /// Nothing changed: the damaged file is still where it was.
    NotApplied,
    /// The new base is in place, or may be, but the process cannot go on with it. The
    /// damaged file is kept; the next start opens whatever is at the path.
    Unfinished,
}
/// «Empezar de nuevo»: an empty configuration whose identifiers do not reuse those the
/// damaged one handed out (`peeked`, when they could be read), and whose installation is
/// the damaged one's when known. The damaged file stays as evidence.
pub fn start_again(
    directory: &Path,
    peeked: Option<PeekedCounters>,
    session: NonZeroU128,
    now: u64,
    stamp: u64,
) -> Result<Bootstrap, ReplaceFailed> {
    let registry = reserving_registry(peeked)?;
    let candidate =
        Configuration::prepare_new_reserving(&registry).map_err(|_| ReplaceFailed::NotApplied)?;
    replace_and_open(directory, session, now, stamp, |temporary| {
        Store::create(temporary, &candidate)
    })
}
/// An empty registry of the damaged file's installation (a random one when it is not
/// known) that does not hand out again the identifiers the damaged file handed out.
fn reserving_registry(peeked: Option<PeekedCounters>) -> Result<IdentityRegistry, ReplaceFailed> {
    let installation = match peeked.and_then(|peeked| peeked.installation) {
        Some(installation) => installation,
        None => random_id().map_err(|_| ReplaceFailed::NotApplied)?.get(),
    };
    let mut registry =
        IdentityRegistry::new(installation).map_err(|_| ReplaceFailed::NotApplied)?;
    if let Some(peeked) = peeked {
        registry.reserve_counters(peeked.counters);
    }
    Ok(registry)
}
/// Why a verified copy cannot become the new configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreRefused {
    /// The copy's contents are not acceptable to this configuration.
    Copy,
    /// Not the copy's fault (no entropy); nothing changed.
    Unavailable,
}
/// «Restaurar una copia»: the configuration of the copy over a new base. Identifiers
/// follow ADR 0003 against that base, whose installation is the damaged file's when
/// known, never the copy's. A copy of that same installation keeps its identifiers and
/// both it and the damaged file reserve their counters, so what is created afterwards
/// collides with neither; any other copy is remapped, or adopted when nothing in the
/// base was handed out. The copy's token is never adopted.
pub fn restored_candidate(
    peeked: Option<PeekedCounters>,
    verified: VerifiedRestore,
) -> Result<Candidate, RestoreRefused> {
    let VerifiedRestore { data, identity } = verified;
    let mut registry = reserving_registry(peeked).map_err(|_| RestoreRefused::Unavailable)?;
    if registry.installation() == identity.installation {
        registry.reserve_counters(identity.counters);
    }
    let base =
        Configuration::prepare_new_reserving(&registry).map_err(|_| RestoreRefused::Unavailable)?;
    base.configuration()
        .prepare_portable_restore(
            base.registry(),
            Origin::Editor,
            true,
            data,
            identity.installation,
            identity.counters,
        )
        .map(|restored| restored.candidate)
        .map_err(|_| RestoreRefused::Copy)
}
/// Puts the base `create` builds in place of the damaged one and opens it. What differs
/// between the ways out of recovery is only `create`.
pub fn replace_and_open(
    directory: &Path,
    session: NonZeroU128,
    now: u64,
    stamp: u64,
    create: impl FnOnce(&Path) -> Result<Store, StoreError>,
) -> Result<Bootstrap, ReplaceFailed> {
    let path = directory.join(DATABASE);
    rad_store::replace_damaged(&path, stamp, create).map_err(|error| match error {
        ReplaceError::NotApplied(_) => ReplaceFailed::NotApplied,
        ReplaceError::AfterExchange => ReplaceFailed::Unfinished,
    })?;
    open_replaced(directory, session, now)
}
/// Opens the base that [`rad_store::replace_damaged`] put in place.
pub fn open_replaced(
    directory: &Path,
    session: NonZeroU128,
    now: u64,
) -> Result<Bootstrap, ReplaceFailed> {
    let path = directory.join(DATABASE);
    let store = Store::open(&path).map_err(|_| ReplaceFailed::Unfinished)?;
    published(store, &path, session, now).map_err(|_| ReplaceFailed::Unfinished)
}

pub fn random_id() -> Result<NonZeroU128, StartupError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| StartupError::Entropy)?;
    NonZeroU128::new(u128::from_be_bytes(bytes)).ok_or(StartupError::Entropy)
}
