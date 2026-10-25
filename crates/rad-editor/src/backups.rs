//! Local export files. Created by the client worker, never on the GUI thread.
use std::{
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub struct ExportFile {
    file: File,
    path: PathBuf,
    complete: bool,
}
impl ExportFile {
    pub fn at(path: PathBuf) -> io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        Ok(Self {
            file,
            path,
            complete: false,
        })
    }
    pub fn automatic(home: &Path, at: SystemTime) -> io::Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !home.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "HOME must be absolute",
            ));
        }
        let stamp = at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid export time"))?;
        let directory = home.join("RAD Backups");
        match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = std::fs::symlink_metadata(&directory)?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "backup directory must be private and not a symlink",
            ));
        }
        for suffix in 0..256 {
            let path = directory.join(format!(
                "rad-backup-{}-{:09}-{suffix}.rad",
                stamp.as_secs(),
                stamp.subsec_nanos()
            ));
            match Self::at(path) {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                result => return result,
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no free backup filename",
        ))
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn finish(mut self) -> io::Result<PathBuf> {
        self.file.sync_all()?;
        File::open(
            self.path
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?
        .sync_all()?;
        self.complete = true;
        Ok(self.path.clone())
    }
}
impl Write for ExportFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
impl Drop for ExportFile {
    fn drop(&mut self) {
        if !self.complete {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
