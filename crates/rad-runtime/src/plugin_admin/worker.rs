use super::*;
use rad_plugin_manager::{Error, Manager, ManagerState, Operation};
use std::{
    os::{fd::AsRawFd, unix::fs::DirBuilderExt},
    process::Command,
};
pub(super) fn run(
    root: PathBuf,
    jobs: mpsc::Receiver<Job>,
    results: mpsc::SyncSender<Reply>,
    wake: Arc<dyn Fn() + Send + Sync>,
    download: Download,
    cancel: rad_plugin_manager::Cancellation,
) {
    let opened = open_store(&root);
    let boot = opened.as_ref().map_err(|e| *e).and_then(|m| m.installed());
    if results.send(Reply::Boot(boot)).is_err() {
        return;
    }
    wake();
    let mut manager = opened.ok();
    let mut _residual: Option<PreparedChange> = None;
    while let Ok(job) = jobs.recv() {
        if matches!(job, Job::Stop) {
            break;
        }
        let Some(m) = manager.as_mut() else {
            continue;
        };
        let reply = match job {
            Job::Prepare(request) => {
                let result = prepare(m, request, &download, &cancel);
                Reply::Prepared(result, m.state() == ManagerState::Ready)
            }
            Job::Commit(prepared) => {
                let key = prepared.key();
                let mut prepared = Some(prepared);
                let outcome = m.commit_recovering(&mut prepared);
                // The manager owns durable recovery state; preserve any unconsumed candidate.
                _residual = if matches!(outcome, CommitOutcome::Uncertain(_)) {
                    prepared
                } else {
                    None
                };
                Reply::Committed(key, outcome, m.installed())
            }
            Job::Abort(prepared) => {
                let mut p = Some(prepared);
                let result = m.abort(&mut p);
                _residual = p;
                Reply::Aborted(result)
            }
            Job::Spawn { id, ticket } => {
                // Linux ELF via retained executable descriptor; no arbitrary package path.
                let file = m.executable(&id);
                let mut command = match &file {
                    Ok(file) => Command::new(format!("/proc/self/fd/{}", file.as_raw_fd())),
                    Err(_) => Command::new(""),
                };
                command.current_dir("/");
                let prepared = ticket.spawn(command);
                drop(file);
                Reply::Spawned(Box::new(prepared))
            }
            Job::Stop => break,
        };
        if results.send(reply).is_err() {
            break;
        }
        wake();
    }
}

fn prepare(
    manager: &mut Manager,
    request: PluginAdmin,
    download: &Download,
    cancel: &rad_plugin_manager::Cancellation,
) -> Result<PreparedChange, Error> {
    let source = request
        .source
        .as_deref()
        .map(rad_plugin_manager::GithubSource::parse)
        .transpose()?;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let package = request
        .source
        .as_ref()
        .map(|s| download(s, cancel))
        .transpose()?;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let id = request.plugin_id.unwrap_or_default();
    let operation = match request.operation {
        rad_protocol::PluginOperation::Install => Operation::Install,
        rad_protocol::PluginOperation::Update => Operation::Update(id),
        rad_protocol::PluginOperation::Reinstall => Operation::Reinstall(id),
        rad_protocol::PluginOperation::Rollback => Operation::Rollback(id),
        rad_protocol::PluginOperation::Uninstall => Operation::Uninstall(id),
    };
    manager.prepare_from_source(operation, package.as_deref(), source.as_ref())
}

fn open_store(root: &std::path::Path) -> Result<Manager, Error> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let parent = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root.parent().ok_or(Error::Io)?)
        .map_err(|_| Error::Io)?;
    let manager = match std::fs::DirBuilder::new().mode(0o700).create(root) {
        Ok(()) => Manager::create(root, rad_plugin_contract::HOST_API, rad_plugin_host::TARGET),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Manager::open(root, rad_plugin_contract::HOST_API, rad_plugin_host::TARGET)
        }
        Err(_) => Err(Error::Io),
    }?;
    // Also repeat for an existing initialized store after a prior sync failure.
    // Failure preserves disk state and prevents Boot(Ok), never recreate/adopt.
    let current = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root.parent().ok_or(Error::Io)?)
        .map_err(|_| Error::Io)?;
    let before = parent.metadata().map_err(|_| Error::Io)?;
    let after = current.metadata().map_err(|_| Error::Io)?;
    if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
        return Err(Error::Io);
    }
    sync_parent(&parent, root)?;
    Ok(manager)
}
fn sync_parent(parent: &std::fs::File, _root: &std::path::Path) -> Result<(), Error> {
    #[cfg(test)]
    {
        // Private, thread-local fault only: never selected by product configuration.
        assert!(
            _root.join("index.json").is_file(),
            "parent sync occurs after initialization, before usable Boot"
        );
        if durability_tests::SYNC.with(|s| {
            let mut s = s.borrow_mut();
            s.1 += 1;
            s.0
        }) {
            return Err(Error::Io);
        }
    }
    parent.sync_all().map_err(|_| Error::Io)
}

#[cfg(test)]
mod durability_tests {
    use super::*;
    thread_local! {
        pub(super) static SYNC: std::cell::RefCell<(bool,usize)> = const { std::cell::RefCell::new((false,0)) };
    }
    #[test]
    fn parent_sync_failure_keeps_store_and_retry_revalidates_without_reset() {
        // Other library tests spawn processes concurrently. A pre-exec child
        // may temporarily retain an inherited flock even with O_CLOEXEC.
        // Run this exact crash/reopen contract after exec in its own test process:
        // no retries, weakened Busy acceptance or global test serialization.
        const CHILD: &str = "RAD_TEST_DURABILITY_ISOLATED";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .env_clear()
                .env(CHILD, "1")
                .args([
                    "--exact",
                    "plugin_admin::worker::durability_tests::parent_sync_failure_keeps_store_and_retry_revalidates_without_reset",
                    "--nocapture",
                ])
                .status()
                .unwrap();
            assert!(status.success(), "isolated durability contract failed");
            return;
        }
        let parent = std::env::temp_dir().join(format!("rad-d1-parent-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&parent)
            .unwrap();
        let root = parent.join("plugins");
        SYNC.with(|s| *s.borrow_mut() = (true, 0));
        let first = open_store(&root);
        let calls = SYNC.with(|s| s.borrow().1);
        let failed = matches!(first, Err(Error::Io));
        drop(first);
        // Already-initialized contents must survive a failed parent sync.
        assert!(Manager::open(&root, 1, rad_plugin_host::TARGET).is_ok());
        SYNC.with(|s| *s.borrow_mut() = (false, 0));
        let reopened = open_store(&root).unwrap();
        assert!(reopened.installed().unwrap().is_empty());
        drop(reopened);
        let retried = SYNC.with(|s| s.borrow().1);
        let index = std::fs::read(root.join("index.json")).unwrap();
        let (commands, jobs) = mpsc::sync_channel(1);
        let (results, replies) = mpsc::sync_channel(1);
        let worker_root = root.clone();
        let worker = std::thread::spawn(move || {
            SYNC.with(|s| *s.borrow_mut() = (true, 0));
            run(
                worker_root,
                jobs,
                results,
                Arc::new(|| {}),
                Box::new(|_, _| panic!("no download on failed boot")),
                rad_plugin_manager::Cancellation::default(),
            );
        });
        assert!(matches!(
            replies
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            Reply::Boot(Err(Error::Io))
        ));
        commands.send(Job::Stop).unwrap();
        worker.join().unwrap();
        assert_eq!(std::fs::read(root.join("index.json")).unwrap(), index);
        let existing = parent.join("uninitialized");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&existing)
            .unwrap();
        assert!(
            open_store(&existing).is_err(),
            "existing empty directory is not adopted"
        );
        assert!(!existing.join("index.json").exists());
        std::fs::remove_dir_all(parent).unwrap();
        assert!(
            failed,
            "parent fsync failure must prevent usable Manager/Boot"
        );
        assert_eq!(
            calls, 1,
            "sync must happen after initialization, before usable Boot"
        );
        assert_eq!(
            retried, 1,
            "existing store must revalidate parent durability"
        );
    }
}

#[cfg(all(test, feature = "fixture-tests"))]
mod prepare_recovery_tests {
    use super::*;
    #[test]
    fn same_worker_reconciles_after_manifest_and_accepts_retry() {
        let root =
            std::env::temp_dir().join(format!("rad-followup-worker-retry-{}", std::process::id()));
        assert!(!root.exists());
        let package = crate::plugin_admin::lifecycle_tests::package(1);
        let (commands, jobs) = mpsc::sync_channel(1);
        let (results, replies) = mpsc::sync_channel(1);
        let worker_root = root.clone();
        let worker = std::thread::spawn(move || {
            rad_plugin_manager::fail_next_prepare_after_manifest();
            run(
                worker_root,
                jobs,
                results,
                Arc::new(|| {}),
                Box::new(move |_, _| Ok(package.clone())),
                rad_plugin_manager::Cancellation::default(),
            );
        });
        let receive = || {
            replies
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap()
        };
        assert!(matches!(receive(), Reply::Boot(Ok(_))));
        let request = PluginAdmin {
            operation: rad_protocol::PluginOperation::Install,
            plugin_id: None,
            source: Some("https://github.com/owner/repo".into()),
        };
        commands.send(Job::Prepare(request.clone())).unwrap();
        assert!(
            matches!(receive(), Reply::Prepared(Err(Error::Io), true)),
            "failed staging must reconcile on same worker and retain original error"
        );
        commands.send(Job::Prepare(request)).unwrap();
        let Reply::Prepared(Ok(prepared), _) = receive() else {
            panic!("retry without restart");
        };
        commands.send(Job::Commit(prepared)).unwrap();
        assert!(matches!(
            receive(),
            Reply::Committed(_, CommitOutcome::Applied, Ok(_))
        ));
        commands.send(Job::Stop).unwrap();
        worker.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
