//! Replacing a database that cannot be opened without ever erasing it.
//!
//! The damaged file is evidence: it ends up under a `.damaged-<stamp>` name beside the
//! new base, together with its `-journal`/`-wal`/`-shm` sidecars. The swap is a single
//! `renameat2(RENAME_EXCHANGE)`, so `path` always names a complete file.
use crate::{Store, StoreError};
use std::{
    ffi::{CString, OsStr},
    fs::{self, File},
    io,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
};

const SIDECARS: [&str; 3] = ["-journal", "-wal", "-shm"];

/// Where the damaged file and its sidecars were kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    pub evidence: PathBuf,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaceError {
    /// Nothing changed at `path` (it still is the damaged base); the temporary is gone.
    NotApplied(StoreError),
    /// The new base is already at `path`, but renaming the evidence or syncing the
    /// directory failed. The evidence still exists, under its temporary name if it
    /// could not be renamed: it is never deleted.
    AfterExchange,
}

/// Put the base `create` builds at `path` and keep the previous file as evidence.
///
/// `create` receives a fresh temporary path beside `path`; `stamp` (Unix ms, supplied
/// by the caller) only forms names. The caller opens the new base afterwards with
/// `Store::open(path)`, not with the connection `create` returned: it is closed here
/// because SQLite derives the journal name from the path a connection was opened with.
pub fn replace_damaged(
    path: &Path,
    stamp: u64,
    create: impl FnOnce(&Path) -> Result<Store, StoreError>,
) -> Result<Replaced, ReplaceError> {
    let not_applied = ReplaceError::NotApplied;
    let name = path.file_name().ok_or(not_applied(StoreError::Io))?;
    let names = Names::new(path, name, stamp);
    if exists(path) == Some(false) || names.taken() {
        return Err(not_applied(StoreError::Io));
    }
    let store = create(&names.temporary).map_err(|error| {
        discard(&names.temporary);
        not_applied(error)
    })?;
    drop(store);
    let prepared = fs::set_permissions(&names.temporary, fs::Permissions::from_mode(0o600))
        .map_err(|_| StoreError::Io)
        .and_then(|()| names.move_sidecars());
    if let Err(error) = prepared {
        discard(&names.temporary);
        return Err(not_applied(error));
    }
    if rename(&names.temporary, path, libc::RENAME_EXCHANGE).is_err() {
        discard(&names.temporary);
        return Err(not_applied(StoreError::Io));
    }
    rename(&names.temporary, &names.evidence, libc::RENAME_NOREPLACE)
        .map_err(|_| ReplaceError::AfterExchange)?;
    sync_directory(path).map_err(|_| ReplaceError::AfterExchange)?;
    Ok(Replaced {
        evidence: names.evidence,
    })
}

struct Names {
    path: PathBuf,
    temporary: PathBuf,
    evidence: PathBuf,
}
impl Names {
    fn new(path: &Path, name: &OsStr, stamp: u64) -> Self {
        let directory = path.parent().unwrap_or(Path::new(""));
        let derived = |tag: &str| {
            let mut file = name.to_owned();
            file.push(format!("{tag}-{stamp}"));
            directory.join(file)
        };
        Self {
            path: path.to_path_buf(),
            temporary: derived(".new"),
            evidence: derived(".damaged"),
        }
    }
    fn sidecar(&self, base: &Path, suffix: &str) -> PathBuf {
        let mut file = base.as_os_str().to_owned();
        file.push(suffix);
        PathBuf::from(file)
    }
    /// A leftover from an earlier attempt is never overwritten or reused.
    fn taken(&self) -> bool {
        exists(&self.temporary) != Some(false)
            || exists(&self.evidence) != Some(false)
            || SIDECARS
                .iter()
                .any(|suffix| exists(&self.sidecar(&self.evidence, suffix)) != Some(false))
    }
    /// With `journal_mode=DELETE` a hot journal left as `<path>-journal` would be
    /// replayed into the new base on its first open, so every sidecar goes with the
    /// damaged file. Sidecars already moved stay as evidence if a later one fails.
    fn move_sidecars(&self) -> Result<(), StoreError> {
        for suffix in SIDECARS {
            let sidecar = self.sidecar(&self.path, suffix);
            match exists(&sidecar) {
                Some(false) => {}
                Some(true) => rename(
                    &sidecar,
                    &self.sidecar(&self.evidence, suffix),
                    libc::RENAME_NOREPLACE,
                )
                .map_err(|_| StoreError::Io)?,
                None => return Err(StoreError::Io),
            }
        }
        Ok(())
    }
}

/// `None` when the answer is unknown (anything but "not found").
fn exists(path: &Path) -> Option<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Some(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}
/// Remove a file this call created and still owns (never the evidence).
fn discard(temporary: &Path) {
    let _ = fs::remove_file(temporary);
    let mut journal = temporary.as_os_str().to_owned();
    journal.push("-journal");
    let _ = fs::remove_file(journal);
}
fn rename(from: &Path, to: &Path, flags: u32) -> io::Result<()> {
    let cstring = |path: &Path| CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other);
    let (from, to) = (cstring(from)?, cstring(to)?);
    // SAFETY: both arguments are NUL-terminated and live for the call.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            flags,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn sync_directory(path: &Path) -> io::Result<()> {
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    File::open(directory)?.sync_all()
}
