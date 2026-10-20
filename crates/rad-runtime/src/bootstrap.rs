//! Startup never substitutes defaults for an existing invalid Store.
use rad_authority::Authority;
use rad_core::{
    configuration::{Configuration, Origin},
    identity::IdentityRegistry,
};
use rad_store::Store;
use std::{
    fs, io,
    num::NonZeroU128,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Component, Path},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupError {
    Path,
    Storage,
    Entropy,
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
                .map_err(|_| StartupError::Path)?;
        }
        Err(_) => return Err(StartupError::Path),
    }
    let meta = fs::symlink_metadata(directory).map_err(|_| StartupError::Path)?;
    // SAFETY: geteuid has no pointer arguments or side effects.
    let uid = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o077 != 0 {
        return Err(StartupError::Path);
    }
    let path = directory.join("state.sqlite3");
    let store = match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() && meta.uid() == uid && meta.nlink() == 1 => Store::open(&path),
        Ok(_) => return Err(StartupError::Path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let registry =
                IdentityRegistry::new(random_id()?.get()).map_err(|_| StartupError::Entropy)?;
            let initial =
                Configuration::prepare_new(&registry).map_err(|_| StartupError::Storage)?;
            Store::create(&path, &initial)
        }
        Err(_) => return Err(StartupError::Path),
    }
    .map_err(|_| StartupError::Storage)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|_| StartupError::Path)?;
    let loaded = store.load().map_err(|_| StartupError::Storage)?;
    let candidate = loaded
        .configuration
        .prepare_edits(&loaded.registry, Origin::Editor, &[])
        .map_err(|_| StartupError::Storage)?;
    Ok(Bootstrap {
        authority: Authority::new_at(candidate, loaded.revision, session, now),
        store,
        recovered_previous: loaded.recovered_previous,
    })
}
pub fn random_id() -> Result<NonZeroU128, StartupError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| StartupError::Entropy)?;
    NonZeroU128::new(u128::from_be_bytes(bytes)).ok_or(StartupError::Entropy)
}
