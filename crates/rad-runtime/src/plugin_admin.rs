//! Single worker and single operation credit, retained through commit, spawn and reap.
//! The download runs on its own thread; the worker never waits for the network.
mod fetch;
mod messages;
mod worker;
use rad_authority::{
    Authority,
    plugins::{PluginChange, PluginError, PluginPermit, Selection},
};
use rad_core::configuration::Origin;
use rad_plugin_contract::Manifest;
use rad_plugin_host::{Host, State};
use rad_plugin_manager::{ChangeKey, CommitOutcome, InstalledPlugin, PreparedChange};
use rad_protocol::PluginAdmin;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
};
type Download = Box<
    dyn Fn(&str, &rad_plugin_manager::Cancellation) -> Result<Vec<u8>, rad_plugin_manager::Error>
        + Send,
>;
enum Job {
    /// `package` is the downloaded archive; the worker validates and stages it.
    Prepare {
        request: PluginAdmin,
        package: Option<Vec<u8>>,
    },
    Commit(PreparedChange),
    Abort(PreparedChange),
    Spawn {
        id: String,
        ticket: rad_plugin_host::LaunchRequest,
    },
    Stop,
}
enum Reply {
    Boot(Result<Vec<InstalledPlugin>, rad_plugin_manager::Error>),
    Prepared(Result<Box<PreparedChange>, rad_plugin_manager::Error>, bool),
    Committed(
        ChangeKey,
        CommitOutcome,
        Result<Vec<InstalledPlugin>, rad_plugin_manager::Error>,
    ),
    Aborted(Result<(), rad_plugin_manager::Error>),
    Spawned(Box<rad_plugin_host::Prepared>),
}
struct Activation {
    id: String,
    manifest: Option<Manifest>,
    permit: Option<PluginPermit>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdminState {
    Starting,
    Failed,
    Idle,
    Preparing,
    Uncertain,
    Committing,
    Aborting,
    Retiring,
    Spawning,
    Handshake,
}
impl AdminState {
    fn wire(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Failed => "failed",
            Self::Idle => "idle",
            Self::Preparing => "preparing",
            Self::Uncertain => "uncertain",
            Self::Committing => "committing",
            Self::Aborting => "aborting",
            Self::Retiring => "activating",
            Self::Spawning => "activating",
            Self::Handshake => "activating",
        }
    }
}
/// Why `submit` refused a request: the wire code and, for the few codes that carry one,
/// the structured details the client needs to explain it.
#[derive(Debug, PartialEq)]
pub(crate) struct Rejection {
    pub code: &'static str,
    pub details: Value,
}
impl From<&'static str> for Rejection {
    fn from(code: &'static str) -> Self {
        Self {
            code,
            details: json!({}),
        }
    }
}
pub(crate) struct Admin {
    tx: mpsc::SyncSender<Job>,
    rx: mpsc::Receiver<Reply>,
    worker: Option<thread::JoinHandle<()>>,
    fetcher: fetch::Fetcher,
    unsent: Option<Job>,
    state: AdminState,
    error: Option<&'static str>,
    busy: bool,
    locked: bool,
    stopping: bool,
    stop_sent: bool,
    cancel: rad_plugin_manager::Cancellation,
    next: u64,
    operation: Option<u64>,
    installed: Vec<InstalledPlugin>,
    startup: VecDeque<Manifest>,
    active: Option<Activation>,
    change_key: Option<ChangeKey>,
    retained: Option<rad_plugin_host::Prepared>,
    demanded: std::collections::BTreeSet<String>,
    attempted: std::collections::BTreeSet<(String, u64)>,
}
impl Admin {
    pub fn new(root: PathBuf, wake: Arc<dyn Fn() + Send + Sync>, download: Download) -> Self {
        let (tx, jobs) = mpsc::sync_channel(1);
        let (results, rx) = mpsc::sync_channel(1);
        let cancel = rad_plugin_manager::Cancellation::default();
        let worker_wake = wake.clone();
        let worker = thread::Builder::new()
            .name("rad-plugin-admin".into())
            .spawn(move || worker::run(root, jobs, results, worker_wake))
            .ok();
        let failed = worker.is_none();
        Self {
            tx,
            rx,
            worker,
            fetcher: fetch::Fetcher::new(wake, download),
            unsent: None,
            state: if failed {
                AdminState::Failed
            } else {
                AdminState::Starting
            },
            error: failed.then_some("resource_exhausted"),
            busy: !failed,
            locked: failed,
            stopping: false,
            stop_sent: false,
            cancel,
            next: 0,
            operation: None,
            installed: vec![],
            startup: VecDeque::new(),
            active: None,
            change_key: None,
            retained: None,
            demanded: Default::default(),
            attempted: Default::default(),
        }
    }
    pub fn submit(
        &mut self,
        origin: Origin,
        request: PluginAdmin,
        actor: &Authority,
    ) -> Result<Value, Rejection> {
        if origin != Origin::Editor {
            return Err("permission_denied".into());
        }
        if self.busy || self.fetcher.active() || self.locked || self.stopping {
            return Err("resource_exhausted".into());
        }
        if let Some(id) = &request.plugin_id {
            actor.plugin_change_allowed(id).map_err(plugin_error)?;
        }
        self.confirm_source(&request)?;
        let next = self.next.checked_add(1).ok_or("resource_exhausted")?;
        self.fetcher
            .start(request, &self.cancel)
            .map_err(|_| "resource_exhausted")?;
        self.next = next;
        self.operation = Some(next);
        self.error = None;
        self.state = AdminState::Preparing;
        Ok(json!({"operation_id":next.to_string(),"state":"preparing"}))
    }
    /// An update from another repository than the registered one must carry the user's
    /// confirmation of exactly that registered origin. Refusing here, before any download,
    /// also covers a registered origin that changed since the user looked at it.
    fn confirm_source(&self, request: &PluginAdmin) -> Result<(), Rejection> {
        use rad_plugin_manager::GithubSource;
        use rad_protocol::PluginOperation::{Reinstall, Update};
        if !matches!(request.operation, Update | Reinstall) {
            return Ok(());
        }
        let registered = self
            .installed
            .iter()
            .find(|item| Some(&item.manifest.id) == request.plugin_id.as_ref())
            .and_then(|item| item.source.as_deref());
        let (Some(registered), Some(requested)) = (registered, request.source.as_deref()) else {
            return Ok(());
        };
        // An unreadable request is the downloader's `invalid_source`; an unreadable
        // registered origin cannot happen (the ledger only keeps canonical ones).
        let (Ok(known), Ok(wanted)) = (
            GithubSource::parse(registered),
            GithubSource::parse(requested),
        ) else {
            return Ok(());
        };
        let confirmed = request
            .replaces_source
            .as_deref()
            .and_then(|url| GithubSource::parse(url).ok())
            .is_some_and(|replaces| replaces.same_origin(&known));
        if known.same_origin(&wanted) || confirmed {
            return Ok(());
        }
        Err(Rejection {
            code: "source_changed",
            details: json!({"registered": known.canonical_url(), "requested": wanted.canonical_url()}),
        })
    }
    fn send(&mut self, job: Job) {
        // Only continuation of the current operation: never a second admin request.
        match self.tx.try_send(job) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(job)) => self.unsent = Some(job),
            Err(mpsc::TrySendError::Disconnected(job)) => {
                self.unsent = Some(job);
                self.locked = true;
                self.state = AdminState::Uncertain;
                self.error = Some("recovery_required");
            }
        }
    }
    fn finish(&mut self, actor: &mut Authority, applied: bool, error: Option<&'static str>) {
        if let Some(active) = &mut self.active
            && active.permit.is_some()
            && actor
                .finish_plugin_change(&mut active.permit, applied)
                .is_err()
        {
            self.locked = true;
            self.state = AdminState::Uncertain;
            self.error = Some("recovery_required");
            return;
        }
        self.active = None;
        self.change_key = None;
        self.busy = false;
        self.error = error;
        self.state = if error.is_some() {
            AdminState::Failed
        } else {
            AdminState::Idle
        };
    }
    pub fn poll(&mut self, actor: &mut Authority, host: &mut Host, now: u64) {
        if let Some(job) = self.unsent.take() {
            self.send(job);
        }
        if let Ok(reply) = self.rx.try_recv() {
            self.handle_reply(reply, actor, host, now);
        }
        while !self.busy
            && !self.locked
            && !self.stopping
            && let Some(manifest) = self.startup.pop_front()
        {
            match actor.register_plugin(manifest.clone(), rad_plugin_host::TARGET) {
                Ok(()) => self.error = None,
                Err(e) => {
                    self.error = Some(plugin_error(e));
                    if transient(e) {
                        self.startup.push_front(manifest);
                        break;
                    }
                }
            }
        }
        if self.state == AdminState::Retiring {
            let active = self.active.as_ref().unwrap();
            if host.remove(&active.id).is_ok() {
                if self.stopping {
                    self.finish(actor, true, None);
                } else if let Some(manifest) = active.manifest.clone() {
                    match actor.register_plugin(manifest, rad_plugin_host::TARGET) {
                        Ok(()) => self.finish(actor, true, None),
                        Err(e) if transient(e) => self.error = Some(plugin_error(e)),
                        Err(e) => self.finish(actor, true, Some(plugin_error(e))),
                    }
                } else {
                    self.finish(actor, true, None);
                }
            }
        }
        if self.state == AdminState::Handshake {
            let active = self.active.as_ref().unwrap();
            match host.state(&active.id) {
                Some(State::Ready) => {
                    let (m, g, _) = host.bindings().find(|(m, _, _)| m.id == active.id).unwrap();
                    let selection = actor
                        .plugin_manifests()
                        .find(|(selected, _, _)| selected.id == m.id)
                        .map(|(_, s, _)| s);
                    match selection
                        .ok_or(PluginError::Incompatible)
                        .and_then(|s| actor.activate_plugin(&m.id, s, g))
                    {
                        Ok(()) => self.finish(actor, true, None),
                        Err(e) if transient(e) => {
                            self.error = Some(plugin_error(e));
                        }
                        Err(e) => {
                            host.retire(&active.id);
                            self.finish(actor, true, Some(plugin_error(e)));
                        }
                    }
                }
                Some(State::Failed | State::Reaped) => {
                    let id = active.id.clone();
                    let selection = actor
                        .plugin_manifests()
                        .find(|(m, _, _)| m.id == id)
                        .map(|(_, s, _)| s);
                    if let Some(selection) = selection {
                        let _ = actor.fail_plugin_selection(&id, selection);
                    }
                    self.finish(actor, true, Some("plugin_failed"))
                }
                _ => {}
            }
        }
        self.fetcher.collect();
        self.dispatch_fetched();
        self.reconcile_demand(actor, host, now);
        if self.stopping && (!self.busy || self.locked) && !self.stop_sent && self.unsent.is_none()
        {
            self.stop_sent = true;
            self.send(Job::Stop);
        }
    }
    /// Hands a returned download to the worker once it is idle. Selector, permits and
    /// publication stay here: the permit is born only in `Reply::Prepared`.
    fn dispatch_fetched(&mut self) {
        if self.stopping || self.locked {
            // A download that returns after shutdown (or a lock) is dropped unused.
            if self.fetcher.take_done().is_some() && !self.locked {
                self.error = Some("cancelled");
                if !self.busy {
                    self.state = AdminState::Failed;
                }
            }
            return;
        }
        if self.busy || self.unsent.is_some() {
            return;
        }
        match self.fetcher.take_done() {
            None => {}
            Some((_, Err(e))) => {
                self.error = Some(manager_error(e));
                self.state = AdminState::Failed;
            }
            Some((request, Ok(package))) => {
                self.busy = true;
                self.state = AdminState::Preparing;
                self.send(Job::Prepare { request, package });
            }
        }
    }
    /// Explicit repair of the same failed selection, only after its process was reaped.
    /// No package operation, configuration edit, credential replacement or action replay.
    pub fn can_retry(&self, id: &str, actor: &Authority, host: &Host) -> bool {
        !self.busy
            && !self.fetcher.active()
            && !self.locked
            && !self.stopping
            && self.startup.is_empty()
            && host.state(id) == Some(State::Reaped)
            && !host.stopped_normally(id)
            && actor.plugin_change_allowed(id).is_ok()
            && actor.plugin_manifests().any(|(manifest, selection, _)| {
                manifest.id == id
                    && !actor.plugin_available(id, selection)
                    && self.attempted.contains(&(id.to_owned(), selection))
            })
    }
    pub fn retry(
        &mut self,
        origin: Origin,
        id: &str,
        actor: &Authority,
        host: &mut Host,
    ) -> Result<(), &'static str> {
        if origin != Origin::Editor {
            return Err("permission_denied");
        }
        if !self.can_retry(id, actor, host) {
            return Err("operation_not_allowed");
        }
        let selection = actor
            .plugin_manifests()
            .find(|(manifest, _, _)| manifest.id == id)
            .map(|(_, selection, _)| selection)
            .ok_or("not_found")?;
        host.remove(id).map_err(|_| "plugin_residual")?;
        self.attempted.remove(&(id.to_owned(), selection));
        self.error = None;
        self.state = AdminState::Idle;
        Ok(())
    }
    pub fn set_extra_demand(&mut self, demand: std::collections::BTreeSet<String>) {
        self.demanded = demand;
    }
    fn reconcile_demand(&mut self, actor: &mut Authority, host: &mut Host, now: u64) {
        if self.stopping || self.locked {
            return;
        }
        let mut desired = actor.plugin_demand();
        desired.extend(self.demanded.iter().cloned());
        let selections: std::collections::BTreeSet<_> = actor
            .plugin_manifests()
            .map(|(m, s, _)| (m.id.clone(), s))
            .collect();
        self.attempted.retain(|key| selections.contains(key));
        let workers: Vec<_> = host
            .bindings()
            .map(|(m, g, s)| (m.id.clone(), g, s))
            .collect();
        for (id, generation, state) in workers {
            if self.active.as_ref().is_some_and(|a| a.id == id) {
                continue;
            }
            if state == State::Reaped && host.stopped_normally(&id) {
                if actor.plugin_generation(&id).is_none() {
                    let _ = host.remove(&id);
                }
                continue;
            }
            if !desired.contains(&id)
                && state == State::Ready
                && host.idle(&id)
                && actor.begin_plugin_stop(&id, generation).is_ok()
            {
                self.attempted.retain(|(plugin, _)| plugin != &id);
                host.retire(&id);
            }
            // Failed tombstones retain their diagnosis until explicit repair.
        }
        if self.busy || !self.startup.is_empty() {
            return;
        }
        let selected: Vec<_> = actor
            .plugin_manifests()
            .filter(|(m, _, _)| desired.contains(&m.id))
            .map(|(m, s, _)| (m.clone(), s))
            .collect();
        for (manifest, selection) in selected {
            let id = manifest.id.clone();
            if self.attempted.contains(&(id.clone(), selection)) {
                continue;
            }
            if let Some(state) = host.state(&id)
                && (state != State::Reaped || host.remove(&id).is_err())
            {
                continue;
            }
            self.attempted.insert((id.clone(), selection));
            match host.reserve_launch(manifest.clone(), now) {
                Ok(ticket) => {
                    self.active = Some(Activation {
                        id: id.clone(),
                        manifest: Some(manifest),
                        permit: None,
                    });
                    self.busy = true;
                    self.state = AdminState::Spawning;
                    self.send(Job::Spawn { id, ticket });
                }
                Err(_) => {
                    let _ = actor.fail_plugin_selection(&id, selection);
                }
            }
            break;
        }
    }
    pub fn export_dependencies(
        &self,
    ) -> std::collections::BTreeMap<String, rad_core::configuration::PluginDependency> {
        self.installed
            .iter()
            .map(|item| {
                let v = item.manifest.version;
                (
                    item.manifest.id.clone(),
                    rad_core::configuration::PluginDependency {
                        version: Some(rad_core::configuration::PluginVersion {
                            major: v.major,
                            minor: v.minor,
                            patch: v.patch,
                        }),
                        origin: item.source.clone(),
                    },
                )
            })
            .collect()
    }
    /// Inventory includes preserved packages that cannot enter Authority or Host.
    pub fn plugins_needing_update(&self) -> impl Iterator<Item = &str> {
        self.installed.iter().filter_map(|item| {
            item.manifest
                .validate(rad_plugin_contract::HOST_API, rad_plugin_host::TARGET)
                .is_err()
                .then_some(item.manifest.id.as_str())
        })
    }
    pub fn status(&self, host: &Host) -> Value {
        let items: Vec<_> = self
            .installed
            .iter()
            .map(|item| {
                let id = &item.manifest.id;
                let host_state = match host.state(id) {
                    None => "absent",
                    Some(State::Starting) => "starting",
                    Some(State::Ready) => "ready",
                    Some(State::Failed) if host.stopped_normally(id) => "absent",
                    Some(State::Reaped) if host.stopped_normally(id) => "absent",
                    Some(State::Failed) => "residual",
                    Some(State::Reaped) => "failed",
                };
                json!({
                    "id": id,
                    "selected": item.manifest.version,
                    "previous": item.previous.as_ref().map(|m| m.version),
                    "source": item.source,
                    "source_trusted": item.source.is_some(),
                    "diagnostic": if item.manifest.validate(rad_plugin_contract::HOST_API, rad_plugin_host::TARGET).is_err() { Some("incompatible_plugin") } else { host.last_diagnostic(id).map(diagnostic_code) },
                    "host": host_state,
                })
            })
            .collect();
        json!({
            // The download is part of the visible operation although the worker is free.
            "state": if self.fetcher.active() { AdminState::Preparing.wire() } else { self.state.wire() },
            "operation_id": self.operation.map(|n| n.to_string()),
            "error": self.error,
            "locked": self.locked,
            "items": items,
        })
    }

    pub fn shutdown(&mut self) {
        self.stopping = true;
        self.cancel.cancel();
    }
    pub fn stopped(&self) -> bool {
        self.worker.as_ref().is_none_or(|w| w.is_finished()) && self.fetcher.stopped()
    }
}
fn diagnostic_code(d: rad_plugin_host::Diagnostic) -> &'static str {
    use rad_plugin_host::Diagnostic::*;
    match d {
        Protocol => "protocol",
        Pipe => "pipe",
        Deadline => "deadline",
        Exited => "exited",
        Backpressure => "backpressure",
    }
}
fn transient(e: PluginError) -> bool {
    matches!(
        e,
        PluginError::PublicationPending | PluginError::Residual | PluginError::InUse
    )
}
fn plugin_error(e: PluginError) -> &'static str {
    match e {
        PluginError::InUse => "plugin_in_use",
        PluginError::Residual => "plugin_residual",
        PluginError::PublicationPending => "publication_pending",
        PluginError::PermissionDenied => "permission_denied",
        PluginError::PluginLimit => "plugin_limit",
        PluginError::Incompatible => "incompatible_plugin",
        PluginError::InvalidPermit => "recovery_required",
    }
}
fn manager_error(e: rad_plugin_manager::Error) -> &'static str {
    use rad_plugin_manager::Error as E;
    match e {
        E::InvalidSource => "invalid_source",
        E::InvalidPackage | E::AmbiguousAsset => "invalid_package",
        E::Incompatible => "incompatible_plugin",
        E::Network => "network",
        E::PluginLimit => "plugin_limit",
        E::Deadline => "deadline",
        E::Cancelled => "cancelled",
        E::Capacity | E::Busy | E::LimitExceeded => "resource_exhausted",
        E::Io => "io",
        E::AlreadyInstalled => "plugin_already_installed",
        E::PluginMismatch => "plugin_mismatch",
        E::SameVersion => "plugin_same_version",
        E::SourceChanged => "source_changed",
        // No wildcard: a new manager error must choose its code, not inherit this one.
        E::CorruptState | E::WrongOwner | E::InvalidOperation | E::RecoveryRequired => {
            "recovery_required"
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use rad_core::{configuration::Configuration, identity::IdentityRegistry};
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };
    #[test]
    fn shutdown_cancels_cooperative_download_and_joins_admin_worker() {
        let root =
            std::env::temp_dir().join(format!("rad-followup-admin-cancel-{}", std::process::id()));
        assert!(!root.exists());
        let (entered, entering) = mpsc::sync_channel(1);
        let mut admin = Admin::new(
            root.clone(),
            Arc::new(|| {}),
            Box::new(move |_, cancel| {
                entered.send(()).unwrap();
                while !cancel.is_cancelled() {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(rad_plugin_manager::Error::Cancelled)
            }),
        );
        let c = Configuration::prepare_new(&IdentityRegistry::new(505).unwrap()).unwrap();
        let mut actor = Authority::new(c, 1, std::num::NonZeroU128::new(507).unwrap());
        let mut host = Host::default();
        let deadline = Instant::now() + Duration::from_secs(3);
        while admin.busy {
            admin.poll(&mut actor, &mut host, 0);
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        admin
            .submit(
                Origin::Editor,
                PluginAdmin {
                    operation: rad_protocol::PluginOperation::Install,
                    plugin_id: None,
                    source: Some("https://github.com/owner/repo".into()),
                    replaces_source: None,
                },
                &actor,
            )
            .unwrap();
        entering.recv_timeout(Duration::from_secs(2)).unwrap();
        admin.shutdown();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !admin.stopped() && Instant::now() < deadline {
            admin.poll(&mut actor, &mut host, 0);
            thread::sleep(Duration::from_millis(1));
        }
        let stopped = admin.stopped();
        // Cleanup even for the deliberately broken red control.
        admin.cancel.cancel();
        let cleanup = Instant::now() + Duration::from_secs(2);
        while !admin.stopped() {
            admin.poll(&mut actor, &mut host, 0);
            assert!(Instant::now() < cleanup);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(admin.installed.is_empty());
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            stopped,
            "shutdown must signal the running cooperative download"
        );
    }
    #[test]
    fn worker_retains_single_credit_until_blocked_download_really_returns() {
        let root = std::env::temp_dir().join(format!("rad-d-admin-{}", std::process::id()));
        assert!(!root.exists());
        let release = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(AtomicBool::new(false));
        let r = release.clone();
        let e = entered.clone();
        let mut admin = Admin::new(
            root.clone(),
            Arc::new(|| {}),
            Box::new(move |_, _| {
                e.store(true, Ordering::SeqCst);
                while !r.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(rad_plugin_manager::Error::Network)
            }),
        );
        let c = Configuration::prepare_new(&IdentityRegistry::new(5).unwrap()).unwrap();
        let mut actor = Authority::new(c, 1, std::num::NonZeroU128::new(7).unwrap());
        let mut host = Host::default();
        let end = Instant::now() + Duration::from_secs(3);
        while admin.status(&host)["state"] == "starting" {
            assert!(Instant::now() < end, "startup worker did not finish");
            admin.poll(&mut actor, &mut host, 0);
            std::thread::sleep(Duration::from_millis(1));
        }
        let request = PluginAdmin {
            operation: rad_protocol::PluginOperation::Install,
            plugin_id: None,
            source: Some("https://github.com/KroxiDev/rad-plugin-fixture".into()),
            replaces_source: None,
        };
        assert_eq!(
            admin.submit(Origin::Automation, request.clone(), &actor),
            Err("permission_denied".into())
        );
        assert!(
            admin
                .submit(Origin::Editor, request.clone(), &actor)
                .is_ok()
        );
        while !entered.load(Ordering::SeqCst) {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            admin.submit(Origin::Editor, request.clone(), &actor),
            Err("resource_exhausted".into())
        );
        admin.shutdown(); // Closing/shutdown cannot make a blocked worker disappear.
        assert!(!admin.stopped());
        release.store(true, Ordering::SeqCst);
        while !admin.stopped() {
            assert!(Instant::now() < end);
            admin.poll(&mut actor, &mut host, 0);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(host.resources(), (0, 0));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, feature = "fixture-tests"))]
mod lifecycle_tests {
    use super::*;
    use rad_authority::{Command, Effect, Outcome};
    use rad_core::{configuration::*, execution::Target, identity::IdentityRegistry};
    use std::time::{Duration, Instant};
    pub(super) fn package(major: u32) -> Vec<u8> {
        package_for("org.rad.fixture", major)
    }
    fn package_for(id: &str, major: u32) -> Vec<u8> {
        let name = format!("rad-plugin-fixture-v{major}");
        let bin = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(name);
        let executable = std::fs::read(bin).unwrap();
        let mut manifest: Manifest = serde_json::from_slice(include_bytes!(
            "../../rad-plugin-host/fixture/manifest-v1.json"
        ))
        .unwrap();
        manifest.id = id.into();
        manifest.version.major = major;
        if major == 2 {
            manifest.actions[0]
                .schema
                .fields
                .push(rad_plugin_contract::Field {
                    id: "new".into(),
                    label: "New".into(),
                    required: false,
                    kind: rad_plugin_contract::FieldKind::Boolean,
                });
        }
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let candidate = rad_plugin_package::Candidate::from_parts(
            &manifest,
            &executable,
            rad_plugin_contract::HOST_API,
            rad_plugin_host::TARGET,
        )
        .unwrap();
        rad_plugin_package::encode(&candidate)
    }
    fn tick(
        admin: &mut Admin,
        actor: &mut Authority,
        ports: &mut crate::plugin_dispatch::Plugins,
        now: u64,
    ) {
        let effects = actor.before_event(now).unwrap();
        for e in effects {
            if let Effect::Dispatch(d) = e {
                ports.submit(actor, &d).unwrap();
            }
        }
        for (id, result) in ports.pump(actor, now).unwrap() {
            actor.job_complete(id, result).unwrap();
        }
        admin.poll(actor, &mut ports.host, now);
    }
    fn settle(
        admin: &mut Admin,
        actor: &mut Authority,
        ports: &mut crate::plugin_dispatch::Plugins,
        now: u64,
    ) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            tick(admin, actor, ports, now);
            if !admin.busy && !admin.fetcher.active() && admin.startup.is_empty() {
                break;
            }
            assert!(
                Instant::now() < end,
                "admin watchdog: {}",
                admin.status(&ports.host)
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn request(op: &str, version: Option<u32>) -> PluginAdmin {
        PluginAdmin {
            operation: serde_json::from_value(serde_json::json!(op)).unwrap(),
            plugin_id: (op != "install").then(|| "org.rad.fixture".into()),
            source: version.map(|v| format!("https://github.com/owner/repo/releases/tag/v{v}")),
            replaces_source: None,
        }
    }
    #[test]
    fn real_worker_versions_wait_guard_rollback_compatibility_and_placeholders_survive() {
        let root = std::env::temp_dir().join(format!("rad-d-lifecycle-{}", std::process::id()));
        assert!(!root.exists());
        let v1 = package(1);
        let v2 = package(2);
        let mut admin = Admin::new(
            root.clone(),
            Arc::new(|| {}),
            Box::new(move |source, _| {
                Ok(if source.ends_with("/v1") {
                    v1.clone()
                } else {
                    v2.clone()
                })
            }),
        );
        let mut c = Configuration::prepare_new(&IdentityRegistry::new(91).unwrap()).unwrap();
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::CreateButton {
                    folder: c.configuration().data().principal,
                    position: 0,
                    name: "Preserved".into(),
                },
            )
            .unwrap();
        let button = c.configuration().data().buttons[0].binding;
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::AddAction {
                    button: button.identity(),
                    kind: ActionKind::Wait(Wait::new(100).unwrap()),
                },
            )
            .unwrap();
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::AddAction {
                    button: button.identity(),
                    kind: ActionKind::External {
                        type_key: ExternalType::new("org.rad.fixture".into(), "run".into())
                            .unwrap(),
                        parameters: br#"{"schema_version":1,"values":{"mode":"echo"}}"#.to_vec(),
                    },
                },
            )
            .unwrap();
        let action = c.configuration().data().buttons[0].actions[1].binding;
        let mut actor = Authority::new(c, 1, std::num::NonZeroU128::new(17).unwrap());
        let mut ports = crate::plugin_dispatch::Plugins::default();
        settle(&mut admin, &mut actor, &mut ports, 0);
        admin
            .submit(Origin::Editor, request("install", Some(1)), &actor)
            .unwrap();
        settle(&mut admin, &mut actor, &mut ports, 0);
        assert_eq!(ports.host.state("org.rad.fixture"), Some(State::Ready));
        assert_eq!(
            admin.status(&ports.host)["items"][0]["selected"]["major"],
            1
        );
        let t = actor.reserve(1, Origin::Editor).unwrap();
        let effects = actor.submit(
            t,
            Command::Run {
                target: Target::Button(button.identity()),
            },
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::Accepted { .. })));
        assert_eq!(
            admin.submit(Origin::Editor, request("update", Some(2)), &actor),
            Err("plugin_in_use".into())
        );
        let end = Instant::now() + Duration::from_secs(3);
        while actor.plugin_change_allowed("org.rad.fixture").is_err() {
            tick(&mut admin, &mut actor, &mut ports, 100);
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            admin.status(&ports.host)["items"][0]["selected"]["major"],
            1,
            "rejected update never replays"
        );
        for (op, version, selected) in [
            ("update", Some(2), 2),
            ("rollback", None, 1),
            ("update", Some(2), 2),
        ] {
            admin
                .submit(Origin::Editor, request(op, version), &actor)
                .unwrap();
            settle(&mut admin, &mut actor, &mut ports, 100);
            assert_eq!(admin.error, None);
            assert_eq!(
                admin.status(&ports.host)["items"][0]["selected"]["major"],
                selected
            );
            assert_eq!(ports.host.state("org.rad.fixture"), Some(State::Ready));
        }
        let bytes = br#"{"schema_version":1,"values":{"mode":"echo","new":true}}"#.to_vec();
        let t = actor.reserve(2, Origin::Editor).unwrap();
        let e = actor.submit(
            t,
            vec![Edit::SetParameters {
                action: action.identity(),
                parameters: ActionParameters::External(bytes.clone()),
            }],
        );
        let id = e
            .iter()
            .find_map(|e| {
                if let Effect::Persist { id, .. } = e {
                    Some(*id)
                } else {
                    None
                }
            })
            .unwrap();
        actor.complete(
            id,
            Outcome::Applied {
                revision: 2,
                changed: true,
            },
        );
        let before = actor.editor_view(Origin::Editor).unwrap();
        admin
            .submit(Origin::Editor, request("rollback", None), &actor)
            .unwrap();
        settle(&mut admin, &mut actor, &mut ports, 100);
        assert_eq!(admin.error, Some("incompatible_plugin"));
        assert_eq!(
            admin.status(&ports.host)["items"][0]["selected"]["major"],
            2
        );
        admin
            .submit(Origin::Editor, request("uninstall", None), &actor)
            .unwrap();
        settle(&mut admin, &mut actor, &mut ports, 100);
        assert_eq!(
            actor
                .plugin_configuration(Origin::Editor, action.short())
                .unwrap()
                .1,
            bytes
        );
        assert_eq!(actor.editor_view(Origin::Editor).unwrap(), before);
        admin
            .submit(Origin::Editor, request("reinstall", Some(2)), &actor)
            .unwrap();
        settle(&mut admin, &mut actor, &mut ports, 100);
        assert_eq!(ports.host.state("org.rad.fixture"), Some(State::Ready));
        assert_eq!(
            actor
                .plugin_configuration(Origin::Editor, action.short())
                .unwrap()
                .1,
            bytes
        );
        admin.shutdown();
        ports.host.shutdown();
        while !admin.stopped() || ports.host.resources().0 > 0 {
            tick(&mut admin, &mut actor, &mut ports, 100);
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn held_download_of_one_plugin_does_not_delay_cold_start_of_another() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let root = std::env::temp_dir().join(format!("rad-b6-fetch-{}", std::process::id()));
        assert!(!root.exists());
        let (b_package, a_package) = (package(1), package_for("org.rad.other", 1));
        let release = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(AtomicBool::new(false));
        let (r, e) = (release.clone(), entered.clone());
        let mut admin = Admin::new(
            root.clone(),
            Arc::new(|| {}),
            Box::new(move |source, _| {
                if source.ends_with("/v1") {
                    return Ok(b_package.clone());
                }
                e.store(true, Ordering::SeqCst);
                while !r.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(a_package.clone())
            }),
        );
        let c = Configuration::prepare_new(&IdentityRegistry::new(95).unwrap()).unwrap();
        let mut actor = Authority::new(c, 1, std::num::NonZeroU128::new(23).unwrap());
        let mut ports = crate::plugin_dispatch::Plugins::default();
        settle(&mut admin, &mut actor, &mut ports, 0);
        admin
            .submit(Origin::Editor, request("install", Some(1)), &actor)
            .unwrap();
        settle(&mut admin, &mut actor, &mut ports, 0);
        assert_eq!(ports.host.state("org.rad.fixture"), None, "B starts cold");
        // Install A: its download is held by the fake downloader.
        admin
            .submit(Origin::Editor, request("install", Some(2)), &actor)
            .unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while !entered.load(Ordering::SeqCst) {
            tick(&mut admin, &mut actor, &mut ports, 0);
            assert!(Instant::now() < end, "download of A never started");
            std::thread::sleep(Duration::from_millis(1));
        }
        // First demand for B while A is still downloading.
        admin.set_extra_demand(["org.rad.fixture".into()].into());
        let cold_start = Instant::now() + Duration::from_secs(3);
        while ports.host.state("org.rad.fixture") != Some(State::Ready) {
            tick(&mut admin, &mut actor, &mut ports, 0);
            assert!(
                Instant::now() < cold_start,
                "B must reach Ready while A's download is held: {}",
                admin.status(&ports.host)
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let held = admin.status(&ports.host);
        assert_eq!(held["state"], "preparing", "A's operation stays visible");
        assert_eq!(
            admin.submit(Origin::Editor, request("install", Some(2)), &actor),
            Err("resource_exhausted".into()),
            "one administrative operation at a time"
        );
        release.store(true, Ordering::SeqCst);
        settle(&mut admin, &mut actor, &mut ports, 0);
        let done = admin.status(&ports.host);
        assert_eq!(done["error"], serde_json::Value::Null);
        assert_eq!(done["items"].as_array().unwrap().len(), 2, "A installed");
        assert_eq!(ports.host.state("org.rad.fixture"), Some(State::Ready));
        admin.set_extra_demand(Default::default());
        admin.shutdown();
        ports.host.shutdown();
        let end = Instant::now() + Duration::from_secs(5);
        while !admin.stopped() || ports.host.resources().0 > 0 {
            tick(&mut admin, &mut actor, &mut ports, 0);
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ordinary_rejections_report_their_own_code_and_leave_admin_usable() {
        let root = std::env::temp_dir().join(format!("rad-d-rejections-{}", std::process::id()));
        assert!(!root.exists());
        let v1 = package(1);
        let foreign = package_for("org.rad.other", 2);
        let mut admin = Admin::new(
            root.clone(),
            Arc::new(|| {}),
            Box::new(move |source, _| {
                Ok(if source.ends_with("/v1") {
                    v1.clone()
                } else {
                    foreign.clone()
                })
            }),
        );
        let c = Configuration::prepare_new(&IdentityRegistry::new(93).unwrap()).unwrap();
        let mut actor = Authority::new(c, 1, std::num::NonZeroU128::new(19).unwrap());
        let mut ports = crate::plugin_dispatch::Plugins::default();
        settle(&mut admin, &mut actor, &mut ports, 0);
        admin
            .submit(Origin::Editor, request("install", Some(1)), &actor)
            .unwrap();
        settle(&mut admin, &mut actor, &mut ports, 0);
        assert_eq!(admin.error, None);
        for (op, version, code) in [
            ("install", 1, "plugin_already_installed"),
            ("update", 2, "plugin_mismatch"),
            ("update", 1, "plugin_same_version"),
        ] {
            admin
                .submit(Origin::Editor, request(op, Some(version)), &actor)
                .unwrap();
            settle(&mut admin, &mut actor, &mut ports, 0);
            let status = admin.status(&ports.host);
            assert_eq!(status["error"], code, "{op}");
            assert_status_decodes(status.clone(), code);
            assert_eq!(status["locked"], false, "{op}: nothing to recover");
            assert_eq!(status["items"][0]["selected"]["major"], 1);
            assert_eq!(ports.host.state("org.rad.fixture"), None);
            assert_eq!(actor.plugin_manifests().count(), 1);
        }
        // A plugin that dies while activating reports `plugin_failed` the same way.
        admin.error = Some("plugin_failed");
        assert_status_decodes(admin.status(&ports.host), "plugin_failed");
        admin.shutdown();
        ports.host.shutdown();
        let end = Instant::now() + Duration::from_secs(3);
        while !admin.stopped() || ports.host.resources().0 > 0 {
            tick(&mut admin, &mut actor, &mut ports, 0);
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    const REGISTERED: &str = "https://github.com/owner/repo/releases/tag/v1";
    const OTHER: &str = "https://github.com/other/repo/releases/tag/v2";
    fn replacing(op: &str, source: &str, replaces: Option<&str>) -> PluginAdmin {
        PluginAdmin {
            source: Some(source.into()),
            replaces_source: replaces.map(str::to_owned),
            ..request(op, None)
        }
    }
    /// An Admin whose downloader counts its calls and serves v2 for anything but `/v1`.
    fn counting_admin(root: &std::path::Path) -> (Admin, Arc<std::sync::atomic::AtomicUsize>) {
        let (v1, v2) = (package(1), package(2));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = calls.clone();
        let admin = Admin::new(
            root.to_path_buf(),
            Arc::new(|| {}),
            Box::new(move |source, _| {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(if source.ends_with("/v1") {
                    v1.clone()
                } else {
                    v2.clone()
                })
            }),
        );
        (admin, calls)
    }
    fn actor_for(seed: u128) -> Authority {
        Authority::new(
            Configuration::prepare_new(&IdentityRegistry::new(seed).unwrap()).unwrap(),
            1,
            std::num::NonZeroU128::new(seed + 100).unwrap(),
        )
    }
    fn registered_source(admin: &Admin, ports: &crate::plugin_dispatch::Plugins) -> Value {
        admin.status(&ports.host)["items"][0]["source"].clone()
    }
    fn shut(
        mut admin: Admin,
        actor: &mut Authority,
        mut ports: crate::plugin_dispatch::Plugins,
        root: PathBuf,
    ) {
        admin.shutdown();
        ports.host.shutdown();
        let end = Instant::now() + Duration::from_secs(3);
        while !admin.stopped() || ports.host.resources().0 > 0 {
            tick(&mut admin, actor, &mut ports, 0);
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn another_repository_is_refused_without_downloading_until_the_registered_origin_is_confirmed()
    {
        for op in ["update", "reinstall"] {
            let root =
                std::env::temp_dir().join(format!("rad-b4-confirm-{op}-{}", std::process::id()));
            assert!(!root.exists());
            let (mut admin, calls) = counting_admin(&root);
            let mut actor = actor_for(201);
            let mut ports = crate::plugin_dispatch::Plugins::default();
            settle(&mut admin, &mut actor, &mut ports, 0);
            admin
                .submit(Origin::Editor, request("install", Some(1)), &actor)
                .unwrap();
            settle(&mut admin, &mut actor, &mut ports, 0);
            assert_eq!(registered_source(&admin, &ports), REGISTERED);
            let downloads = calls.load(std::sync::atomic::Ordering::SeqCst);
            // Missing and wrong confirmations are both refused, with both origins named.
            for replaces in [None, Some("https://github.com/third/repo")] {
                let refused = admin
                    .submit(Origin::Editor, replacing(op, OTHER, replaces), &actor)
                    .unwrap_err();
                assert_eq!(refused.code, "source_changed", "{op} {replaces:?}");
                assert_eq!(
                    refused.details,
                    json!({"registered": REGISTERED, "requested": OTHER})
                );
                assert_eq!(
                    calls.load(std::sync::atomic::Ordering::SeqCst),
                    downloads,
                    "nothing downloaded"
                );
                assert!(!admin.busy && !admin.fetcher.active() && admin.error.is_none());
                assert_eq!(registered_source(&admin, &ports), REGISTERED);
            }
            // The confirmed origin (any case, any tag) lets the change through.
            admin
                .submit(
                    Origin::Editor,
                    replacing(op, OTHER, Some("https://github.com/OWNER/Repo")),
                    &actor,
                )
                .unwrap();
            settle(&mut admin, &mut actor, &mut ports, 0);
            assert_eq!(admin.error, None, "{op}");
            assert_eq!(
                calls.load(std::sync::atomic::Ordering::SeqCst),
                downloads + 1
            );
            assert_eq!(registered_source(&admin, &ports), OTHER, "{op}");
            assert_eq!(
                admin.status(&ports.host)["items"][0]["selected"]["major"],
                2
            );
            shut(admin, &mut actor, ports, root);
        }
    }
    #[test]
    fn same_repository_needs_no_confirmation_whatever_the_tag_or_case() {
        for (n, (op, next, registered)) in [
            (
                "update",
                "https://github.com/owner/repo/releases/tag/v2",
                "https://github.com/owner/repo/releases/tag/v2",
            ),
            (
                "update",
                "https://github.com/OWNER/REPO.git",
                "https://github.com/OWNER/REPO",
            ),
            (
                "reinstall",
                "https://github.com/owner/repo/releases/tag/v2",
                "https://github.com/owner/repo/releases/tag/v2",
            ),
            (
                "reinstall",
                "https://github.com/OWNER/REPO.git",
                "https://github.com/OWNER/REPO",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let root = std::env::temp_dir().join(format!("rad-b4-same-{n}-{}", std::process::id()));
            assert!(!root.exists());
            let (mut admin, _) = counting_admin(&root);
            let mut actor = actor_for(202);
            let mut ports = crate::plugin_dispatch::Plugins::default();
            settle(&mut admin, &mut actor, &mut ports, 0);
            admin
                .submit(Origin::Editor, request("install", Some(1)), &actor)
                .unwrap();
            settle(&mut admin, &mut actor, &mut ports, 0);
            admin
                .submit(Origin::Editor, replacing(op, next, None), &actor)
                .unwrap_or_else(|r| panic!("{op} {next}: {r:?}"));
            settle(&mut admin, &mut actor, &mut ports, 0);
            assert_eq!(admin.error, None, "{op} {next}");
            assert_eq!(registered_source(&admin, &ports), registered, "{op} {next}");
            shut(admin, &mut actor, ports, root);
        }
    }
    #[test]
    fn a_plugin_without_registered_origin_takes_the_new_one_without_confirmation() {
        use std::os::unix::fs::DirBuilderExt;
        let root = std::env::temp_dir().join(format!("rad-b4-unregistered-{}", std::process::id()));
        assert!(!root.exists());
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        {
            // A plugin installed before origins were recorded.
            let mut manager = rad_plugin_manager::Manager::create(
                &root,
                rad_plugin_contract::HOST_API,
                rad_plugin_host::TARGET,
            )
            .unwrap();
            let mut change = Some(
                manager
                    .prepare(rad_plugin_manager::Operation::Install, Some(&package(1)))
                    .unwrap(),
            );
            manager.commit(&mut change).unwrap();
        }
        let (mut admin, _) = counting_admin(&root);
        let mut actor = actor_for(203);
        let mut ports = crate::plugin_dispatch::Plugins::default();
        settle(&mut admin, &mut actor, &mut ports, 0);
        assert_eq!(registered_source(&admin, &ports), Value::Null);
        admin
            .submit(Origin::Editor, replacing("update", OTHER, None), &actor)
            .unwrap();
        settle(&mut admin, &mut actor, &mut ports, 0);
        assert_eq!(admin.error, None);
        assert_eq!(registered_source(&admin, &ports), OTHER);
        shut(admin, &mut actor, ports, root);
    }
    /// The status the editor polls must decode with the wire vocabulary: a code outside
    /// `ErrorCode` made the server fail the whole response and end the runtime.
    fn assert_status_decodes(status: Value, code: &str) {
        let data = rad_api::ResponseData::decode(
            rad_api::Operation::PluginStatus,
            rad_api::Status::Ok,
            status,
        )
        .unwrap_or_else(|_| panic!("plugin_status with error {code} must decode"));
        let rad_api::ResponseData::PluginStatus(view) = data else {
            panic!("plugin_status data");
        };
        assert_eq!(view.error.map(|e| e.to_string()).as_deref(), Some(code));
    }
}

#[cfg(all(test, feature = "fixture-tests"))]
mod outcome_tests;

#[cfg(all(test, feature = "fixture-tests"))]
mod demand_tests;

#[cfg(test)]
mod repair_tests {
    use super::*;
    use rad_core::{configuration::Configuration, identity::IdentityRegistry};
    fn setup() -> (Admin, Authority, Host, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "rad-explicit-repair-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut admin = Admin::new(
            root.clone(),
            Arc::new(|| {}),
            Box::new(|_, _| panic!("repair must not download")),
        );
        let mut actor = Authority::new(
            Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap(),
            1,
            std::num::NonZeroU128::new(81).unwrap(),
        );
        let mut host = Host::default();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while admin.busy {
            admin.poll(&mut actor, &mut host, 0);
            assert!(std::time::Instant::now() < end);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        (admin, actor, host, root)
    }
    fn finish(mut admin: Admin, root: PathBuf) {
        admin.shutdown();
        admin.send(Job::Stop);
        if let Some(worker) = admin.worker.take() {
            worker.join().unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn incompatible_inventory_is_visible_as_needing_update_without_starting() {
        let (mut admin, _, host, root) = setup();
        let mut manifest: Manifest = serde_json::from_str(include_str!(
            "../../rad-plugin-host/fixture/manifest-v1.json"
        ))
        .unwrap();
        manifest.host_min = 2;
        manifest.host_max = 3;
        admin.installed.push(InstalledPlugin {
            manifest,
            previous: None,
            source: Some("https://github.com/owner/repo".into()),
        });
        let status = admin.status(&host);
        finish(admin, root);
        assert_eq!(status["items"][0]["diagnostic"], "incompatible_plugin");
        assert_eq!(status["items"][0]["host"], "absent");
    }
    #[test]
    fn retry_requires_failed_selection_and_reap_and_only_clears_its_attempt() {
        let (mut admin, mut actor, mut host, root) = setup();
        let manifest: Manifest = serde_json::from_str(include_str!(
            "../../rad-plugin-host/fixture/manifest-v1.json"
        ))
        .unwrap();
        let id = manifest.id.clone();
        actor
            .register_plugin(manifest.clone(), rad_plugin_host::TARGET)
            .unwrap();
        let selection = actor.plugin_manifests().next().unwrap().1;
        admin.attempted.insert((id.clone(), selection));
        admin.attempted.insert(("other".into(), 99));
        let launch = host.reserve_launch(manifest, 0).unwrap();
        actor.fail_plugin_selection(&id, selection).unwrap();
        assert!(!admin.can_retry(&id, &actor, &host));
        let mut prepared =
            Some(launch.spawn(std::process::Command::new(root.join("nonexistent-fixture"))));
        host.attach(&mut prepared, 0).unwrap();
        assert_eq!(host.state(&id), Some(State::Reaped));
        assert_eq!(
            admin.retry(Origin::Automation, &id, &actor, &mut host),
            Err("permission_denied")
        );
        assert!(admin.can_retry(&id, &actor, &host));
        admin.retry(Origin::Editor, &id, &actor, &mut host).unwrap();
        assert_eq!(host.state(&id), None);
        assert!(!admin.attempted.contains(&(id.clone(), selection)));
        assert!(admin.attempted.contains(&("other".into(), 99)));
        assert_eq!(actor.plugin_manifests().next().unwrap().1, selection);
        assert!(!admin.can_retry(&id, &actor, &host));
        finish(admin, root);
    }
}
