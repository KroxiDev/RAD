//! Bounded HTTPS subprocesses, private staging and complete-artifact verification.
use crate::{Cancellation, Error, Progress, ReleaseMetadata};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

/// Every initial URL and every redirect passes through this allowlist.
pub fn validate_destination(url: &str) -> Result<(), Error> {
    if url.len() > 8192 || url.bytes().any(|b| b.is_ascii_control()) || url.contains(['#', '\\']) {
        return Err(Error("prohibited release URL".into()));
    }
    let uri: http::Uri = url
        .parse()
        .map_err(|_| Error("invalid release URL".into()))?;
    if uri.scheme_str() != Some("https")
        || uri.port_u16().is_some_and(|port| port != 443)
        || uri
            .authority()
            .is_none_or(|authority| authority.as_str().contains('@'))
    {
        return Err(Error(
            "release URL requires HTTPS without credentials".into(),
        ));
    }
    let permitted = match uri.host() {
        Some("api.github.com") => uri.path() == "/repos/KroxiDev/RAD/releases",
        Some("github.com") => {
            uri.path().starts_with("/KroxiDev/RAD/releases/download/")
                && !uri.path().contains('%')
                && !uri
                    .path()
                    .split('/')
                    .any(|part| part == "." || part == "..")
        }
        Some("release-assets.githubusercontent.com") => true,
        _ => false,
    };
    if !permitted {
        return Err(Error(
            "release URL is not an official RAD destination".into(),
        ));
    }
    Ok(())
}
/// Verifies exact bytes, independent of their untrusted staging filename.
pub fn verify_file(
    path: &Path,
    metadata: &ReleaseMetadata,
    cancel: &Cancellation,
) -> Result<(), Error> {
    // SAFETY: geteuid has no preconditions and reads the current process identity.
    let uid = unsafe { libc::geteuid() };
    let file = crate::installation::private_file(path, uid)?;
    verify_reader(file, metadata, cancel)
}
pub(crate) fn verify_reader(
    mut file: File,
    metadata: &ReleaseMetadata,
    cancel: &Cancellation,
) -> Result<(), Error> {
    if file.metadata()?.len() != metadata.size || metadata.size > crate::MAX_PACKAGE_BYTES {
        return Err(Error("package size does not match official release".into()));
    }
    let mut hash = Sha256::new();
    let mut block = [0; 65536];
    let mut read = 0u64;
    loop {
        cancel.check()?;
        let n = file.read(&mut block)?;
        if n == 0 {
            break;
        }
        read += n as u64;
        if read > metadata.size {
            return Err(Error("package grew during verification".into()));
        }
        hash.update(&block[..n]);
    }
    if read != metadata.size || format!("{:x}", hash.finalize()) != metadata.sha256 {
        return Err(Error(
            "package digest does not match official release".into(),
        ));
    }
    Ok(())
}

pub(crate) struct Temporary {
    pub path: PathBuf,
    pub files: Vec<String>,
    pub keep: bool,
    lease: Option<crate::cleanup::Lease>,
}
impl Temporary {
    pub(crate) fn new(prefix: &str) -> Result<Self, Error> {
        Self::new_in(Path::new("/tmp"), prefix)
    }
    pub(crate) fn new_in(parent: &Path, prefix: &str) -> Result<Self, Error> {
        let mut random = [0u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let suffix = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        if !fs::symlink_metadata(parent)?.is_dir() {
            return Err(Error("unsafe temporary parent".into()));
        }
        let path = parent.join(format!("rad-update-{prefix}-{uid}-{suffix}"));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        let lease = crate::cleanup::initialize(&path, uid, prefix, &suffix)?;
        Ok(Self {
            path,
            files: vec![crate::cleanup::OWNER.into(), crate::cleanup::LEASE.into()],
            keep: false,
            lease: Some(lease),
        })
    }
    pub(crate) fn create(&mut self, name: &str) -> Result<File, Error> {
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err(Error("invalid staging filename".into()));
        }
        let file = self
            .lease
            .as_ref()
            .ok_or_else(|| Error("temporary lease was released".into()))?
            .create(name)?;
        self.files.push(name.into());
        Ok(file)
    }
    pub(crate) fn inherit_to(&self, command: &mut std::process::Command) -> Result<(), Error> {
        self.lease
            .as_ref()
            .ok_or_else(|| Error("temporary lease was released".into()))?
            .inherit_to(command)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.keep
            && let Some(lease) = self.lease.take()
        {
            let _ = lease.discard(&self.files);
        }
    }
}

pub(crate) struct Curl {
    deadline: Instant,
}
impl Curl {
    pub(crate) fn new(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
        }
    }
}
impl crate::release::Source for Curl {
    fn get(&mut self, url: &str, limit: usize, cancel: &Cancellation) -> Result<Vec<u8>, Error> {
        let mut temporary = Temporary::new("fetch")?;
        drop(temporary.create("response")?);
        self.fetch_file(
            url,
            &temporary.path.join("response"),
            limit as u64,
            cancel,
            &Progress::default(),
        )?;
        let mut bytes = Vec::new();
        File::open(temporary.path.join("response"))?
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(Error("response body limit exceeded".into()));
        }
        Ok(bytes)
    }
}
impl Curl {
    /// curl >=8.4 bounds streamed output even without Content-Length; private file avoids a large in-memory body.
    pub(crate) fn fetch_file(
        &mut self,
        url: &str,
        path: &Path,
        limit: u64,
        cancel: &Cancellation,
        progress: &Progress,
    ) -> Result<(), Error> {
        let mut url = url.to_owned();
        for redirects in 0..=5 {
            cancel.check()?;
            validate_destination(&url)?;
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .filter(|duration| !duration.is_zero())
                .ok_or_else(|| Error("HTTPS deadline exceeded".into()))?;
            let mut command = crate::installation::command("/usr/bin/curl");
            command
                .args([
                    "--disable",
                    "--silent",
                    "--show-error",
                    "--noproxy",
                    "*",
                    "--proto",
                    "=https",
                    "--proto-redir",
                    "=https",
                    "--max-redirs",
                    "0",
                    "--connect-timeout",
                    "10",
                    "--http1.1",
                    "--header",
                    "Accept-Encoding: identity",
                    "--header",
                    "Accept: application/vnd.github+json",
                    "--header",
                    "User-Agent: RAD-updater/1",
                    "--dump-header",
                    "-",
                    "--output",
                ])
                .arg(path)
                .args([
                    "--max-filesize",
                    &limit.to_string(),
                    "--max-time",
                    &format!("{:.3}", remaining.as_secs_f64()),
                    "--url",
                    &url,
                ])
                .stdin(Stdio::null());
            let output = curl_capture(command, path, limit, remaining, cancel, progress)?;
            if output.code != Some(0) {
                return Err(Error(format!(
                    "official HTTPS request failed (curl {:?})",
                    output.code
                )));
            }
            let headers = parse_headers(&output.stdout)?;
            if matches!(headers.status, 301 | 302 | 303 | 307 | 308) {
                if redirects == 5 {
                    return Err(Error("HTTPS redirect limit exceeded".into()));
                }
                url = headers
                    .location
                    .ok_or_else(|| Error("redirect without destination".into()))?;
                continue;
            }
            if headers.status != 200 {
                return Err(Error(format!(
                    "official release server returned HTTP {}",
                    headers.status
                )));
            }
            return Ok(());
        }
        Err(Error("HTTPS redirect limit exceeded".into()))
    }
}
struct Headers {
    status: u16,
    location: Option<String>,
}
fn parse_headers(text: &str) -> Result<Headers, Error> {
    if text.len() > 65536 {
        return Err(Error("HTTP header limit exceeded".into()));
    }
    let mut lines = text.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| {
            line.strip_prefix("HTTP/1.1 ")
                .or_else(|| line.strip_prefix("HTTP/1.0 "))
        })
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| Error("malformed HTTP status".into()))?;
    let mut location = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| Error("malformed HTTP header".into()))?;
        let value = value.trim();
        if key.eq_ignore_ascii_case("location") && location.replace(value.into()).is_some() {
            return Err(Error("ambiguous redirect".into()));
        }
        if key.eq_ignore_ascii_case("content-encoding") && !value.eq_ignore_ascii_case("identity") {
            return Err(Error("compressed release response rejected".into()));
        }
    }
    Ok(Headers { status, location })
}
fn curl_capture(
    mut command: std::process::Command,
    path: &Path,
    limit: u64,
    timeout: Duration,
    cancel: &Cancellation,
    progress: &Progress,
) -> Result<crate::installation::Output, Error> {
    // capture polls the real process: cancelling DNS/TLS also kills and reaps curl, without detached workers.
    // A progress observer owns no thread; file size is read by a short-lived observer joined before return.
    let completed = Cancellation::default();
    let observer = std::thread::spawn({
        let completed = completed.clone();
        let path = path.to_owned();
        let progress = progress.clone();
        move || {
            while !completed.is_cancelled() {
                if let Ok(meta) = fs::metadata(&path) {
                    progress.report(meta.len(), limit);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    });
    command.stderr(Stdio::piped());
    let result = crate::installation::capture(command, Some(timeout), cancel, 65537);
    completed.cancel();
    let _ = observer.join();
    let output = result?;
    if output.stdout.len() > 65536 || fs::metadata(path)?.len() > limit {
        return Err(Error("HTTPS response limit exceeded".into()));
    }
    Ok(output)
}

pub(crate) fn save_candidate(
    temporary: &mut Temporary,
    candidate: &crate::Candidate,
) -> Result<(), Error> {
    let mut file = temporary.create("candidate.json")?;
    serde_json::to_writer(&mut file, candidate).map_err(|error| Error(error.to_string()))?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
/// Deletes only a validated updater-created directory and its two known files; never recursive deletion.
pub fn discard_prepared(prepared: &crate::Prepared) -> Result<(), Error> {
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let path = &prepared.directory;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error("invalid staging directory".into()))?;
    let prefix = format!("rad-update-stage-{uid}-");
    if path.parent() != Some(Path::new("/tmp"))
        || !name.strip_prefix(&prefix).is_some_and(|suffix| {
            suffix.len() == 32 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        return Err(Error("not an updater staging directory".into()));
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err(Error("unsafe staging directory".into()));
    }
    let lease = crate::cleanup::Lease::pin(path, uid)?;
    let marker = lease.open_file("candidate.json")?;
    let candidate: crate::Candidate =
        serde_json::from_reader(marker.take(crate::MAX_REQUEST_BYTES + 1))
            .map_err(|_| Error("invalid staging marker".into()))?;
    if candidate != prepared.candidate {
        return Err(Error("staging does not belong to this operation".into()));
    }
    prepared
        .candidate
        .metadata
        .validate(&prepared.candidate.tag)?;
    let expected = vec![
        crate::cleanup::OWNER.into(),
        crate::cleanup::LEASE.into(),
        "candidate.json".into(),
        prepared.candidate.metadata.file.clone(),
    ];
    lease.discard(&expected)
}

#[cfg(test)]
mod temporary_cleanup_tests {
    use super::*;
    use crate::cleanup::{self, Liveness, ProcessState};
    struct Dead;
    impl Liveness for Dead {
        fn state(&self, _: u32) -> ProcessState {
            ProcessState::Dead
        }
    }
    struct Parent(PathBuf);
    impl Parent {
        fn new() -> Self {
            let random = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "rad-producer-fixture-{}-{random}",
                std::process::id()
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Parent {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn temporary_provenance_and_pin_prevent_drop_or_gc_from_removing_a_consumers_files() {
        let parent = Parent::new();
        let mut temporary = Temporary::new_in(&parent.0, "fetch").unwrap();
        let mut response = temporary.create("response").unwrap();
        response.write_all(b"consumer bytes").unwrap();
        drop(response);
        // SAFETY: geteuid only reads the current process identity.
        let uid = unsafe { libc::geteuid() };
        let marker: serde_json::Value =
            serde_json::from_slice(&fs::read(temporary.path.join(".owner.json")).unwrap()).unwrap();
        assert_eq!(marker["pid"], std::process::id());
        assert_eq!(marker["uid"], uid);
        assert_eq!(marker["kind"], "fetch");
        let pin = cleanup::Lease::pin(&temporary.path, uid).unwrap();
        let path = temporary.path.clone();
        drop(temporary);
        assert_eq!(fs::read(path.join("response")).unwrap(), b"consumer bytes");
        assert_eq!(
            cleanup::collect_in(&parent.0, uid, &Dead).unwrap().removed,
            0
        );
        drop(pin);
        assert_eq!(
            cleanup::collect_in(&parent.0, uid, &Dead).unwrap().removed,
            1
        );
        let temporary = Temporary::new_in(&parent.0, "fetch").unwrap();
        let path = temporary.path.clone();
        drop(temporary);
        assert!(
            !path.exists(),
            "normal Drop removes its private marker and lease too"
        );
    }
    fn candidate() -> crate::Candidate {
        crate::Candidate {
            tag: "v0.2.0-1".into(),
            installed_version: "0.1.0-1".into(),
            metadata: crate::ReleaseMetadata {
                schema: 1,
                product: "RAD".into(),
                package: "rust-action-deck".into(),
                version: "0.2.0".into(),
                pkgrel: 1,
                architecture: "x86_64".into(),
                file: "rust-action-deck-0.2.0-1-x86_64.pkg.tar.zst".into(),
                size: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
                base: "omarchy-4.0.6".into(),
                commit: "a".repeat(40),
            },
        }
    }
    #[test]
    fn explicit_prepared_discard_accepts_new_ownership_files_but_never_deletes_unknown_content() {
        let candidate = candidate();
        let mut temporary = Temporary::new("stage").unwrap();
        drop(temporary.create(&candidate.metadata.file).unwrap());
        save_candidate(&mut temporary, &candidate).unwrap();
        let prepared = crate::Prepared {
            directory: temporary.path.clone(),
            candidate: candidate.clone(),
        };
        temporary.keep = true;
        drop(temporary);
        discard_prepared(&prepared).unwrap();
        assert!(!prepared.directory.exists());
        let mut temporary = Temporary::new("stage").unwrap();
        drop(temporary.create(&candidate.metadata.file).unwrap());
        save_candidate(&mut temporary, &candidate).unwrap();
        let prepared = crate::Prepared {
            directory: temporary.path.clone(),
            candidate,
        };
        fs::write(temporary.path.join("unexpected"), b"must remain").unwrap();
        assert!(discard_prepared(&prepared).is_err());
        assert!(temporary.path.join("candidate.json").exists());
        fs::remove_file(temporary.path.join("unexpected")).unwrap();
        drop(temporary);
        assert!(!prepared.directory.exists());
    }
}
