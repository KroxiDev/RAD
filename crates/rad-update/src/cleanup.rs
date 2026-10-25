//! Conservative, bounded collection of updater-owned temporary directories only.
use crate::Error;
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CString, OsStr, OsString},
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt, process::CommandExt},
    },
    path::{Component, Path},
    process::Command,
};

const MAX_ENTRIES: usize = 4096;
const MAX_DIRECTORIES: usize = 64;
const MAX_FILES: usize = 8;
const MAX_MARKER_BYTES: u64 = 1024;
pub(crate) const OWNER: &str = ".owner.json";
pub(crate) const LEASE: &str = ".lease";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    Alive,
    Dead,
    Unknown,
}
pub trait Liveness {
    fn state(&self, pid: u32) -> ProcessState;
}
pub struct ProcessLiveness;
impl Liveness for ProcessLiveness {
    fn state(&self, pid: u32) -> ProcessState {
        let Ok(pid) = i32::try_from(pid) else {
            return ProcessState::Unknown;
        };
        if pid <= 0 {
            return ProcessState::Unknown;
        }
        // SAFETY: signal 0 checks existence/access without delivering a signal.
        if unsafe { libc::kill(pid, 0) } == 0 {
            ProcessState::Alive
        } else if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            ProcessState::Dead
        } else {
            ProcessState::Unknown
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub removed: usize,
    pub inspected: usize,
    pub limit_reached: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Fetch,
    Stage,
    Root,
    Config,
}
impl Kind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "fetch" => Some(Self::Fetch),
            "stage" => Some(Self::Stage),
            "root" => Some(Self::Root),
            "config" => Some(Self::Config),
            _ => None,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    schema: u32,
    uid: u32,
    kind: Kind,
    pid: u32,
    namespace: String,
    nonce: String,
}
/// PID values are only meaningful in this namespace; unknown identity always preserves data.
pub(crate) fn pid_namespace() -> Result<String, Error> {
    let namespace = File::open("/proc/self/ns/pid")?.metadata()?;
    if namespace.dev() == 0 || namespace.ino() == 0 {
        return Err(Error("PID namespace identity is unavailable".into()));
    }
    Ok(format!("{:016x}{:016x}", namespace.dev(), namespace.ino()))
}
fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}
fn name(value: &OsStr) -> Result<CString, Error> {
    if value.as_bytes().contains(&b'/') || value.is_empty() {
        return Err(Error("invalid anchored filename".into()));
    }
    CString::new(value.as_bytes()).map_err(|_| Error("invalid anchored filename".into()))
}
fn open_at(parent: &File, value: &OsStr, flags: i32, mode: u32) -> Result<File, Error> {
    let value = name(value)?;
    // SAFETY: parent is live, value is NUL-terminated, and returned ownership transfers once.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            value.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            mode,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fd is a new descriptor owned by this function.
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn entry_metadata(parent: &File, value: &OsStr) -> Result<libc::stat, Error> {
    let value = name(value)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: stat is writable; AT_SYMLINK_NOFOLLOW inspects only this anchored entry.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            value.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fstatat initialized the structure on success.
    Ok(unsafe { stat.assume_init() })
}
fn private_file(parent: &File, value: &OsStr, uid: u32) -> Result<File, Error> {
    let before = entry_metadata(parent, value)?;
    if before.st_mode & libc::S_IFMT != libc::S_IFREG
        || before.st_uid != uid
        || before.st_mode & 0o7777 != 0o600
        || before.st_nlink != 1
    {
        return Err(Error("unsafe updater temporary file".into()));
    }
    let file = open_at(parent, value, libc::O_RDWR | libc::O_NONBLOCK, 0)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != uid
        || meta.mode() & 0o7777 != 0o600
        || meta.nlink() != 1
        || before.st_dev != meta.dev()
        || before.st_ino != meta.ino()
    {
        return Err(Error("unsafe updater temporary file".into()));
    }
    Ok(file)
}
fn directory_at(parent: &File, value: &OsStr, uid: u32) -> Result<File, Error> {
    let before = entry_metadata(parent, value)?;
    if before.st_mode & libc::S_IFMT != libc::S_IFDIR
        || before.st_uid != uid
        || before.st_mode & 0o7777 != 0o700
    {
        return Err(Error("unsafe updater temporary directory".into()));
    }
    let directory = open_at(parent, value, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
    let meta = directory.metadata()?;
    if !meta.is_dir()
        || meta.uid() != uid
        || meta.mode() & 0o7777 != 0o700
        || before.st_dev != meta.dev()
        || before.st_ino != meta.ino()
    {
        return Err(Error("unsafe updater temporary directory".into()));
    }
    Ok(directory)
}
/// Traversal rejects symlinks and untrusted writable ancestors; only /tmp may be root-owned sticky/shared.
fn parent_directory(path: &Path, uid: u32) -> Result<File, Error> {
    if !path.is_absolute() || uid != current_uid() {
        return Err(Error(
            "cleanup requires the current UID and an absolute parent".into(),
        ));
    }
    let root = CString::new("/").expect("literal");
    // SAFETY: root is NUL-terminated and flags only open a directory.
    let fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fd is a new owned descriptor.
    let mut parent = unsafe { File::from_raw_fd(fd) };
    let mut traversed = std::path::PathBuf::from("/");
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(part) => {
                parent = open_at(&parent, part, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
                traversed.push(part);
                let meta = parent.metadata()?;
                let shared_tmp = traversed == Path::new("/tmp")
                    && meta.uid() == 0
                    && meta.mode() & 0o1777 == 0o1777;
                if !meta.is_dir()
                    || (!shared_tmp
                        && (meta.uid() != 0 && meta.uid() != uid || meta.mode() & 0o022 != 0))
                {
                    return Err(Error("untrusted temporary parent".into()));
                }
            }
            _ => return Err(Error("unsafe temporary parent components".into())),
        }
    }
    let meta = parent.metadata()?;
    let shared_tmp = path == Path::new("/tmp") && meta.uid() == 0 && meta.mode() & 0o1777 == 0o1777;
    if !shared_tmp && (meta.uid() != uid || meta.mode() & 0o7777 != 0o700) {
        return Err(Error("temporary parent must be private".into()));
    }
    Ok(parent)
}
/// An existing private directory, reached exclusively through anchored no-follow traversal.
pub(crate) fn private_directory_anchor(path: &Path, uid: u32) -> Result<File, Error> {
    let directory = parent_directory(path, uid)?;
    let meta = directory.metadata()?;
    if meta.uid() != uid || meta.mode() & 0o7777 != 0o700 {
        return Err(Error(
            "IPC parent must be private and owned by the current UID".into(),
        ));
    }
    Ok(directory)
}
fn parse_name(value: &OsStr, uid: u32) -> Option<(Kind, String)> {
    let value = value.to_str()?.strip_prefix("rad-update-")?;
    let (kind, value) = value.split_once('-')?;
    let kind = Kind::parse(kind)?;
    let (owner, nonce) = value.split_once('-')?;
    if owner != uid.to_string()
        || nonce.len() != 32
        || !nonce
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    Some((kind, nonce.into()))
}
fn owner(directory: &File, kind: Kind, nonce: &str, uid: u32) -> Result<Owner, Error> {
    let marker = private_file(directory, OsStr::new(OWNER), uid)?;
    if marker.metadata()?.len() > MAX_MARKER_BYTES {
        return Err(Error("oversized temporary owner marker".into()));
    }
    let value: Owner = serde_json::from_reader(marker.take(MAX_MARKER_BYTES + 1))
        .map_err(|_| Error("invalid temporary owner marker".into()))?;
    if value.schema != 1
        || value.uid != uid
        || value.kind != kind
        || value.nonce != nonce
        || value.pid == 0
        || value.pid > i32::MAX as u32
    {
        return Err(Error("temporary owner does not match its directory".into()));
    }
    Ok(value)
}
fn lock(file: &File, mode: i32) -> Result<(), Error> {
    // SAFETY: the descriptor is live and flock flags are valid.
    if unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
fn entries(directory: &File, limit: usize) -> Result<(Vec<OsString>, bool), Error> {
    // SAFETY: duplicate is owned here; fdopendir takes ownership on success only.
    let component = CString::new(".").expect("literal");
    let duplicate = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            component.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: duplicate is a live directory descriptor.
    let pointer = unsafe { libc::fdopendir(duplicate) };
    if pointer.is_null() {
        unsafe { libc::close(duplicate) };
        return Err(std::io::Error::last_os_error().into());
    }
    struct DirectoryStream(*mut libc::DIR);
    impl Drop for DirectoryStream {
        fn drop(&mut self) {
            // SAFETY: this stream was returned by fdopendir and closes once.
            unsafe { libc::closedir(self.0) };
        }
    }
    let stream = DirectoryStream(pointer);
    let mut names = Vec::new();
    loop {
        // SAFETY: stream is live and returned entry is copied before the next readdir.
        let entry = unsafe {
            *libc::__errno_location() = 0;
            libc::readdir(stream.0)
        };
        if entry.is_null() {
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error() == Some(0) {
                Ok((names, false))
            } else {
                Err(error.into())
            };
        }
        // SAFETY: d_name is NUL-terminated by readdir.
        let value = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if value == b"." || value == b".." {
            continue;
        }
        if names.len() >= limit {
            return Ok((names, true));
        }
        names.push(OsStr::from_bytes(value).to_owned());
    }
}
fn package_file(value: &str) -> bool {
    let Some(version) = value
        .strip_prefix("rust-action-deck-")
        .and_then(|value| value.strip_suffix("-x86_64.pkg.tar.zst"))
    else {
        return false;
    };
    let Some((version, revision)) = version.rsplit_once('-') else {
        return false;
    };
    let Ok(parsed) = semver::Version::parse(version) else {
        return false;
    };
    let Ok(revision_number) = revision.parse::<u32>() else {
        return false;
    };
    parsed.pre.is_empty()
        && parsed.build.is_empty()
        && parsed.to_string() == version
        && revision_number > 0
        && revision_number.to_string() == revision
}
fn known(kind: Kind, value: &str) -> bool {
    matches!(value, OWNER | LEASE)
        || match kind {
            Kind::Fetch => value == "response",
            Kind::Stage => value == "candidate.json" || package_file(value),
            Kind::Root => package_file(value),
            Kind::Config => matches!(value, "pacman.conf" | "00-rad-update-guard.hook"),
        }
}
fn validate_contents(
    directory: &File,
    kind: Kind,
    uid: u32,
    expected: Option<&[String]>,
) -> Result<Vec<(OsString, File)>, Error> {
    let (names, limited) = entries(directory, MAX_FILES)?;
    if limited
        || !names.iter().any(|value| value == OWNER)
        || !names.iter().any(|value| value == LEASE)
    {
        return Err(Error("unrecognized temporary contents".into()));
    }
    let mut packages = 0;
    let mut files = Vec::new();
    for value in names {
        let text = value
            .to_str()
            .ok_or_else(|| Error("unrecognized temporary filename".into()))?;
        if !known(kind, text)
            || expected.is_some_and(|expected| !expected.iter().any(|entry| entry == text))
        {
            return Err(Error("unknown temporary contents retained".into()));
        }
        if package_file(text) {
            packages += 1;
            if packages > 1 {
                return Err(Error("ambiguous temporary packages retained".into()));
            }
        }
        let file = private_file(directory, &value, uid)?;
        files.push((value, file));
    }
    Ok(files)
}
fn same_entry(parent: &File, value: &OsStr, file: &File) -> Result<bool, Error> {
    let value = name(value)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: stat points to writable storage and AT_SYMLINK_NOFOLLOW never follows the entry.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            value.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fstatat succeeded and initialized stat.
    let stat = unsafe { stat.assume_init() };
    let metadata = file.metadata()?;
    Ok(stat.st_dev == metadata.dev() && stat.st_ino == metadata.ino())
}
fn unlink(parent: &File, value: &OsStr, flags: i32) -> Result<(), Error> {
    let value = name(value)?;
    // SAFETY: parent is live and the name is one validated component.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), value.as_ptr(), flags) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
fn remove(
    parent: &File,
    value: &OsStr,
    directory: &File,
    files: Vec<(OsString, File)>,
) -> Result<(), Error> {
    if !same_entry(parent, value, directory)? {
        return Err(Error("temporary directory was replaced".into()));
    }
    for (name, file) in &files {
        if !same_entry(directory, name, file)? {
            return Err(Error("temporary file was replaced".into()));
        }
    }
    // Known data first, ownership/lease last. Unknown entries are never recursively removed.
    for (name, _) in files
        .iter()
        .filter(|(name, _)| name != OWNER && name != LEASE)
    {
        unlink(directory, name, 0)?;
    }
    unlink(directory, OsStr::new(OWNER), 0)?;
    unlink(directory, OsStr::new(LEASE), 0)?;
    if !same_entry(parent, value, directory)? {
        return Err(Error("temporary directory was renamed".into()));
    }
    unlink(parent, value, libc::AT_REMOVEDIR)
}
/// This API accepts an explicit synthetic parent for tests. Production only calls it for /tmp and its own UID.
pub fn collect_in(
    parent: &Path,
    uid: u32,
    liveness: &(impl Liveness + ?Sized),
) -> Result<Report, Error> {
    let namespace = pid_namespace()?;
    let parent = parent_directory(parent, uid)?;
    let (names, limited) = entries(&parent, MAX_ENTRIES)?;
    let mut report = Report {
        limit_reached: limited,
        ..Default::default()
    };
    for name in names {
        // Filter names before opening or statting anything outside this UID's exact namespace.
        let Some((kind, nonce)) = parse_name(&name, uid) else {
            continue;
        };
        if report.inspected >= MAX_DIRECTORIES {
            report.limit_reached = true;
            break;
        }
        report.inspected += 1;
        let attempt = (|| -> Result<(), Error> {
            let directory = directory_at(&parent, &name, uid)?;
            let owner = owner(&directory, kind, &nonce, uid)?;
            if owner.namespace != namespace {
                return Err(Error(
                    "temporary PID namespace is foreign or unknown".into(),
                ));
            }
            if liveness.state(owner.pid) != ProcessState::Dead {
                return Err(Error("temporary owner is live or unknown".into()));
            }
            let lease = private_file(&directory, OsStr::new(LEASE), uid)?;
            if lease.metadata()?.len() != 0 {
                return Err(Error("unexpected lease contents".into()));
            }
            lock(&lease, libc::LOCK_EX)?;
            let files = validate_contents(&directory, kind, uid, None)?;
            if liveness.state(owner.pid) != ProcessState::Dead {
                return Err(Error("temporary owner changed state".into()));
            }
            remove(&parent, &name, &directory, files)
        })();
        if attempt.is_ok() {
            report.removed += 1;
        }
    }
    Ok(report)
}
pub(crate) fn opportunistic() {
    let _ = collect_in(Path::new("/tmp"), current_uid(), &ProcessLiveness);
}

pub(crate) struct Lease {
    file: File,
    directory: File,
    parent: File,
    name: OsString,
    kind: Kind,
    uid: u32,
}
impl Lease {
    pub(crate) fn pin(path: &Path, uid: u32) -> Result<Self, Error> {
        let value = path
            .file_name()
            .ok_or_else(|| Error("invalid temporary directory name".into()))?;
        let (kind, nonce) = parse_name(value, uid)
            .ok_or_else(|| Error("not an updater temporary directory".into()))?;
        let parent = parent_directory(
            path.parent()
                .ok_or_else(|| Error("missing temporary parent".into()))?,
            uid,
        )?;
        let directory = directory_at(&parent, value, uid)?;
        owner(&directory, kind, &nonce, uid)?;
        let file = private_file(&directory, OsStr::new(LEASE), uid)?;
        if file.metadata()?.len() != 0 {
            return Err(Error("unexpected lease contents".into()));
        }
        lock(&file, libc::LOCK_SH)?;
        Ok(Self {
            file,
            directory,
            parent,
            name: value.to_owned(),
            kind,
            uid,
        })
    }
    pub(crate) fn inherit_to(&self, command: &mut Command) -> Result<(), Error> {
        let lease = self.file.try_clone()?;
        // SAFETY: the closure only uses async-signal-safe fcntl, with the cloned File
        // captured so the descriptor stays alive until exec. No explicit LOCK_UN is used.
        unsafe {
            command.pre_exec(move || {
                let fd = lease.as_raw_fd();
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
    pub(crate) fn create(&self, value: &str) -> Result<File, Error> {
        if !known(self.kind, value) || matches!(value, OWNER | LEASE) {
            return Err(Error("unknown temporary filename".into()));
        }
        open_at(
            &self.directory,
            OsStr::new(value),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )
    }
    pub(crate) fn open_file(&self, value: &str) -> Result<File, Error> {
        private_file(&self.directory, OsStr::new(value), self.uid)
    }
    pub(crate) fn discard(self, expected: &[String]) -> Result<(), Error> {
        let Self {
            file,
            directory,
            parent,
            name,
            kind,
            uid,
        } = self;
        // Closing only this descriptor preserves inherited SH locks in live consumers.
        drop(file);
        let lease = private_file(&directory, OsStr::new(LEASE), uid)?;
        lock(&lease, libc::LOCK_EX)?;
        let files = validate_contents(&directory, kind, uid, Some(expected))?;
        remove(&parent, &name, &directory, files)
    }
}
pub(crate) fn initialize(path: &Path, uid: u32, kind: &str, nonce: &str) -> Result<Lease, Error> {
    let kind = Kind::parse(kind).ok_or_else(|| Error("unknown temporary kind".into()))?;
    let value = path
        .file_name()
        .ok_or_else(|| Error("missing temporary directory".into()))?;
    if parse_name(value, uid) != Some((kind, nonce.into())) {
        return Err(Error("temporary name does not match ownership".into()));
    }
    let parent = parent_directory(
        path.parent()
            .ok_or_else(|| Error("missing temporary parent".into()))?,
        uid,
    )?;
    let directory = directory_at(&parent, value, uid)?;
    let file = open_at(
        &directory,
        OsStr::new(LEASE),
        libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        0o600,
    )?;
    lock(&file, libc::LOCK_SH)?;
    let marker = Owner {
        schema: 1,
        uid,
        kind,
        pid: std::process::id(),
        namespace: pid_namespace()?,
        nonce: nonce.into(),
    };
    let mut output = open_at(
        &directory,
        OsStr::new(OWNER),
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        0o600,
    )?;
    serde_json::to_writer(&mut output, &marker).map_err(|error| Error(error.to_string()))?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    directory.sync_all()?;
    Ok(Lease {
        file,
        directory,
        parent,
        name: value.to_owned(),
        kind,
        uid,
    })
}
