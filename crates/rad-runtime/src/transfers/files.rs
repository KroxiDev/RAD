use super::Error;
use std::{
    fs::{self, File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
pub(super) struct Area {
    directory: File,
}
pub(super) struct Spool {
    pub file: File,
    path: PathBuf,
    dev: u64,
    ino: u64,
}
impl Area {
    pub fn recover(root: &Path) -> Result<Self, Error> {
        let before = fs::symlink_metadata(root).map_err(|_| Error::Io)?;
        if !before.is_dir() || before.mode() & 0o777 != 0o700 {
            return Err(Error::Io);
        }
        let directory = File::open(root).map_err(|_| Error::Io)?;
        let after = directory.metadata().map_err(|_| Error::Io)?;
        if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
            return Err(Error::Io);
        }
        let area = Self { directory };
        let mut pending = Vec::new();
        let mut packages = false;
        for (i, entry) in fs::read_dir(area.path())
            .map_err(|_| Error::Io)?
            .enumerate()
        {
            if i >= 3 {
                return Err(Error::Io);
            }
            let entry = entry.map_err(|_| Error::Io)?;
            let name = entry.file_name();
            let name = name.to_str().ok_or(Error::Io)?;
            if name == "packages" {
                packages = true;
                continue;
            }
            let suffix = name.strip_prefix("upload-").ok_or(Error::Io)?;
            if suffix.len() != 16
                || !suffix
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(Error::Io);
            }
            let m = fs::symlink_metadata(entry.path()).map_err(|_| Error::Io)?;
            if !m.is_file()
                || m.mode() & 0o777 != 0o600
                || m.uid() != after.uid()
                || m.len() > super::PACKAGE
            {
                return Err(Error::Io);
            }
            pending.push((entry.path(), m.dev(), m.ino()));
        }
        if !packages {
            return Err(Error::Io);
        }
        for (path, dev, ino) in pending {
            let m = fs::symlink_metadata(&path).map_err(|_| Error::Io)?;
            if !m.is_file() || (m.dev(), m.ino()) != (dev, ino) {
                return Err(Error::Io);
            }
            fs::remove_file(path).map_err(|_| Error::Io)?;
        }
        Ok(area)
    }

    pub fn create(root: &Path) -> Result<Self, Error> {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root)
            .map_err(|_| Error::Io)?;
        let directory = File::open(root).map_err(|_| Error::Io)?;
        Ok(Self { directory })
    }
    pub fn path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd()))
    }
    pub fn spool(&self, id: u64) -> Result<Spool, Error> {
        let path = self.path().join(format!("upload-{id:016x}"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| Error::Io)?;
        let m = file.metadata().map_err(|_| Error::Io)?;
        Ok(Spool {
            file,
            path,
            dev: m.dev(),
            ino: m.ino(),
        })
    }
}
impl Spool {
    pub fn remove(&self) -> Result<(), Error> {
        match fs::symlink_metadata(&self.path) {
            Ok(m) if m.is_file() && m.dev() == self.dev && m.ino() == self.ino => {
                fs::remove_file(&self.path).map_err(|_| Error::Io)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(Error::Io),
        }
    }
}
