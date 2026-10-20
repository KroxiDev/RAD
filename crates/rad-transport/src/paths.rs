use crate::TransportError as Error;
use std::os::unix::ffi::OsStrExt;
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};
pub(super) struct Instance {
    pub directory: File,
    _lock: File,
    sockets: Vec<(PathBuf, u64, u64)>,
}
fn private(file: &File, mode: u32, directory: bool) -> Result<(), Error> {
    let m = file.metadata().map_err(|_| Error::Io)?;
    // SAFETY:geteuid has no preconditions.
    if m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o7777 != mode
        || (directory && !m.is_dir())
        || (!directory && (!m.is_file() || m.nlink() != 1))
    {
        return Err(Error::UnsafePath);
    }
    Ok(())
}
fn child(dir: &File, name: &str, flags: i32, mode: u32) -> Result<File, Error> {
    let name = std::ffi::CString::new(name).map_err(|_| Error::UnsafePath)?;
    // SAFETY:live directory fd,NUL terminated name; returned fd uniquely owned.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            mode,
        )
    };
    if fd < 0 {
        return Err(Error::UnsafePath);
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
impl Instance {
    pub fn acquire(runtime: &Path) -> Result<Self, Error> {
        if !runtime.is_absolute()
            || runtime
                .join("rad/automation.sock")
                .as_os_str()
                .as_bytes()
                .len()
                >= 108
        {
            return Err(Error::UnsafePath);
        }
        let mut dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open("/")
            .map_err(|_| Error::Io)?;
        for component in runtime.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(s) => {
                    dir = child(
                        &dir,
                        s.to_str().ok_or(Error::UnsafePath)?,
                        libc::O_RDONLY | libc::O_DIRECTORY,
                        0,
                    )?;
                }
                _ => return Err(Error::UnsafePath),
            }
        }
        private(&dir, 0o700, true)?;
        let name = c"rad";
        // SAFETY:live fd and constant name. EEXIST is verified by openat below.
        let result = unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) };
        if result < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
            return Err(Error::Io);
        }
        let directory = child(&dir, "rad", libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
        private(&directory, 0o700, true)?;
        let lock = child(
            &directory,
            "instance.lock",
            libc::O_RDWR | libc::O_CREAT,
            0o600,
        )?;
        private(&lock, 0o600, false)?;
        // SAFETY:lock file fd remains owned for Instance lifetime.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
            return Err(
                if io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK) {
                    Error::InstanceBusy
                } else {
                    Error::Io
                },
            );
        }
        Ok(Self {
            directory,
            _lock: lock,
            sockets: Vec::new(),
        })
    }
    pub fn bind(&mut self, name: &str) -> Result<mio::net::UnixListener, Error> {
        let path = PathBuf::from(format!(
            "/proc/self/fd/{}/{}",
            self.directory.as_raw_fd(),
            name
        ));
        if let Ok(m) = fs::symlink_metadata(&path) {
            if !m.file_type().is_socket()
                || m.uid() != unsafe { libc::geteuid() }
                || m.mode() & 0o7777 != 0o600
            {
                return Err(Error::UnsafePath);
            }
            match mio::net::UnixStream::connect(&path) {
                Err(e) if e.raw_os_error() == Some(libc::ECONNREFUSED) => {
                    fs::remove_file(&path).map_err(|_| Error::Io)?
                }
                _ => return Err(Error::InstanceBusy),
            }
        }
        let socket = mio::net::UnixListener::bind(&path).map_err(|_| Error::Io)?;
        let m = fs::symlink_metadata(&path).map_err(|_| Error::Io)?;
        self.sockets.push((path.clone(), m.dev(), m.ino()));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|_| Error::Io)?;
        Ok(socket)
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        for (path, dev, ino) in &self.sockets {
            if fs::symlink_metadata(path)
                .is_ok_and(|m| m.file_type().is_socket() && m.dev() == *dev && m.ino() == *ino)
            {
                let _ = fs::remove_file(path);
            }
        }
    }
}
