use crate::PackageError as Error;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    os::fd::AsRawFd,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
struct Entry {
    path: PathBuf,
    dev: u64,
    ino: u64,
    bytes: u64,
    live: bool,
}
struct State {
    next: u64,
    files: BTreeMap<u64, Entry>,
}
struct Inner {
    directory: File,
    state: Mutex<State>,
}
/// Create in a trusted parent selected by the installation adapter, never from package data.
/// One area per runtime; reservations include failed cleanups and are shared across workers.
/// Linux /proc/self/fd anchors operations to the opened directory even after rename.
/// The trusted parent and runtime lock are adapter preconditions, not package inputs.
/// A startup cleanup failure must block admission; never bypass it with another area.
#[derive(Clone)]
pub struct StagingArea {
    inner: Arc<Inner>,
}
impl StagingArea {
    pub fn create_new(root: &Path) -> Result<Self, Error> {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root)
            .map_err(|_| Error::UnsafeStaging)?;
        Ok(Self {
            inner: Arc::new(Inner {
                directory: open_directory(root)?,
                state: Mutex::new(State {
                    next: 0,
                    files: BTreeMap::new(),
                }),
            }),
        })
    }
    /// Caller must hold the exclusive runtime lock and recover Store first.
    /// Never resumes or publishes abandoned transfers. Unknown material is preserved.
    pub fn recover_abandoned(root: &Path) -> Result<Self, Error> {
        let directory = open_directory(root)?;
        let anchored = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        let owner = directory.metadata().map_err(|_| Error::Io)?.uid();
        let mut state = State {
            next: 0,
            files: BTreeMap::new(),
        };
        // Validate the complete bounded inventory before deleting anything.
        for entry in fs::read_dir(&anchored).map_err(|_| Error::Io)? {
            let entry = entry.map_err(|_| Error::Io)?;
            let name = entry.file_name();
            let name = name.to_str().ok_or(Error::UnsafeStaging)?;
            let suffix = name.strip_prefix("transfer-").ok_or(Error::UnsafeStaging)?;
            if suffix.len() != 16
                || !suffix
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || state.files.len() >= 2
            {
                return Err(Error::UnsafeStaging);
            }
            let id = u64::from_str_radix(suffix, 16).map_err(|_| Error::UnsafeStaging)?;
            let path = anchored.join(name);
            let metadata = fs::symlink_metadata(&path).map_err(|_| Error::Io)?;
            if !metadata.is_file()
                || metadata.mode() & 0o777 != 0o600
                || metadata.uid() != owner
                || metadata.len() > super::format::PACKAGE
            {
                return Err(Error::UnsafeStaging);
            }
            state.next = state.next.max(id.checked_add(1).ok_or(Error::Capacity)?);
            state.files.insert(
                id,
                Entry {
                    path,
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                    bytes: metadata.len(),
                    live: false,
                },
            );
        }
        let area = Self {
            inner: Arc::new(Inner {
                directory,
                state: Mutex::new(state),
            }),
        };
        area.reap()?;
        Ok(area)
    }
    pub fn usage(&self) -> Result<(usize, u64), Error> {
        let s = self.inner.state.lock().map_err(|_| Error::Io)?;
        Ok((s.files.len(), s.files.values().map(|e| e.bytes).sum()))
    }
    pub fn reap(&self) -> Result<(), Error> {
        let mut s = self.inner.state.lock().map_err(|_| Error::Io)?;
        let ids: Vec<_> = s
            .files
            .iter()
            .filter(|(_, e)| !e.live)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            remove(&mut s, id)?;
        }
        Ok(())
    }
    pub(crate) fn reserve(&self, bytes: u64) -> Result<StageFile, Error> {
        if bytes > super::format::PACKAGE {
            return Err(Error::LimitExceeded);
        }
        let mut s = self.inner.state.lock().map_err(|_| Error::Io)?;
        if s.files.len() >= 2
            || s.files.values().map(|e| e.bytes).sum::<u64>() + bytes > 512 * 1024 * 1024
        {
            return Err(Error::Capacity);
        }
        let id = s.next;
        s.next = s.next.checked_add(1).ok_or(Error::Capacity)?;
        let path = PathBuf::from(format!(
            "/proc/self/fd/{}/transfer-{id:016x}",
            self.inner.directory.as_raw_fd()
        ));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| Error::Io)?;
        let m = file.metadata().map_err(|_| Error::Io)?;
        s.files.insert(
            id,
            Entry {
                path,
                dev: m.dev(),
                ino: m.ino(),
                bytes,
                live: true,
            },
        );
        Ok(StageFile {
            file: Some(file),
            area: self.clone(),
            id: Some(id),
        })
    }
}
fn open_directory(root: &Path) -> Result<File, Error> {
    let before = fs::symlink_metadata(root).map_err(|_| Error::UnsafeStaging)?;
    if !before.is_dir() || before.mode() & 0o777 != 0o700 {
        return Err(Error::UnsafeStaging);
    }
    let file = File::open(root).map_err(|_| Error::UnsafeStaging)?;
    let after = file.metadata().map_err(|_| Error::UnsafeStaging)?;
    if before.dev() != after.dev() || before.ino() != after.ino() {
        return Err(Error::UnsafeStaging);
    }
    Ok(file)
}
fn remove(s: &mut State, id: u64) -> Result<(), Error> {
    let Some(e) = s.files.get(&id) else {
        return Ok(());
    };
    match fs::symlink_metadata(&e.path) {
        Ok(m) if m.is_file() && m.dev() == e.dev && m.ino() == e.ino => {
            fs::remove_file(&e.path).map_err(|_| Error::Io)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err(Error::UnsafeStaging),
    }
    s.files.remove(&id);
    Ok(())
}
pub(crate) struct StageFile {
    file: Option<File>,
    area: StagingArea,
    id: Option<u64>,
}
impl StageFile {
    pub fn file(&mut self) -> &mut File {
        self.file.as_mut().expect("live staging handle")
    }
    pub fn discard(mut self) -> Result<(), Error> {
        self.release()
    }
    fn release(&mut self) -> Result<(), Error> {
        drop(self.file.take());
        if let Some(id) = self.id.take() {
            let mut s = self.area.inner.state.lock().map_err(|_| Error::Io)?;
            if let Some(e) = s.files.get_mut(&id) {
                e.live = false;
            }
            remove(&mut s, id)?;
        }
        Ok(())
    }
}
impl Drop for StageFile {
    fn drop(&mut self) {
        let _ = self.release();
    }
}
