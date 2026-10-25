//! Local package ownership and bounded, shell-free administrative queries.
use crate::{Cancellation, Error};
use std::{
    io::Read,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}
pub(crate) fn command(program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    command
}
fn nonblocking(stream: &impl AsRawFd) -> Result<(), Error> {
    // SAFETY: fcntl only changes flags on the owned, live descriptor.
    unsafe {
        let flags = libc::fcntl(stream.as_raw_fd(), libc::F_GETFL);
        if flags < 0 || libc::fcntl(stream.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}
fn drain(stream: &mut impl Read, kept: &mut Vec<u8>, limit: usize) -> Result<(), Error> {
    let mut block = [0; 8192];
    let mut drained = 0;
    while drained < 65536 {
        match stream.read(&mut block) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                drained += n;
                let retain = n.min(limit.saturating_sub(kept.len()));
                kept.extend_from_slice(&block[..retain]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn reap_error(child: &mut std::process::Child, cancellable: bool, error: Error) -> Error {
    if cancellable {
        let _ = child.kill();
    }
    let _ = child.wait();
    error
}
/// `None` deliberately means a transaction, which must be waited for rather than killed.
pub(crate) fn capture(
    mut command: Command,
    timeout: Option<Duration>,
    cancel: &Cancellation,
    output_limit: usize,
) -> Result<Output, Error> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error("missing stdout".into()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error("missing stderr".into()))?;
    if let Err(error) = nonblocking(&stdout).and_then(|_| nonblocking(&stderr)) {
        return Err(reap_error(&mut child, timeout.is_some(), error));
    }
    let started = Instant::now();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    loop {
        if let Err(error) = drain(&mut stdout, &mut out, output_limit)
            .and_then(|_| drain(&mut stderr, &mut err, output_limit))
        {
            return Err(reap_error(&mut child, timeout.is_some(), error));
        }
        if timeout.is_some() && (out.len() >= output_limit || err.len() >= output_limit) {
            return Err(reap_error(
                &mut child,
                true,
                Error("process output limit exceeded".into()),
            ));
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => return Err(reap_error(&mut child, timeout.is_some(), error.into())),
        };
        if let Some(status) = status {
            drain(&mut stdout, &mut out, output_limit)?;
            drain(&mut stderr, &mut err, output_limit)?;
            return Ok(Output {
                code: status.code(),
                stdout: String::from_utf8_lossy(&out).into_owned(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
            });
        }
        if timeout.is_some_and(|limit| started.elapsed() >= limit)
            || (timeout.is_some() && cancel.is_cancelled())
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error(
                if cancel.is_cancelled() {
                    "cancelled"
                } else {
                    "process deadline exceeded"
                }
                .into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
pub(crate) fn query(program: &str, args: &[&str]) -> Result<Output, Error> {
    let mut process = command(program);
    process.args(args);
    capture(
        process,
        Some(Duration::from_secs(10)),
        &Cancellation::default(),
        65536,
    )
}
/// Uses the Arch comparator, including epoch and package revision, never lexical order.
pub fn compare_versions(left: &str, right: &str) -> Result<std::cmp::Ordering, Error> {
    if [left, right].iter().any(|value| {
        value.is_empty()
            || value.len() > 128
            || value.starts_with('-')
            || value
                .chars()
                .any(|c| !c.is_ascii_alphanumeric() && !".+_:-".contains(c))
    }) {
        return Err(Error("invalid package version".into()));
    }
    let output = query("/usr/bin/vercmp", &[left, right])?;
    if output.code != Some(0) {
        return Err(Error("Arch version comparison failed".into()));
    }
    let value = output
        .stdout
        .trim()
        .parse::<i32>()
        .map_err(|_| Error("invalid Arch comparison result".into()))?;
    Ok(value.cmp(&0))
}
/// The installed system package, not the version claimed by a user-supplied candidate.
pub fn installed_version() -> Result<String, Error> {
    let package = crate::current_release()?.package;
    let output = query("/usr/bin/pacman", &["-Q", "--", &package])?;
    if output.code != Some(0) {
        return Err(Error("official package is not installed".into()));
    }
    let mut fields = output.stdout.split_whitespace();
    let name = fields
        .next()
        .ok_or_else(|| Error("missing package identity".into()))?;
    let version = fields
        .next()
        .ok_or_else(|| Error("missing installed version".into()))?;
    if name != package || fields.next().is_some() {
        return Err(Error("unexpected installed package identity".into()));
    }
    compare_versions(version, version)?;
    Ok(version.into())
}
/// A development checkout must never replace the system installation.
pub fn managed() -> Result<bool, Error> {
    let executable = std::env::current_exe()?;
    if executable != Path::new("/usr/lib/rad/rad-editor") {
        return Ok(false);
    }
    let canonical = std::fs::canonicalize(&executable)?;
    if canonical != executable {
        return Ok(false);
    }
    let output = query(
        "/usr/bin/pacman",
        &["-Qoq", "--", "/usr/lib/rad/rad-editor"],
    )?;
    Ok(output.code == Some(0) && output.stdout.trim() == crate::current_release()?.package)
}
/// Refuses symlink final components and opens only a regular private file owned by `uid`.
pub(crate) fn private_file(path: &Path, uid: u32) -> Result<std::fs::File, Error> {
    use std::os::unix::fs::MetadataExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != uid || meta.mode() & 0o077 != 0 || meta.nlink() != 1 {
        return Err(Error("unsafe staging file".into()));
    }
    Ok(file)
}
