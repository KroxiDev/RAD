//! Minimal privileged installer. The executable has no arbitrary-command or arbitrary-package mode.
use crate::{Cancellation, Candidate, Error, InstallOutcome, Prepared};
use std::{fs::File, path::Path};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageIdentity {
    pub package: String,
    pub version: String,
    pub architecture: String,
}
pub struct TransactionReport {
    pub success: bool,
    pub details: String,
}
/// Effects seam for critical synthetic transaction tests. The production binary always uses the fixed implementation.
pub trait System {
    fn configuration(&mut self) -> Result<String, Error>;
    fn installed_version(&mut self) -> Result<String, Error>;
    fn official_candidate(&mut self, installed: &str) -> Result<Option<Candidate>, Error>;
    fn inspect(&mut self, path: &Path) -> Result<PackageIdentity, Error>;
    fn transact(&mut self, path: &Path) -> Result<TransactionReport, Error>;
}
pub struct Authorized {
    candidate: Candidate,
    staging: crate::download::Temporary,
}
/// Authenticate from the official origin and freeze the user's bytes before READY.
pub fn preflight_with(
    prepared: &Prepared,
    uid: u32,
    system: &mut impl System,
) -> Result<Authorized, Error> {
    use std::os::unix::fs::MetadataExt;
    let candidate = &prepared.candidate;
    candidate.metadata.validate(&candidate.tag)?;
    pacman_options_only(&system.configuration()?)?;
    let installed = system.installed_version()?;
    if installed != candidate.installed_version {
        return Err(Error(
            "the installed package changed before authorization".into(),
        ));
    }
    if crate::installation::compare_versions(&candidate.metadata.package_version(), &installed)?
        != std::cmp::Ordering::Greater
    {
        return Err(Error(
            "only a strictly newer official package may be installed".into(),
        ));
    }
    let official = system
        .official_candidate(&installed)?
        .ok_or_else(|| Error("no official update is available".into()))?;
    if &official != candidate {
        return Err(Error(
            "candidate does not match the freshly queried official release".into(),
        ));
    }
    let name = prepared
        .directory
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| Error("invalid source staging".into()))?;
    let prefix = format!("rad-update-stage-{uid}-");
    if prepared.directory.parent() != Some(Path::new("/tmp"))
        || !name.strip_prefix(&prefix).is_some_and(|suffix| {
            suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(Error(
            "source is not a private updater staging directory".into(),
        ));
    }
    let directory = std::fs::symlink_metadata(&prepared.directory)?;
    if !directory.is_dir() || directory.uid() != uid || directory.mode() & 0o077 != 0 {
        return Err(Error("unsafe source staging directory".into()));
    }
    let source =
        crate::installation::private_file(&prepared.directory.join(&candidate.metadata.file), uid)?;
    if source.metadata()?.len() != candidate.metadata.size {
        return Err(Error("source package size changed".into()));
    }
    let mut staging = crate::download::Temporary::new("root")?;
    let mut output = staging.create(&candidate.metadata.file)?;
    use std::io::Read;
    let copied = std::io::copy(&mut source.take(candidate.metadata.size + 1), &mut output)?;
    if copied != candidate.metadata.size {
        return Err(Error("source package size changed during handover".into()));
    }
    output.sync_all()?;
    drop(output);
    let path = staging.path.join(&candidate.metadata.file);
    crate::download::verify_file(&path, &candidate.metadata, &Cancellation::default())?;
    let identity = system.inspect(&path)?;
    if identity.package != candidate.metadata.package
        || identity.version != candidate.metadata.package_version()
        || identity.architecture != candidate.metadata.architecture
    {
        return Err(Error(
            "protected archive identity differs from official metadata".into(),
        ));
    }
    File::open(&staging.path)?.sync_all()?;
    Ok(Authorized {
        candidate: candidate.clone(),
        staging,
    })
}
/// Revalidate immediately before the one irreversible transaction; no retry or downgrade.
pub fn commit_with(
    authorized: Authorized,
    system: &mut impl System,
) -> Result<InstallOutcome, Error> {
    if system.installed_version()? != authorized.candidate.installed_version {
        return Err(Error(
            "installation changed before the transaction; no package was installed".into(),
        ));
    }
    let path = authorized
        .staging
        .path
        .join(&authorized.candidate.metadata.file);
    crate::download::verify_file(
        &path,
        &authorized.candidate.metadata,
        &Cancellation::default(),
    )?;
    let report = system.transact(&path)?;
    let installed = system.installed_version()?;
    if !report.success || installed != authorized.candidate.metadata.package_version() {
        return Err(Error(format!(
            "pacman transaction did not produce a verified installation (installed {installed}): {}",
            report.details
        )));
    }
    Ok(InstallOutcome {
        installed_version: installed,
        details: report.details,
    })
}
/// Retains effective local options verbatim and excludes every sync repository.
pub fn pacman_options_only(expanded: &str) -> Result<String, Error> {
    if expanded.len() > 65536 || expanded.lines().next() != Some("[options]") {
        return Err(Error("invalid expanded pacman configuration".into()));
    }
    let mut options = String::new();
    let mut root = false;
    let mut database = false;
    for (index, line) in expanded.lines().enumerate() {
        if index > 0 && line.starts_with('[') {
            break;
        }
        if let Some(value) = line.strip_prefix("RootDir = ") {
            if value != "/" {
                return Err(Error(
                    "RAD updates require the normal system installation root".into(),
                ));
            }
            root = true;
        }
        if let Some(value) = line.strip_prefix("DBPath = ") {
            if value.trim_end_matches('/') != "/var/lib/pacman" {
                return Err(Error(
                    "RAD updates require the normal package database".into(),
                ));
            }
            database = true;
        }
        if let Some((key, value)) = line.split_once('=')
            && matches!(key.trim(), "NoUpgrade" | "NoExtract")
            && !value.trim().is_empty()
        {
            return Err(Error("pacman NoUpgrade/NoExtract file exclusions prevent verifying a complete RAD installation; use a manual update after reviewing the local configuration".into()));
        }
        if line.starts_with("Include") {
            return Err(Error("pacman configuration was not fully expanded".into()));
        }
        options.push_str(line);
        options.push('\n');
    }
    if !root || !database {
        return Err(Error(
            "pacman root/database configuration is incomplete".into(),
        ));
    }
    Ok(options)
}

struct OfficialSystem {
    expected: String,
    target: String,
}
impl System for OfficialSystem {
    fn configuration(&mut self) -> Result<String, Error> {
        let expanded = crate::installation::query("/usr/bin/pacman-conf", &[])?;
        if expanded.code != Some(0) || expanded.stdout.len() >= 65536 {
            return Err(Error("cannot read effective pacman configuration".into()));
        }
        Ok(expanded.stdout)
    }
    fn installed_version(&mut self) -> Result<String, Error> {
        crate::installation::installed_version()
    }
    fn official_candidate(&mut self, installed: &str) -> Result<Option<Candidate>, Error> {
        crate::release::check_with(
            &mut crate::download::Curl::new(std::time::Duration::from_secs(45)),
            installed,
            &Cancellation::default(),
        )
    }
    fn inspect(&mut self, path: &Path) -> Result<PackageIdentity, Error> {
        let path = path
            .to_str()
            .ok_or_else(|| Error("invalid protected package path".into()))?;
        let query = crate::installation::query("/usr/bin/pacman", &["-Qp", "--", path])?;
        if query.code != Some(0) {
            return Err(Error(format!(
                "pacman rejected the protected archive: {}",
                query.stderr
            )));
        }
        let mut values = query.stdout.split_whitespace();
        let package = values
            .next()
            .ok_or_else(|| Error("archive package identity missing".into()))?
            .to_owned();
        let version = values
            .next()
            .ok_or_else(|| Error("archive version missing".into()))?
            .to_owned();
        if values.next().is_some() {
            return Err(Error("ambiguous archive identity".into()));
        }
        let archive = crate::installation::query("/usr/bin/bsdtar", &["-xOf", path, ".PKGINFO"])?;
        if archive.code != Some(0) || archive.stdout.len() >= 65536 {
            return Err(Error(
                "protected package metadata is invalid or oversized".into(),
            ));
        }
        let mut name = None;
        let mut pkgver = None;
        let mut architecture = None;
        for line in archive.stdout.lines() {
            let Some((key, value)) = line.split_once(" = ") else {
                continue;
            };
            let field = match key {
                "pkgname" => &mut name,
                "pkgver" => &mut pkgver,
                "arch" => &mut architecture,
                _ => continue,
            };
            if field.replace(value.to_owned()).is_some() {
                return Err(Error("duplicate protected package identity field".into()));
            }
        }
        if name.as_deref() != Some(&package) || pkgver.as_deref() != Some(&version) {
            return Err(Error("pacman and archive identities disagree".into()));
        }
        Ok(PackageIdentity {
            package,
            version,
            architecture: architecture
                .ok_or_else(|| Error("archive architecture missing".into()))?,
        })
    }
    fn transact(&mut self, path: &Path) -> Result<TransactionReport, Error> {
        use std::io::Write;
        let options = pacman_options_only(&self.configuration()?)?;
        let mut temporary = crate::download::Temporary::new("config")?;
        let mut config = temporary.create("pacman.conf")?;
        config.write_all(options.as_bytes())?;
        config.sync_all()?;
        drop(config);
        let hook = format!(
            "[Trigger]\nOperation = Install\nOperation = Upgrade\nType = Package\nTarget = rust-action-deck\n[Action]\nWhen = PreTransaction\nExec = /usr/lib/rad/rad-update-installer --transaction-guard {} {}\nAbortOnFail\n",
            self.expected, self.target
        );
        let mut guard = temporary.create("00-rad-update-guard.hook")?;
        guard.write_all(hook.as_bytes())?;
        guard.sync_all()?;
        drop(guard);
        let mut command = crate::installation::command("/usr/bin/pacman");
        command
            .arg("--config")
            .arg(temporary.path.join("pacman.conf"))
            .arg("--hookdir")
            .arg(&temporary.path)
            .args([
                "-U",
                "--needed",
                "--noconfirm",
                "--color",
                "never",
                "--noprogressbar",
                "--",
            ])
            .arg(path);
        let package_directory = path
            .parent()
            .ok_or_else(|| Error("protected package directory is missing".into()))?;
        inherit_transaction_leases(&mut command, package_directory, &temporary)?;
        // None is intentional: never kill a running pacman transaction for cancellation/deadline.
        let result = crate::installation::capture(command, None, &Cancellation::default(), 32768)?;
        Ok(TransactionReport {
            success: result.code == Some(0),
            details: format!(
                "pacman exit {:?}\n{}\n{}",
                result.code, result.stdout, result.stderr
            ),
        })
    }
}
fn read_frame(limit: usize, timeout: std::time::Duration) -> Result<Vec<u8>, Error> {
    let deadline = std::time::Instant::now() + timeout;
    let mut bytes = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or_else(|| Error("installer handover deadline exceeded".into()))?;
        let mut poll = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `poll` is initialized, one entry is passed, and the byte buffer is writable.
        let received = unsafe {
            let ready = libc::poll(&mut poll, 1, remaining.as_millis().min(1000) as i32);
            if ready < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(std::io::Error::last_os_error().into());
            }
            if ready == 0 {
                continue;
            }
            let mut byte = 0u8;
            let read = libc::read(0, (&mut byte as *mut u8).cast(), 1);
            (read, byte)
        };
        match received {
            (0, _) => return Err(Error("installer handover closed before commit".into())),
            (1, b'\n') => return Ok(bytes),
            (1, byte) => {
                if bytes.len() >= limit {
                    return Err(Error("installer frame exceeds limit".into()));
                }
                bytes.push(byte);
            }
            _ => return Err(std::io::Error::last_os_error().into()),
        }
    }
}
fn root_lock() -> Result<File, Error> {
    use std::os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let path = Path::new("/run/lock/rad-update-install.lock");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o077 != 0 || meta.nlink() != 1 {
        return Err(Error("unsafe installation lock".into()));
    }
    // SAFETY: the descriptor is live and lock flags are valid.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(Error(
            "another RAD installation update is in progress".into(),
        ));
    }
    Ok(file)
}
/// Serialized outcome frame, including its newline, remains below the coordinator's 16 KiB input cap.
pub const MAX_OUTCOME_FRAME_BYTES: usize = 8192;
pub fn outcome_frame(outcome: &InstallOutcome) -> Result<Vec<u8>, Error> {
    fn encode(outcome: &InstallOutcome) -> Result<Vec<u8>, Error> {
        let mut frame = serde_json::to_vec(outcome).map_err(|error| Error(error.to_string()))?;
        frame.push(b'\n');
        Ok(frame)
    }
    let frame = encode(outcome)?;
    if frame.len() <= MAX_OUTCOME_FRAME_BYTES {
        return Ok(frame);
    }
    let marker = "\n[diagnostic output truncated]\n";
    let characters = outcome.details.chars().collect::<Vec<_>>();
    let mut summary = outcome.clone();
    summary.details = marker.into();
    let mut best = encode(&summary)?;
    if best.len() > MAX_OUTCOME_FRAME_BYTES {
        return Err(Error(
            "installation outcome identity exceeds the frame limit".into(),
        ));
    }
    // Preserve both the initial status and final stderr, measuring actual JSON escaping.
    let (mut low, mut high) = (0, characters.len());
    while low < high {
        let retained = (low + high).div_ceil(2);
        let head = retained.div_ceil(2);
        let tail = retained / 2;
        summary.details = characters[..head]
            .iter()
            .chain(marker.chars().collect::<Vec<_>>().iter())
            .chain(characters[characters.len() - tail..].iter())
            .collect();
        let candidate = encode(&summary)?;
        if candidate.len() <= MAX_OUTCOME_FRAME_BYTES {
            low = retained;
            best = candidate;
        } else {
            high = retained - 1;
        }
    }
    Ok(best)
}
/// The only entry point used by the privileged executable; no command-line parameters.
pub fn run() -> Result<(), Error> {
    use std::io::Write;
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        return Err(Error(
            "installer requires administrative authorization".into(),
        ));
    }
    if std::env::current_exe()? != Path::new("/usr/lib/rad/rad-update-installer") {
        return Err(Error(
            "installer is not the official installed executable".into(),
        ));
    }
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if let [mode, expected, target] = args.as_slice()
        && mode == "--transaction-guard"
    {
        return transaction_guard(expected, target, &crate::installation::installed_version()?);
    }
    if !args.is_empty() {
        return Err(Error(
            "installer accepts only its closed handover or transaction guard protocol".into(),
        ));
    }
    let uid = std::env::var("PKEXEC_UID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|uid| *uid > 0)
        .ok_or_else(|| Error("original authorized user is missing".into()))?;
    let _lock = root_lock()?;
    // Only the already-authorized privileged entrypoint collects root-owned
    // updater staging/config leftovers. Maintenance never changes admission and
    // never writes to the READY/outcome protocol or touches another user's data.
    let _ = crate::cleanup::collect_in(Path::new("/tmp"), 0, &crate::cleanup::ProcessLiveness);
    let request = read_frame(
        crate::MAX_REQUEST_BYTES as usize,
        std::time::Duration::from_secs(30),
    )?;
    let prepared: Prepared =
        serde_json::from_slice(&request).map_err(|_| Error("invalid installer request".into()))?;
    let mut system = OfficialSystem {
        expected: prepared.candidate.installed_version.clone(),
        target: prepared.candidate.metadata.package_version(),
    };
    let authorized = preflight_with(&prepared, uid, &mut system)?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(b"READY\n")?;
    stdout.flush()?;
    if read_frame(16, std::time::Duration::from_secs(120))? != b"commit" {
        return Err(Error("installation was not committed".into()));
    }
    let result = commit_with(authorized, &mut system)?;
    stdout.write_all(&outcome_frame(&result)?)?;
    stdout.flush()?;
    Ok(())
}
/// Invoked by the temporary PreTransaction hook while pacman owns its real database lock.
pub fn transaction_guard(expected: &str, target: &str, actual: &str) -> Result<(), Error> {
    if expected != actual
        || crate::installation::compare_versions(target, actual)? != std::cmp::Ordering::Greater
    {
        return Err(Error(
            "installed version changed while waiting for pacman; transaction aborted".into(),
        ));
    }
    Ok(())
}

fn inherit_transaction_leases(
    command: &mut std::process::Command,
    package_directory: &Path,
    configuration: &crate::download::Temporary,
) -> Result<(), Error> {
    // Both descriptions remain shared in the exec'd transaction, even if this
    // installer dies. The clone captured by pre_exec keeps its fd alive until
    // exec; the parent's shared flock is never explicitly unlocked.
    // SAFETY: geteuid only reads the current identity (root in run).
    let package = crate::cleanup::Lease::pin(package_directory, unsafe { libc::geteuid() })?;
    package.inherit_to(command)?;
    configuration.inherit_to(command)
}

#[cfg(test)]
mod lease_tests {
    use super::*;
    use std::{
        fs,
        io::Write,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    const TEST: &str = "installer::lease_tests::pacman_equivalent_child_retains_both_temporaries_after_installer_exit";
    const PACKAGE: &str = "rust-action-deck-0.2.0-1-x86_64.pkg.tar.zst";
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct Release(PathBuf);
    impl Drop for Release {
        fn drop(&mut self) {
            let _ = fs::write(&self.0, b"release");
            if let Some(parent) = self.0.parent() {
                let deadline = Instant::now() + Duration::from_secs(1);
                while !parent.join("consumer.finished").exists() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
    #[test]
    fn pacman_equivalent_child_retains_both_temporaries_after_installer_exit() {
        if let Ok(role) = std::env::var("RAD_UPDATE_INHERITANCE_ROLE") {
            let root = PathBuf::from(std::env::var_os("RAD_UPDATE_INHERITANCE_ROOT").unwrap());
            if role == "consumer" {
                fs::write(root.join("consumer.ready"), b"ready").unwrap();
                let deadline = Instant::now() + Duration::from_secs(30);
                while !root.join("release").exists() {
                    assert!(
                        Instant::now() < deadline,
                        "controlled consumer release timeout"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                fs::write(root.join("consumer.finished"), b"finished").unwrap();
                return;
            }
            assert_eq!(role, "owner");
            let mut package = crate::download::Temporary::new_in(&root, "root").unwrap();
            package
                .create(PACKAGE)
                .unwrap()
                .write_all(b"synthetic package")
                .unwrap();
            let mut config = crate::download::Temporary::new_in(&root, "config").unwrap();
            config
                .create("pacman.conf")
                .unwrap()
                .write_all(b"[options]\n")
                .unwrap();
            config
                .create("00-rad-update-guard.hook")
                .unwrap()
                .write_all(b"synthetic hook")
                .unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap());
            child
                .args(["--exact", TEST, "--nocapture"])
                .env("RAD_UPDATE_INHERITANCE_ROLE", "consumer")
                .env("RAD_UPDATE_INHERITANCE_ROOT", &root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            inherit_transaction_leases(&mut child, &package.path, &config).unwrap();
            let spawned = child.spawn().unwrap();
            fs::write(
                root.join("paths.json"),
                serde_json::to_vec(&[package.path.clone(), config.path.clone()]).unwrap(),
            )
            .unwrap();
            fs::write(root.join("consumer.pid"), spawned.id().to_string()).unwrap();
            // Deliberate abrupt owner exit: no Temporary Drop runs. The controlled
            // transaction-equivalent child must retain both SH leases after exec.
            std::process::exit(0);
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("rad-update-inherited-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let fixture = Fixture(root.clone());
        let _release = Release(root.join("release"));
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env("RAD_UPDATE_INHERITANCE_ROLE", "owner")
            .env("RAD_UPDATE_INHERITANCE_ROOT", &root)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "owner: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("consumer.ready").exists() {
            assert!(Instant::now() < deadline, "controlled child readiness");
            std::thread::sleep(Duration::from_millis(10));
        }
        let paths: Vec<PathBuf> =
            serde_json::from_slice(&fs::read(root.join("paths.json")).unwrap()).unwrap();
        // Only this private fixture root is collected, never the machine's /tmp.
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let report =
            crate::cleanup::collect_in(&root, uid, &crate::cleanup::ProcessLiveness).unwrap();
        assert_eq!(
            report.removed, 0,
            "a dead installer is not an abandoned transaction"
        );
        assert_eq!(
            fs::read(paths[0].join(PACKAGE)).unwrap(),
            b"synthetic package"
        );
        assert_eq!(
            fs::read(paths[1].join("pacman.conf")).unwrap(),
            b"[options]\n"
        );
        fs::write(root.join("release"), b"release").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut removed = 0;
        while removed < 2 {
            removed += crate::cleanup::collect_in(&root, uid, &crate::cleanup::ProcessLiveness)
                .unwrap()
                .removed;
            assert!(
                removed >= 2 || Instant::now() < deadline,
                "transaction leases were not released after child exit"
            );
            if removed < 2 {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        assert!(!paths[0].exists());
        assert!(!paths[1].exists());
        drop(_release);
        drop(fixture);
    }
}
