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
            Job::Prepare { request, package } => {
                let result = prepare(m, request, package);
                Reply::Prepared(result.map(Box::new), m.state() == ManagerState::Ready)
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
    package: Option<Vec<u8>>,
) -> Result<PreparedChange, Error> {
    let source = request
        .source
        .as_deref()
        .map(rad_plugin_manager::GithubSource::parse)
        .transpose()?;
    // Only the identity of the confirmed origin matters; the Admin already vetted the request.
    let replaces = request
        .replaces_source
        .as_deref()
        .and_then(|url| rad_plugin_manager::GithubSource::parse(url).ok());
    let id = request.plugin_id.unwrap_or_default();
    let operation = match request.operation {
        rad_protocol::PluginOperation::Install => Operation::Install,
        rad_protocol::PluginOperation::Update => Operation::Update(id),
        rad_protocol::PluginOperation::Reinstall => Operation::Reinstall(id),
        rad_protocol::PluginOperation::Rollback => Operation::Rollback(id),
        rad_protocol::PluginOperation::Uninstall => Operation::Uninstall(id),
    };
    manager.prepare_from_source(
        operation,
        package.as_deref(),
        source.as_ref(),
        replaces.as_ref(),
    )
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
    thread_local! {
        pub(super) static SYNC: std::cell::RefCell<(bool,usize)> = const { std::cell::RefCell::new((false,0)) };
    }
}
