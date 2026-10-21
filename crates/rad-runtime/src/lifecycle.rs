//! Process entry policy; no authority, storage or device logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub help: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleError {
    Arguments,
    Environment,
    Path,
    Signals,
}
impl Options {
    pub fn parse(
        args: impl IntoIterator<Item = std::ffi::OsString>,
    ) -> Result<Self, LifecycleError> {
        let mut options = Self::default();
        for arg in args {
            match arg.to_str() {
                Some("--help") if !options.help => options.help = true,
                _ => return Err(LifecycleError::Arguments),
            }
        }
        Ok(options)
    }
}

/// Builds the inherited-session helper command; never invokes a shell.
pub fn helper_command(path: &std::path::Path) -> std::process::Command {
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new(path);
    // SAFETY: the post-fork closure uses only async-signal-safe libc signal
    // operations and constructs an OS error without allocation on failure.
    unsafe {
        command.pre_exec(|| {
            let mut signals = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut signals);
            libc::sigaddset(&mut signals, libc::SIGINT);
            libc::sigaddset(&mut signals, libc::SIGTERM);
            if libc::sigprocmask(libc::SIG_UNBLOCK, &signals, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}

#[derive(Debug, PartialEq, Eq)]
pub struct Paths {
    pub runtime: std::path::PathBuf,
    pub data: std::path::PathBuf,
    pub helper: std::path::PathBuf,
}
impl Paths {
    pub fn from_values(
        runtime: Option<&std::ffi::OsStr>,
        data: Option<&std::ffi::OsStr>,
        home: Option<&std::ffi::OsStr>,
        executable: &std::path::Path,
    ) -> Result<Self, LifecycleError> {
        use std::path::{Component, Path};
        fn absolute(path: &Path) -> Result<(), LifecycleError> {
            if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
                return Err(LifecycleError::Environment);
            }
            Ok(())
        }
        let runtime = Path::new(runtime.ok_or(LifecycleError::Environment)?).to_path_buf();
        let data = match data.filter(|v| !v.is_empty()) {
            Some(value) => Path::new(value).join("rad"),
            None => Path::new(home.ok_or(LifecycleError::Environment)?).join(".local/share/rad"),
        };
        absolute(&runtime)?;
        absolute(&data)?;
        absolute(executable)?;
        let helper = executable
            .parent()
            .ok_or(LifecycleError::Environment)?
            .join("rad-launcher-helper");
        Ok(Self {
            runtime,
            data,
            helper,
        })
    }
}
impl Paths {
    /// Rejects symlinks in all components; existing parent permissions are unchanged.
    /// This is not a security boundary against concurrent same-UID OS access.
    pub fn prepare(&self) -> Result<(), LifecycleError> {
        self.prepare_runtime()?;
        self.prepare_data()
    }
    /// Apart from the data directory because the exit classes differ (ADR 0011): an unsafe
    /// runtime directory blocks any restart, an unsafe data directory does not.
    pub fn prepare_runtime(&self) -> Result<(), LifecycleError> {
        check_directories(&self.runtime, false)?;
        check_private(&self.runtime)
    }
    pub fn prepare_data(&self) -> Result<(), LifecycleError> {
        check_directories(&self.data, true)?;
        check_private(&self.data)
    }
}

fn check_directories(path: &std::path::Path, create: bool) -> Result<(), LifecycleError> {
    use std::{
        fs,
        os::unix::fs::DirBuilderExt,
        path::{Component, PathBuf},
    };
    if !path.is_absolute() {
        return Err(LifecycleError::Path);
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        if matches!(part, Component::ParentDir) {
            return Err(LifecycleError::Path);
        }
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&current)
                    .map_err(|_| LifecycleError::Path)?;
            }
            _ => return Err(LifecycleError::Path),
        }
    }
    Ok(())
}
fn check_private(path: &std::path::Path) -> Result<(), LifecycleError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| LifecycleError::Path)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(LifecycleError::Path);
    }
    Ok(())
}

/// Install on the main thread BEFORE creating Server, workers, or helpers.
/// Kept !Send/!Sync because the saved signal mask belongs to this thread.
pub struct ShutdownSignals {
    fd: std::os::fd::OwnedFd,
    original: libc::sigset_t,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl ShutdownSignals {
    pub fn install() -> Result<Self, LifecycleError> {
        use std::os::fd::FromRawFd;
        // SAFETY: both sigsets are initialized before use; fd ownership transfers once.
        unsafe {
            let mut mask = std::mem::zeroed::<libc::sigset_t>();
            let mut original = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGINT);
            libc::sigaddset(&mut mask, libc::SIGTERM);
            if libc::pthread_sigmask(libc::SIG_BLOCK, &mask, &mut original) != 0 {
                return Err(LifecycleError::Signals);
            }
            let fd = libc::signalfd(-1, &mask, libc::SFD_NONBLOCK | libc::SFD_CLOEXEC);
            if fd < 0 {
                libc::pthread_sigmask(libc::SIG_SETMASK, &original, std::ptr::null_mut());
                return Err(LifecycleError::Signals);
            }
            Ok(Self {
                fd: std::os::fd::OwnedFd::from_raw_fd(fd),
                original,
                _thread: std::marker::PhantomData,
            })
        }
    }
    /// Consumes at most the two standard shutdown signals; repeated requests coalesce.
    pub fn take_shutdown(&self) -> Result<bool, LifecycleError> {
        use std::os::fd::AsRawFd;
        let mut shutdown = false;
        for _ in 0..2 {
            let mut info = std::mem::MaybeUninit::<libc::signalfd_siginfo>::uninit();
            // SAFETY: writable buffer has exactly the supplied size; contents not inspected.
            let count = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    info.as_mut_ptr().cast(),
                    std::mem::size_of::<libc::signalfd_siginfo>(),
                )
            };
            if count == std::mem::size_of::<libc::signalfd_siginfo>() as isize {
                shutdown = true;
            } else if count < 0 {
                match std::io::Error::last_os_error().kind() {
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted => break,
                    _ => return Err(LifecycleError::Signals),
                }
            } else {
                return Err(LifecycleError::Signals);
            }
        }
        Ok(shutdown)
    }
}
impl std::os::fd::AsRawFd for ShutdownSignals {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        std::os::fd::AsRawFd::as_raw_fd(&self.fd)
    }
}

impl Drop for ShutdownSignals {
    fn drop(&mut self) {
        // SAFETY: !Send guard restores the mask on the same thread that installed it.
        unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &self.original, std::ptr::null_mut());
        }
    }
}
