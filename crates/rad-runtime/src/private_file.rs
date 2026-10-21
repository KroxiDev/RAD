//! Small private files of the runtime's data directory: bounded reads, atomic 0600 writes.
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub(crate) fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid private file")
}

/// `Ok(None)` when absent. A symlink, a non-regular file or more than `limit` bytes is invalid.
pub(crate) fn read_bounded(path: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Ok(m) if m.is_file() && m.len() <= limit => {}
        Ok(_) => return Err(invalid()),
        Err(e) => return Err(e),
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid());
    }
    Ok(Some(bytes))
}

/// Publishes `bytes` as `directory/name` with mode 0600: a synced staging file, then rename.
pub(crate) fn write_atomic(directory: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if !directory.is_absolute() {
        return Err(invalid());
    }
    static NEXT_TEMPORARY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (mut file, mut temporary) = loop {
        let nonce = NEXT_TEMPORARY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temporary = directory.join(format!("{name}.{}.{nonce}.tmp", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
        {
            Ok(file) => break (file, PendingFile(Some(temporary))),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(
        temporary.0.as_ref().expect("unpublished temporary"),
        directory.join(name),
    )?;
    temporary.0 = None;
    fs::File::open(directory)?.sync_all()
}

// Own only the file created by this attempt, never an existing collision or the published path.
struct PendingFile(Option<PathBuf>);
impl Drop for PendingFile {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}
