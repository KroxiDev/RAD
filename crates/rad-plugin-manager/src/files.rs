//! Descriptor-relative private files; no recursive deletion or package paths.
use crate::Error;
use std::{
    ffi::CString,
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path},
};
pub(crate) struct Directory(File);
fn owned(fd: i32) -> Result<File, Error> {
    if fd < 0 {
        return Err(Error::Io);
    } // SAFETY: successful open owns this new fd.
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn name(s: &str) -> Result<CString, Error> {
    if s.is_empty() || s.contains('/') || matches!(s, "." | "..") {
        return Err(Error::CorruptState);
    }
    CString::new(s).map_err(|_| Error::CorruptState)
}
impl Directory {
    pub fn open(path: &Path) -> Result<Self, Error> {
        if !path.is_absolute() {
            return Err(Error::Io);
        }
        // SAFETY: constant NUL-terminated path; owned wraps only successful fresh descriptors.
        let mut dir = Self(owned(unsafe {
            libc::open(
                c"/".as_ptr(),
                libc::O_DIRECTORY | libc::O_RDONLY | libc::O_CLOEXEC,
            )
        })?);
        for c in path.components() {
            match c {
                Component::RootDir => {}
                Component::Normal(s) => {
                    let m = dir.0.metadata()?;
                    if m.mode() & 0o022 != 0 && m.mode() & 0o1000 == 0 {
                        return Err(Error::Io);
                    }
                    let n = CString::new(s.as_bytes()).map_err(|_| Error::Io)?;
                    // SAFETY: live parent fd and CString component remain valid for openat.
                    dir = Self(owned(unsafe {
                        libc::openat(
                            dir.0.as_raw_fd(),
                            n.as_ptr(),
                            libc::O_DIRECTORY | libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                        )
                    })?);
                }
                _ => return Err(Error::Io),
            }
        }
        dir.private()?;
        // SAFETY: owned directory descriptor is live for the call.
        if unsafe { libc::flock(dir.0.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(Error::Busy);
        }
        Ok(dir)
    }
    fn private(&self) -> Result<(), Error> {
        let m = self.0.metadata()?;
        // SAFETY: geteuid has no memory or ownership preconditions.
        if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o7777 != 0o700 {
            return Err(Error::Io);
        }
        Ok(())
    }
    pub fn child(&self, s: &str) -> Result<Self, Error> {
        let n = name(s)?;
        // SAFETY: live parent fd and CString; returned fd is newly owned.
        let d = Self(owned(unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                n.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?);
        d.private()?;
        Ok(d)
    }
    pub fn mkdir(&self, s: &str) -> Result<Self, Error> {
        let n = name(s)?;
        // SAFETY: live parent fd and CString; mkdirat never follows a child link.
        if unsafe { libc::mkdirat(self.0.as_raw_fd(), n.as_ptr(), 0o700) } != 0 {
            return Err(Error::Io);
        }
        self.child(s)
    }
    pub fn names(&self) -> Result<Vec<String>, Error> {
        // /proc/self/fd is a kernel reference to our already-owned directory,
        // never a supplied path or a second resolution of the destination.
        let mut result = Vec::new();
        for entry in fs::read_dir(format!("/proc/self/fd/{}", self.0.as_raw_fd()))? {
            if result.len() >= 70 {
                return Err(Error::LimitExceeded);
            }
            result.push(
                entry?
                    .file_name()
                    .into_string()
                    .map_err(|_| Error::CorruptState)?,
            );
        }
        result.sort();
        Ok(result)
    }
    pub fn file(&self, s: &str) -> Result<File, Error> {
        let n = name(s)?;
        // SAFETY: live directory fd and CString; successful open returns a fresh fd.
        let f = owned(unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                n.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        })?;
        let m = f.metadata()?;
        if !m.is_file()
            || m.uid() != unsafe { libc::geteuid() }
            || m.nlink() != 1
            || m.mode() & 0o7077 != 0
        {
            return Err(Error::CorruptState);
        }
        Ok(f)
    }
    pub fn read(&self, s: &str, max: usize) -> Result<Vec<u8>, Error> {
        let f = self.file(s)?;
        if f.metadata()?.len() > max as u64 {
            return Err(Error::LimitExceeded);
        }
        let mut b = Vec::new();
        f.take(max as u64 + 1).read_to_end(&mut b)?;
        if b.len() > max {
            return Err(Error::LimitExceeded);
        }
        Ok(b)
    }
    pub fn write(&self, s: &str, b: &[u8], mode: u32) -> Result<(), Error> {
        let n = name(s)?;
        // SAFETY: live directory fd and CString; O_EXCL cannot overwrite existing entries.
        let mut f = owned(unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                n.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                mode,
            )
        })?;
        f.write_all(b)?;
        f.sync_all()?;
        Ok(())
    }
    pub fn sync(&self) -> Result<(), Error> {
        self.0.sync_all()?;
        Ok(())
    }
    pub fn publish(&self, from: &str, to: &str) -> Result<(), Error> {
        let a = name(from)?;
        let b = name(to)?;
        self.file(from)?;
        // SAFETY: live directory and names; NOREPLACE preserves any existing journal.
        if unsafe {
            libc::renameat2(
                self.0.as_raw_fd(),
                a.as_ptr(),
                self.0.as_raw_fd(),
                b.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(Error::Io);
        }
        Ok(())
    }
    pub fn replace(&self, from: &str, to: &str) -> Result<(), Error> {
        let a = name(from)?;
        let b = name(to)?;
        self.file(to)?;
        // SAFETY: both names and the owned parent fd remain live through renameat.
        if unsafe {
            libc::renameat(
                self.0.as_raw_fd(),
                a.as_ptr(),
                self.0.as_raw_fd(),
                b.as_ptr(),
            )
        } != 0
        {
            return Err(Error::Io);
        }
        Ok(())
    }
    pub fn unlink_file(&self, s: &str) -> Result<(), Error> {
        self.file(s)?;
        let n = name(s)?;
        // SAFETY: validated component and live directory fd; no recursive traversal.
        if unsafe { libc::unlinkat(self.0.as_raw_fd(), n.as_ptr(), 0) } != 0 {
            return Err(Error::Io);
        }
        Ok(())
    }
    pub fn remove_slot(&self, s: &str) -> Result<(), Error> {
        let d = self.child(s)?;
        let names = d.names()?;
        if names
            .iter()
            .any(|n| !matches!(n.as_str(), "manifest.json" | "plugin"))
        {
            return Err(Error::CorruptState);
        }
        for n in names {
            d.unlink_file(&n)?;
        }
        d.sync()?;
        let n = name(s)?;
        // SAFETY: validated component and live directory fd; no recursive traversal.
        if unsafe { libc::unlinkat(self.0.as_raw_fd(), n.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
            return Err(Error::Io);
        }
        self.sync()
    }
}
