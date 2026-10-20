use crate::Error;
use std::{
    ffi::{CStr, CString},
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::{Component, Path},
};
use zeroize::Zeroizing;
fn last() -> Error {
    match io::Error::last_os_error().raw_os_error() {
        Some(libc::ELOOP | libc::ENOTDIR) => Error::UnsafePath,
        _ => Error::Io,
    }
}
fn name(s: &str) -> Result<CString, Error> {
    CString::new(s).map_err(|_| Error::UnsafePath)
}
pub fn private(f: &File, dir: bool) -> Result<(), Error> {
    let m = f.metadata().map_err(|_| Error::Io)?;
    // SAFETY: geteuid has no preconditions.
    if m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o7777 != if dir { 0o700 } else { 0o600 }
        || if dir {
            !m.is_dir()
        } else {
            !m.is_file() || m.nlink() != 1
        }
    {
        return Err(Error::UnsafePath);
    }
    Ok(())
}
pub fn child(dir: &File, s: &str, flags: i32) -> Result<Option<File>, Error> {
    let n = name(s)?;
    // SAFETY: live parent descriptor and NUL terminated component; returned descriptor is uniquely owned.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            n.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        if io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        return Err(last());
    }
    Ok(Some(unsafe { File::from_raw_fd(fd) }))
}
pub fn directory(parent: &File, s: &str, create: bool) -> Result<Option<File>, Error> {
    if create {
        let n = name(s)?;
        // SAFETY: live fd and component.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), n.as_ptr(), 0o700) } == 0 {
            parent.sync_all().map_err(|_| Error::Io)?;
        } else if io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
            return Err(last());
        }
    }
    let f = child(parent, s, libc::O_RDONLY | libc::O_DIRECTORY)?;
    if let Some(f) = &f {
        private(f, true)?;
        if create {
            parent.sync_all().map_err(|_| Error::Io)?;
        }
    }
    Ok(f)
}
pub fn root(path: &Path) -> Result<File, Error> {
    if !path.is_absolute() {
        return Err(Error::UnsafePath);
    }
    let mut d = File::open("/").map_err(|_| Error::Io)?;
    let parts: Vec<_> = path.components().collect();
    for (i, c) in parts.iter().enumerate() {
        match c {
            Component::RootDir => {}
            Component::Normal(n) => {
                let s = n.to_str().ok_or(Error::UnsafePath)?;
                d = if i == parts.len() - 1 {
                    directory(&d, s, true)?.ok_or(Error::Io)?
                } else {
                    child(&d, s, libc::O_RDONLY | libc::O_DIRECTORY)?.ok_or(Error::Io)?
                };
            }
            _ => return Err(Error::UnsafePath),
        }
    }
    private(&d, true)?;
    Ok(d)
}
pub fn read(
    dir: &File,
    s: &str,
    min: usize,
    max: usize,
) -> Result<Option<Zeroizing<Vec<u8>>>, Error> {
    let Some(f) = child(dir, s, libc::O_RDONLY)? else {
        return Ok(None);
    };
    private(&f, false)?;
    let len = f.metadata().map_err(|_| Error::Io)?.len();
    if len < min as u64 || len > max as u64 {
        return Err(Error::Unavailable);
    }
    let mut b = Zeroizing::new(Vec::with_capacity(len as usize));
    f.take(max as u64 + 1)
        .read_to_end(&mut b)
        .map_err(|_| Error::Io)?;
    if b.len() != len as usize {
        return Err(Error::Unavailable);
    }
    Ok(Some(b))
}
pub fn entries(dir: &File) -> Result<Vec<String>, Error> {
    let f = child(dir, ".", libc::O_RDONLY | libc::O_DIRECTORY)?.ok_or(Error::Io)?;
    use std::os::fd::IntoRawFd;
    let fd = f.into_raw_fd();
    // SAFETY: fd is transferred to DIR; closed below on all paths.
    let p = unsafe { libc::fdopendir(fd) };
    if p.is_null() {
        unsafe { libc::close(fd) };
        return Err(Error::Io);
    }
    let mut out = Vec::new();
    let result = loop {
        // SAFETY: thread local errno reset, valid DIR, entry valid until next readdir.
        unsafe { *libc::__errno_location() = 0 };
        let e = unsafe { libc::readdir(p) };
        if e.is_null() {
            break if io::Error::last_os_error().raw_os_error() == Some(0) {
                Ok(out)
            } else {
                Err(Error::Io)
            };
        }
        let s = unsafe { CStr::from_ptr((*e).d_name.as_ptr()) };
        match s.to_str() {
            Ok("." | "..") => {}
            Ok(s) => out.push(s.to_owned()),
            Err(_) => break Err(Error::UnsafePath),
        }
    };
    unsafe { libc::closedir(p) };
    result
}
pub fn unlink(dir: &File, s: &str) -> Result<(), Error> {
    let n = name(s)?;
    // SAFETY: live dir and component.
    if unsafe { libc::unlinkat(dir.as_raw_fd(), n.as_ptr(), 0) } < 0
        && io::Error::last_os_error().raw_os_error() != Some(libc::ENOENT)
    {
        return Err(last());
    }
    Ok(())
}
pub fn publish(dir: &File, dest: &str, bytes: &[u8], exclusive: bool) -> Result<(), Error> {
    #[cfg(test)]
    fail(Fault::Rng)?;
    let mut random = [0; 16];
    self::random(&mut random)?;
    #[cfg(test)]
    TEMP_RANDOM.with(|value| {
        if let Some(bytes) = value.take() {
            random = bytes;
        }
    });
    let temp = format!(
        ".pending-{}",
        random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let mut owned = false;
    let result = (|| {
        let mut f =
            child(dir, &temp, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?.ok_or(Error::Io)?;
        owned = true;
        private(&f, false)?;
        #[cfg(test)]
        fail(Fault::Write)?;
        #[cfg(test)]
        if FAULT.with(|fault| fault.get() == Some(Fault::PartialWrite)) {
            f.write_all(&bytes[..bytes.len() / 2])
                .map_err(|_| Error::Io)?;
            fail(Fault::PartialWrite)?;
        }
        f.write_all(bytes).map_err(|_| Error::Io)?;
        #[cfg(test)]
        fail(Fault::FileSync)?;
        f.sync_all().map_err(|_| Error::Io)?;
        let src = name(&temp)?;
        let dst = name(dest)?;
        // SAFETY: both names in held directory. NOREPLACE protects an existing master key.
        #[cfg(test)]
        fail(Fault::Rename)?;
        let r = unsafe {
            libc::renameat2(
                dir.as_raw_fd(),
                src.as_ptr(),
                dir.as_raw_fd(),
                dst.as_ptr(),
                if exclusive { libc::RENAME_NOREPLACE } else { 0 },
            )
        };
        if r < 0 {
            if exclusive && io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST) {
                return Ok(());
            }
            return Err(Error::Uncertain);
        }
        #[cfg(test)]
        fail(Fault::DirectorySync)?;
        dir.sync_all().map_err(|_| Error::Uncertain)
    })();
    if owned {
        let _ = unlink(dir, &temp);
    }
    result
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    Rng,
    Write,
    PartialWrite,
    FileSync,
    Rename,
    DirectorySync,
}
#[cfg(test)]
thread_local! {pub(crate) static FAULT:std::cell::Cell<Option<Fault>>=const{std::cell::Cell::new(None)};}
#[cfg(test)]
fn fail(at: Fault) -> Result<(), Error> {
    FAULT.with(|v| {
        if v.get() == Some(at) {
            v.set(None);
            Err(if matches!(at, Fault::Rename | Fault::DirectorySync) {
                Error::Uncertain
            } else {
                Error::Io
            })
        } else {
            Ok(())
        }
    })
}
pub fn random(bytes: &mut [u8]) -> Result<(), Error> {
    #[cfg(test)]
    fail(Fault::Rng)?;
    getrandom::fill(bytes).map_err(|_| Error::Unavailable)
}
pub fn sync_directory(dir: &File) -> Result<(), Error> {
    #[cfg(test)]
    fail(Fault::DirectorySync)?;
    dir.sync_all().map_err(|_| Error::Uncertain)
}
pub fn sync_file(file: &File) -> Result<(), Error> {
    #[cfg(test)]
    fail(Fault::FileSync)?;
    file.sync_all().map_err(|_| Error::Io)
}

#[cfg(test)]
thread_local! {pub(crate) static TEMP_RANDOM:std::cell::Cell<Option<[u8;16]>>=const{std::cell::Cell::new(None)};}
