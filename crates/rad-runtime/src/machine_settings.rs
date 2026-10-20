//! Machine-local options. Phase 1 stores them without changing device/service policy.
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MachineSettings {
    pub use_stream_deck: bool,
    pub start_with_session: bool,
}
impl MachineSettings {
    /// The directory is the runtime's private local data directory, not the Store.
    pub fn load(directory: &Path) -> io::Result<Self> {
        let path = directory.join("machine.json");
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Ok(m) if m.is_file() && m.len() <= 4096 => {}
            Ok(_) => return Err(invalid()),
            Err(e) => return Err(e),
        }
        let mut bytes = Vec::new();
        fs::File::open(path)?.take(4097).read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err(invalid());
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if value["version"] != 1 || value.as_object().is_none_or(|o| o.len() != 3) {
            return Err(invalid());
        }
        Ok(Self {
            use_stream_deck: value["use_stream_deck"].as_bool().ok_or_else(invalid)?,
            start_with_session: value["start_with_session"].as_bool().ok_or_else(invalid)?,
        })
    }
    pub fn save(self, directory: &Path) -> io::Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        if !directory.is_absolute() {
            return Err(invalid());
        }
        static NEXT_TEMPORARY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let (mut file, mut temporary) = loop {
            let nonce = NEXT_TEMPORARY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let temporary = directory.join(format!("machine.{}.{nonce}.tmp", std::process::id()));
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
        let bytes = serde_json::to_vec(
            &serde_json::json!({"version":1,"use_stream_deck":self.use_stream_deck,"start_with_session":self.start_with_session}),
        )?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(
            temporary.0.as_ref().expect("unpublished temporary"),
            directory.join("machine.json"),
        )?;
        temporary.0 = None;
        fs::File::open(directory)?.sync_all()
    }
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid machine settings")
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
