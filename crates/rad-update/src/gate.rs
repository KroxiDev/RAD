//! Session-local exclusion between normal RAD processes and an installation handoff.
//! A normal process holds a shared running lease for its whole lifetime. An updater
//! first excludes new launches, then waits for every old lease before replacement.
use crate::Error;
use std::{
    fs::{File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

pub struct SessionGate {
    directory: PathBuf,
}
pub struct StartupGuard {
    _running: File,
}
pub struct UpdateGuard {
    _operation: File,
    directory: PathBuf,
}

impl SessionGate {
    /// Uses an existing private runtime directory, never another user's session.
    pub fn at(runtime: &Path) -> Result<Self, Error> {
        check_directory(runtime, true)?;
        let directory = runtime.join("rad-update");
        private_directory(&directory)?;
        Ok(Self { directory })
    }
    pub fn startup_guard(&self) -> Result<Option<StartupGuard>, Error> {
        let running = private_file(&self.directory.join("running.lock"))?;
        if !try_lock(&running, libc::LOCK_SH)? || !self.startup_allowed()? {
            return Ok(None);
        }
        Ok(Some(StartupGuard { _running: running }))
    }
    pub fn acquire_update(&self) -> Result<UpdateGuard, Error> {
        let operation = private_file(&self.directory.join("operation.lock"))?;
        if !try_lock(&operation, libc::LOCK_EX)? {
            return Err(Error("another RAD update is in progress".into()));
        }
        Ok(UpdateGuard {
            _operation: operation,
            directory: self.directory.clone(),
        })
    }
    pub fn startup_allowed(&self) -> Result<bool, Error> {
        let file = private_file(&self.directory.join("operation.lock"))?;
        try_lock(&file, libc::LOCK_EX)
    }
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }
}
impl UpdateGuard {
    /// Never kills a process. Keep the returned file alive across the transaction.
    pub fn wait_quiescent(&self, timeout: Duration) -> Result<File, Error> {
        let running = private_file(&self.directory.join("running.lock"))?;
        let deadline = Instant::now() + timeout;
        loop {
            if try_lock(&running, libc::LOCK_EX)? {
                return Ok(running);
            }
            if Instant::now() >= deadline {
                return Err(Error(
                    "RAD processes did not close; installation was not started".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
fn environment_gate() -> Result<SessionGate, Error> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| Error("XDG_RUNTIME_DIR is required for a RAD update".into()))?;
    SessionGate::at(&runtime)
}
/// Keep the returned guard alive until the normal process exits. A false result
/// means the update owns this session; do not start GUI, workers, data or hardware.
pub fn startup_guard() -> Result<Option<StartupGuard>, Error> {
    environment_gate()?.startup_guard()
}
/// Only an instantaneous check for an already-running process (e.g. tray action).
/// Entrypoints must use [`startup_guard`] instead to close the launch race.
pub fn startup_allowed() -> Result<bool, Error> {
    environment_gate()?.startup_allowed()
}
pub(crate) fn session_gate() -> Result<SessionGate, Error> {
    environment_gate()
}

fn try_lock(file: &File, kind: i32) -> Result<bool, Error> {
    // SAFETY: flock only borrows this live file descriptor.
    if unsafe { libc::flock(file.as_raw_fd(), kind | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(error.into())
    }
}
pub(crate) fn private_file(path: &Path) -> Result<File, Error> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err(Error("unsafe RAD update file".into()));
    }
    Ok(file)
}
pub(crate) fn private_directory(path: &Path) -> Result<(), Error> {
    if !path.is_absolute() {
        return Err(Error("RAD update paths must be absolute".into()));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::ParentDir | Component::CurDir) {
            return Err(Error("RAD update paths cannot traverse parents".into()));
        }
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::DirBuilder::new().mode(0o700).create(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
            }
            _ => {
                return Err(Error(
                    "RAD update directory contains an unsafe component".into(),
                ));
            }
        }
    }
    check_directory(path, true)
}
fn check_directory(path: &Path, private: bool) -> Result<(), Error> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(Error("invalid RAD update directory".into()));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        let metadata = std::fs::symlink_metadata(&current)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Error("RAD update directory contains a symlink".into()));
        }
    }
    let metadata = std::fs::symlink_metadata(path)?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if private && (metadata.uid() != uid || metadata.mode() & 0o077 != 0) {
        return Err(Error(
            "RAD update directory must be private and owned by this user".into(),
        ));
    }
    Ok(())
}
