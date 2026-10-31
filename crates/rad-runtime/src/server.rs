//! The sole runtime owner. Clients never own publication, history or execution.
mod client_api;
#[cfg(test)]
mod contained_tests;
mod credentials;
mod deck;

#[cfg(all(test, feature = "fixture-tests"))]
mod fault_tests;
// Launcher fixtures share one process-wide owner; keep the seam available to all
// unit tests even when optional fixture modules are not compiled.
#[cfg(test)]
#[cfg_attr(not(feature = "fixture-tests"), allow(dead_code))]
pub(crate) static FIXTURE_RUNTIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
mod machine_tests;
mod options;
mod responses;
mod transfers;
use crate::{bootstrap, persistence::Persistence};
use rad_api::{
    EventCode,
    response::{Error, Outcome, Response, Status},
};
use rad_authority::{Authority, Effect, RequestId};
use rad_core::configuration::Origin;
use rad_launcher::monotonic_ms;
use rad_protocol::{Capabilities, External, IncomingContext, Protocol, Resolution};
use rad_transport::{RequestKey, Transport, TransportError};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    num::NonZeroU128,
    path::Path,
    sync::Arc,
    time::Duration,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeError {
    /// The instance lock or the sockets cannot be had: nothing can be served.
    Blocked,
    Startup,
    Transport,
    Clock,
    Storage,
    Protocol,
}
struct ReplyTarget {
    key: RequestKey,
    context: IncomingContext,
    sent: bool,
}

/// How to open the deck, registered by whoever owns the hardware; the server decides when
/// (ADR 0006: only while the machine setting is on).
type DeckFactory = Box<
    dyn FnMut(
        rad_deck::Desired,
        Arc<mio::Waker>,
    ) -> Result<Box<dyn crate::display::DeckPort>, rad_deck::DeckError>,
>;

pub struct Server {
    transport: Transport,
    actor: Authority,
    persistence: Persistence,
    credentials: credentials::Credentials,
    native: Option<crate::native::Native>,
    plugin_admin: Option<crate::plugin_admin::Admin>,
    plugin_root: std::path::PathBuf,
    plugin_visuals: Option<crate::plugin_visuals::Visuals>,
    protocol: Protocol,
    connections: BTreeSet<u64>,
    requests: BTreeMap<RequestId, ReplyTarget>,
    /// Questions to plugins, by host correlation (generation, request). At most the host's
    /// own cap per plugin: it refuses more before anything is stored here.
    options: BTreeMap<(u64, u64), options::PendingOptions>,
    waiting_options: Vec<options::WaitingOptions>,
    session: NonZeroU128,
    transfer: Option<transfers::Flight>,
    exports: BTreeMap<String, (u64, rad_authority::ExportLease)>,
    prepared: BTreeMap<(u64, String), String>,
    display_dirty: bool,
    shutting_down: bool,
    reconciliation_attempted: bool,
    deck: Option<Box<dyn crate::display::DeckPort>>,
    deck_connected: bool,
    deck_inaccessible: bool,
    deck_factory: Option<DeckFactory>,
    /// The setting turned the deck off and it is being stopped; it is let go once it ends.
    deck_stopping: bool,
    /// The setting is on and no attempt has been made since it was turned on.
    deck_open_pending: bool,
    machine: crate::machine_settings::MachineSettings,
    data_dir: std::path::PathBuf,
    notices: client_api::Notices,
    pictures: crate::pictures::Pictures,
    /// Pictures of the folders the deck may show next, for the folder and revision shown.
    nearby_pictures: Option<(transfers::Shown, BTreeSet<String>)>,
    asleep: bool,
    brightness: u8,
    _wake: Arc<mio::Waker>,
    pub recovered_previous: bool,
    previous_exit: Option<rad_api::PreviousExit>,
    events: crate::events::Events,
    connection_failures: crate::events::ConnectionFailures,
    /// Last storage state recorded, so only a change leaves an event.
    storage_seen: rad_authority::StorageState,
}
impl Server {
    /// Binds and opens in one step; recovery mode is an error here (see [`crate::startup`]).
    pub fn new(runtime: &Path, data: &Path, session: NonZeroU128) -> Result<Self, RuntimeError> {
        match crate::startup::Bound::bind(runtime, session)?.open(data)? {
            crate::startup::Opened::Serving(server) => Ok(*server),
            crate::startup::Opened::Recovery(_) => Err(RuntimeError::Startup),
        }
    }
    /// The server over an open Store. `events` already holds what happened before.
    pub(crate) fn from_parts(
        transport: Transport,
        boot: bootstrap::Bootstrap,
        data: &Path,
        session: NonZeroU128,
        previous_exit: Option<rad_api::PreviousExit>,
        mut events: crate::events::Events,
    ) -> Result<Self, RuntimeError> {
        let wake = Arc::new(
            transport
                .worker_waker()
                .map_err(|_| RuntimeError::Transport)?,
        );
        let transfer_root = data.join("transfers");
        let transfers = crate::transfers::Transfers::open(&transfer_root, session.get())
            .map_err(|_| RuntimeError::Storage)?;
        let persistence = Persistence::with_transfers(boot.store, transfers, wake.clone())
            .map_err(|_| RuntimeError::Storage)?;
        let brightness = boot
            .authority
            .editor_view(Origin::Editor)
            .expect("Editor view")
            .brightness;
        let credentials = credentials::Credentials::new(data.to_path_buf(), wake.clone())?;
        if let Some(previous) = &previous_exit {
            events.record(EventCode::PreviousExit, Some(&previous.code));
        }
        // An unreadable file means defaults, and is left as it is for the user to see.
        let machine = crate::machine_settings::MachineSettings::load(data).unwrap_or_else(|_| {
            events.record(EventCode::MachineSettingsUnreadable, None);
            Default::default()
        });
        Ok(Self {
            transport,
            actor: boot.authority,
            persistence,
            credentials,
            native: None,
            plugin_admin: None,
            plugin_root: data.join("plugins"),
            plugin_visuals: None,
            protocol: Protocol::with_capabilities(
                session,
                Capabilities {
                    transfers: true,
                    media: true,
                    launcher: false,
                },
            ),
            connections: BTreeSet::new(),
            requests: BTreeMap::new(),
            options: BTreeMap::new(),
            waiting_options: Vec::new(),
            session,
            transfer: None,
            exports: BTreeMap::new(),
            prepared: BTreeMap::new(),
            display_dirty: true,
            shutting_down: false,
            deck: None,
            deck_connected: false,
            deck_inaccessible: false,
            deck_factory: None,
            deck_stopping: false,
            deck_open_pending: false,
            machine,
            data_dir: data.to_path_buf(),
            notices: client_api::Notices::default(),
            pictures: crate::pictures::Pictures::new(crate::pictures::SPARE_BYTES),
            nearby_pictures: None,
            asleep: false,
            brightness,
            _wake: wake,
            recovered_previous: boot.recovered_previous,
            previous_exit,
            events,
            connection_failures: Default::default(),
            storage_seen: rad_authority::StorageState::Ready,
            reconciliation_attempted: false,
        })
    }
    fn clock(&mut self) -> Result<(), RuntimeError> {
        let effects = self
            .actor
            .before_event(monotonic_ms().map_err(|_| RuntimeError::Clock)?)
            .map_err(|_| RuntimeError::Clock)?;
        self.effects(effects)
    }
    fn effects(&mut self, effects: Vec<Effect>) -> Result<(), RuntimeError> {
        let effects = self.persistence.route(&mut self.actor, effects);
        let effects = if let Some(native) = &mut self.native
            && !self.shutting_down
        {
            native
                .route(&mut self.actor, effects)
                .map_err(|_| RuntimeError::Protocol)?
        } else {
            effects
        };
        let mut queue: VecDeque<_> = effects.into();
        while let Some(effect) = queue.pop_front() {
            match &effect {
                Effect::Reply { request, .. }
                | Effect::Rejected { request, .. }
                | Effect::Accepted { request, .. }
                | Effect::Unknown { request } => {
                    if matches!(effect, Effect::Reply { .. } | Effect::Rejected { .. }) {
                        // Includes valid no-ops: no Persist means the prepared media is unused.
                        self.persistence.release_media(*request);
                        self.pictures.retry_failed();
                    }
                    if let Some(mut target) = self.requests.remove(request) {
                        if !target.sent {
                            let found = self.protocol.response_for(
                                &self.actor,
                                target.context,
                                *request,
                                &effect,
                            );
                            let response = self.response_or_generic(found, target.context, &effect);
                            self.respond(target.key, response)?;
                            target.sent = true;
                        }
                        // Unknown retains Authority's started commit credit until reconciliation.
                        if !self.actor.finish(*request) {
                            self.requests.insert(*request, target);
                        }
                    } else {
                        self.actor.finish(*request);
                    }
                    self.display_dirty = true;
                }
                Effect::Display { brightness, asleep } => {
                    self.brightness = *brightness;
                    self.asleep = *asleep;
                    self.display_dirty = true;
                }
                Effect::SelectionChanged { .. }
                | Effect::Wake
                | Effect::HardwareAccepted { .. } => self.display_dirty = true,
                Effect::ReleaseHold { execution } => self.actor.hold_released(*execution),
                Effect::Dispatch(intent) => {
                    // No native port attached in this cut: fail explicitly, never pretend to launch.
                    let effects = self
                        .actor
                        .job_complete(
                            intent.id,
                            rad_core::execution::Completion::Failed(
                                rad_core::execution::Failure::ServiceUnavailable,
                            ),
                        )
                        .map_err(|_| RuntimeError::Protocol)?;
                    queue.extend(self.persistence.route(&mut self.actor, effects));
                }
                Effect::DisplayClockOverflow => {
                    // The deck just does not go to sleep by itself; input still wakes it.
                    self.events.record(EventCode::DisplayClockOverflow, None);
                }
                Effect::InputRejected { .. }
                | Effect::CollectionFinished { .. }
                | Effect::ExportExpired { .. } => {}
                Effect::Persist { .. }
                | Effect::ReadRevision { .. }
                | Effect::Reconcile { .. }
                | Effect::Collect { .. } => return Err(RuntimeError::Storage),
            }
        }
        self.note_storage();
        Ok(())
    }
    /// Records a change of the storage state, whatever produced it.
    fn note_storage(&mut self) {
        use rad_authority::StorageState;
        let now = self.actor.storage_state();
        if now == self.storage_seen {
            return;
        }
        self.storage_seen = now;
        self.events.record(
            match now {
                StorageState::Ready => EventCode::StorageReady,
                StorageState::Uncertain => EventCode::StorageUncertain,
                StorageState::RecoveryRequired => EventCode::StorageRecoveryRequired,
            },
            None,
        );
    }
    pub fn attach_launcher(
        &mut self,
        launcher: rad_launcher::Launcher,
    ) -> Result<(), RuntimeError> {
        self.attach_launcher_with_download(launcher, |source, cancel, progress| {
            let source = rad_plugin_manager::GithubSource::parse(source)?;
            rad_plugin_manager::Downloader::new().download(
                &source,
                rad_plugin_host::TARGET,
                cancel.clone(),
                progress.clone(),
            )
        })
    }
    fn attach_launcher_with_download(
        &mut self,
        launcher: rad_launcher::Launcher,
        download: impl Fn(
            &str,
            &rad_plugin_manager::Cancellation,
            &rad_plugin_manager::Progress,
        ) -> Result<Vec<u8>, rad_plugin_manager::Error>
        + Send
        + 'static,
    ) -> Result<(), RuntimeError> {
        if self.native.is_some() {
            return Err(RuntimeError::Protocol);
        }
        self.native = Some(
            crate::native::Native::attach(launcher, &self.transport)
                .map_err(|_| RuntimeError::Protocol)?,
        );
        let wake = self._wake.clone();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = wake.wake();
        });
        self.plugin_admin = Some(crate::plugin_admin::Admin::new(
            self.plugin_root.clone(),
            notify.clone(),
            Box::new(download),
        ));
        self.plugin_visuals = Some(crate::plugin_visuals::Visuals::new(notify));
        self.protocol = Protocol::with_capabilities(
            self.session,
            Capabilities {
                launcher: true,
                transfers: true,
                media: true,
            },
        );
        Ok(())
    }
    fn sync_credentials(&mut self) {
        if let Some(native) = &mut self.native {
            native.sync_credentials(
                &self.actor,
                &self
                    .waiting_options
                    .iter()
                    .filter_map(options::WaitingOptions::binding)
                    .collect::<Vec<_>>(),
                &self.transport,
            );
        }
        self.record_native_events();
    }
    /// Records what the launcher and the plugin host reported since the last call.
    fn record_native_events(&mut self) {
        if let Some(native) = &mut self.native {
            self.events.record_all(native.take_events());
        }
    }
    /// Pumps plugins and the launcher; `true` when that changed something (see `Native::pump`).
    fn pump_native(&mut self) -> Result<bool, RuntimeError> {
        let Some(native) = &mut self.native else {
            return Ok(false);
        };
        let pumped = native.pump(&mut self.actor, &self.transport);
        self.record_native_events();
        let (effects, advanced) = pumped.map_err(|_| RuntimeError::Protocol)?;
        let produced = !effects.is_empty();
        self.effects(effects)?;
        Ok(advanced || produced)
    }
    /// Lets the plugin administrator advance; `true` when it changed anything.
    fn poll_admin(&mut self) -> Result<bool, RuntimeError> {
        let (Some(native), Some(admin)) = (&mut self.native, &mut self.plugin_admin) else {
            return Ok(false);
        };
        admin.set_extra_demand(
            self.waiting_options
                .iter()
                .map(options::WaitingOptions::plugin)
                .collect(),
        );
        Ok(admin.poll(
            &mut self.actor,
            native.plugin_host_mut(),
            monotonic_ms().map_err(|_| RuntimeError::Clock)?,
        ))
    }
    /// Refreshes plugin visuals; `true` when one changed, which also dirties the display.
    fn poll_visuals(&mut self) -> bool {
        if self.shutting_down {
            return false;
        }
        let (Some(native), Some(visuals)) = (&mut self.native, &mut self.plugin_visuals) else {
            return false;
        };
        let changed = visuals.poll(&self.actor, native.plugin_host_mut());
        self.display_dirty |= changed;
        changed
    }
    fn external(
        &mut self,
        key: RequestKey,
        context: IncomingContext,
        operation: External,
    ) -> Result<(), RuntimeError> {
        let operation = match operation {
            External::Credential(operation) => {
                return self.credential_request(key, context, operation);
            }
            External::PluginOptions(query) => return self.plugin_options(key, context, *query),
            other => other,
        };
        if matches!(
            operation,
            External::PluginAdmin(_)
                | External::PluginStatus
                | External::PluginRetry { .. }
                | External::PluginLog { .. }
                | External::PluginVisual { .. }
                | External::PluginVisualRead { .. }
        ) {
            let response_operation = match &operation {
                External::PluginAdmin(_) => rad_api::Operation::PluginAdmin,
                External::PluginStatus => rad_api::Operation::PluginStatus,
                External::PluginRetry { .. } => rad_api::Operation::PluginRetry,
                External::PluginLog { .. } => rad_api::Operation::PluginLog,
                External::PluginVisual { .. } => rad_api::Operation::PluginVisual,
                External::PluginVisualRead { .. } => rad_api::Operation::PluginVisualRead,
                _ => unreachable!("plugin external operation"),
            };
            let mutation = matches!(
                operation,
                External::PluginAdmin(_) | External::PluginRetry { .. }
            );
            let result = match operation {
                External::PluginAdmin(request) => self.plugin_admin(context, request),
                operation => self
                    .plugin_external(context, operation)
                    .map_err(crate::plugin_admin::Rejection::from),
            };
            let response = match result {
                Ok(data) => self.plugin_response(context, response_operation, mutation, data),
                Err(rejection) => {
                    self.error_with_details(context, rejection.code, rejection.details, mutation)
                }
            };
            return self.respond(key, response);
        }
        if !matches!(
            operation,
            External::LauncherCatalog { .. } | External::LauncherOptions { .. }
        ) {
            return self.transfer_external(key, context, operation);
        }
        let result = match operation {
            External::LauncherCatalog { cursor, query } => {
                self.catalog(context, &query, "catalog", cursor.as_deref())
            }
            External::LauncherOptions { id, cursor } => {
                self.catalog(context, "", &format!("options:{id:?}"), cursor.as_deref())
            }
            _ => Err("executor_unavailable"),
        };
        let response = match result {
            Ok(data) => self.launcher_response(context, data),
            Err(code) => self.error(context, code, false),
        };
        self.respond(key, response)
    }
    fn plugin_admin(
        &mut self,
        context: IncomingContext,
        request: rad_protocol::PluginAdmin,
    ) -> Result<Value, crate::plugin_admin::Rejection> {
        if context.origin != Origin::Editor {
            return Err("permission_denied".into());
        }
        let residual = request.plugin_id.as_ref().is_some_and(|id| {
            self.native.as_ref().is_some_and(|native| {
                native.plugin_host().state(id) == Some(rad_plugin_host::State::Failed)
            })
        });
        if residual {
            return Err("plugin_residual".into());
        }
        self.plugin_admin
            .as_mut()
            .ok_or("executor_unavailable")?
            .submit(context.origin, request, &self.actor)
    }
    fn plugin_external(
        &mut self,
        context: IncomingContext,
        operation: External,
    ) -> Result<Value, &'static str> {
        match operation {
            External::PluginLog { plugin_id } => {
                if context.origin != Origin::Editor { return Err("permission_denied"); }
                let host = self.native.as_ref().ok_or("executor_unavailable")?.plugin_host();
                if !self.actor.plugin_manifests().any(|(manifest, _, _)| manifest.id == plugin_id) { return Err("not_found"); }
                serde_json::to_value(bounded_plugin_log(plugin_id.clone(), host.stderr(&plugin_id))?).map_err(|_| "invalid_configuration")
            }
            External::PluginRetry { plugin_id } => {
                let (admin, native) = self.plugin_admin.as_mut().zip(self.native.as_mut()).ok_or("executor_unavailable")?;
                admin.retry(context.origin, &plugin_id, &self.actor, native.plugin_host_mut())?;
                Ok(json!({}))
            }
            External::PluginStatus => {
                if context.origin != Origin::Editor { return Err("permission_denied"); }
                let (admin, native) = self.plugin_admin.as_ref().zip(self.native.as_ref())
                    .ok_or("executor_unavailable")?;
                let mut status=admin.status(native.plugin_host());
                if let Some(items)=status["items"].as_array_mut() {
                    for item in items {
                        let id=item["id"].as_str().unwrap_or_default();
                        let can_retry = admin.can_retry(id, &self.actor, native.plugin_host());
                        let has_settings = self.actor.plugin_manifests().any(|(manifest, _, _)| manifest.id == id && manifest.settings.is_some());
                        let needs=self.actor.plugin_credential_status(id).iter().any(|(_,present)|!present);
                        item["needs_credentials"]=json!(needs);
                        item["can_retry"]=json!(can_retry);
                        item["has_settings"]=json!(has_settings);
                    }
                }
                Ok(status)
            }
            External::PluginVisual { id } => Ok(self.plugin_visuals.as_ref()
                .map(|visuals| visuals.ipc_view(&self.actor, id))
                .unwrap_or(json!({"availability":"unavailable","revision":"0","text":null,"text_position":"bottom","color":null,"image":false}))),
            External::PluginVisualRead { id, revision, offset } => self.plugin_visuals.as_ref()
                .ok_or("executor_unavailable")?.read(id, revision, offset),
            _ => Err("executor_unavailable"),
        }
    }
    fn catalog(
        &self,
        context: IncomingContext,
        query: &str,
        scope: &str,
        cursor: Option<&str>,
    ) -> Result<Value, &'static str> {
        let entries = self
            .native
            .as_ref()
            .and_then(|n| n.launcher.catalog())
            .ok_or("service_unavailable")?;
        crate::catalog::page(entries, context.connection, query, scope, cursor)
    }
    /// Most rounds `settle` runs before yielding to the rest of the turn.
    const SETTLE_ROUNDS: usize = 8;
    /// Runs the steps that feed each other (credential results, plugin pump, plugin
    /// administrator, visuals) until none of them advances, so the turn does not depend on
    /// the order in which they were written. The cap bounds the time spent here; reaching it
    /// with work still advancing wakes the loop, so the next `wait` returns at once instead
    /// of sleeping on pending work.
    fn settle(&mut self) -> Result<(), RuntimeError> {
        self.settle_with(Self::settle_round).map(|_| ())
    }
    /// `settle` over an arbitrary round, returning how many rounds ran.
    fn settle_with(
        &mut self,
        mut round: impl FnMut(&mut Self) -> Result<bool, RuntimeError>,
    ) -> Result<usize, RuntimeError> {
        for ran in 1..=Self::SETTLE_ROUNDS {
            if !round(self)? {
                return Ok(ran);
            }
        }
        // A failed wake only costs latency: the next readiness or deadline runs the turn.
        let _ = self._wake.wake();
        Ok(Self::SETTLE_ROUNDS)
    }
    /// One pass over the settled steps; `true` when any of them advanced. Credentials are
    /// synchronized in every pass but never counted: they only react to what the other
    /// steps produce (a result, a published binding, a Ready host) and nothing in the turn
    /// waits on them, since the plugin acknowledges installs through its own readiness.
    fn settle_round(&mut self) -> Result<bool, RuntimeError> {
        let mut advanced = self.poll_credentials()?;
        self.sync_credentials();
        advanced |= self.pump_native()?;
        advanced |= self.poll_admin()?;
        advanced |= self.poll_visuals();
        Ok(advanced)
    }
    pub fn turn(&mut self) -> Result<(), RuntimeError> {
        self.clock()?;
        self.settle()?;
        if !self.shutting_down {
            self.pump_deck()?;
            self.settle_deck()?;
        }
        self.transport
            .drive(monotonic_ms().map_err(|_| RuntimeError::Clock)?)
            .map_err(|_| RuntimeError::Transport)?;
        let failures = self.transport.take_connection_failures();
        if let Some(count) = self
            .connection_failures
            .note(failures, monotonic_ms().map_err(|_| RuntimeError::Clock)?)
        {
            self.events
                .record(EventCode::ConnectionFailed, Some(&count.to_string()));
        }
        let live: BTreeSet<_> = self.transport.connection_ids().collect();
        let lost: Vec<_> = self.connections.difference(&live).copied().collect();
        for connection in lost {
            self.actor.disconnect(connection);
            self.abandon_options(connection)?;
            let ids: Vec<_> = self
                .requests
                .iter()
                .filter(|(_, target)| target.context.connection == connection)
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                self.persistence.release_media(id);
                let target = self.requests.remove(&id).expect("collected reply target");
                if !target.sent {
                    self.respond(
                        target.key,
                        self.error(target.context, "runtime_unavailable", true),
                    )?;
                }
            }
        }
        self.connections = live;
        self.clock()?;
        let effects = self.persistence.poll(&mut self.actor);
        self.effects(effects)?;
        self.poll_transfers()?;
        while !self.transfer.as_ref().is_some_and(|f| f.prepares()) {
            let Some(incoming) = self.transport.take_next() else {
                break;
            };
            self.clock()?;
            let context = IncomingContext {
                request_id: incoming.key.id(),
                connection: incoming.key.connection(),
                origin: match incoming.origin {
                    rad_transport::Origin::Editor => Origin::Editor,
                    rad_transport::Origin::Automation => Origin::Automation,
                },
            };
            if self.shutting_down {
                self.respond(
                    incoming.key,
                    self.error(context, "runtime_unavailable", true),
                )?;
                continue;
            }
            if self.client_api(incoming.key, context, &incoming.request)? {
                continue;
            }
            match self
                .protocol
                .handle(&mut self.actor, context, &incoming.request)
            {
                Resolution::Immediate {
                    mut response,
                    effects,
                } => {
                    if context.origin == Origin::Editor
                        && incoming.request.operation() == "status"
                        && response.error.is_none()
                        && let rad_api::ResponseData::Status(status) = &mut response.data
                    {
                        status.deck_state = Some(self.deck_state());
                        status.recovered_previous = Some(self.recovered_previous);
                        status.previous_exit = self.previous_exit.clone();
                    }
                    self.effects(effects)?;
                    self.respond(incoming.key, *response)?;
                }
                Resolution::Command(command) => {
                    let media = self.prepared_image(context, &incoming.request);
                    self.command(incoming.key, context, command, media)?;
                }
                Resolution::External(operation) => {
                    self.external(incoming.key, context, operation)?
                }
            }
        }
        if !self.shutting_down {
            let effects = self
                .actor
                .advance(monotonic_ms().map_err(|_| RuntimeError::Clock)?)
                .map_err(|_| RuntimeError::Clock)?;
            self.effects(effects)?;
        }
        self.settle()?;
        // Last on purpose: the transport stamps a response with the time of its latest
        // `drive`, so answering before it after a long sleep would look expired and be dropped.
        self.pump_options()?;
        // Answered questions leave `waiting_options`, so the credentials installed only for
        // them are released here rather than on a later wake.
        self.sync_credentials();
        self.respond_credentials()?;
        let effects = self.reconcile_once();
        self.effects(effects)?;
        self.publish_notices()?;
        if !self.shutting_down {
            self.paint();
            self.maintain_transfers()?;
        }
        Ok(())
    }
    // One reaffirmation per incident, never a replay of the original commit.
    // Persistent uncertainty remains visible and blocked, without an IO loop.
    fn reconcile_once(&mut self) -> Vec<Effect> {
        match self.actor.storage_state() {
            rad_authority::StorageState::Ready => self.reconciliation_attempted = false,
            rad_authority::StorageState::Uncertain if !self.reconciliation_attempted => {
                self.reconciliation_attempted = true;
                return self.actor.reconcile();
            }
            _ => {}
        }
        vec![]
    }
    /// Client requests accepted and not answered yet (queued or started): the transport's
    /// own bound, exposed so a test or a measurement can see that a slot came back.
    pub fn pending_requests(&self) -> usize {
        self.transport.request_count()
    }
    pub fn request_shutdown(&mut self) {
        if self.shutting_down {
            return;
        }
        self.shutting_down = true;
        self.events.record(EventCode::ShutdownRequested, None);
        self.credentials.shutdown();
        if let Some(admin) = &mut self.plugin_admin {
            admin.shutdown();
        }
        if let Some(deck) = &mut self.deck {
            deck.shutdown();
        }
        if let Some(native) = &mut self.native {
            native.stop_launcher();
            native.shutdown_plugins();
            self.record_native_events();
        }
    }
    pub fn shutdown_complete(&self) -> bool {
        self.shutting_down
            && self.credentials.stopped()
            && self
                .plugin_admin
                .as_ref()
                .is_none_or(|admin| admin.stopped())
            && self.deck.as_ref().is_none_or(|deck| deck.is_stopped())
            && self.native.as_ref().is_none_or(|native| {
                native.launcher.process_id().is_none() && native.plugins_stopped()
            })
            && self.transfer.is_none()
            && self.persistence.can_submit_transfer()
    }
    pub fn register_worker_io(
        &self,
        source: &mut impl mio::event::Source,
        interest: mio::Interest,
    ) -> Result<(), RuntimeError> {
        self.transport
            .register_worker_io(source, interest)
            .map_err(|_| RuntimeError::Transport)
    }
    pub fn wait(&mut self, maximum: Duration) -> Result<(), RuntimeError> {
        let now = monotonic_ms().map_err(|_| RuntimeError::Clock)?;
        let transfer_deadline = if self.transfer.is_none() && self.persistence.can_submit_transfer()
        {
            self.persistence
                .transfer_metadata()
                .and_then(|m| m.next_deadline_ms)
        } else {
            None
        };
        let timeout = self
            .actor
            .next_deadline()
            .into_iter()
            .chain(transfer_deadline)
            .chain(self.waiting_options.iter().map(|w| w.deadline))
            .chain(self.native.as_ref().and_then(|n| n.next_plugin_poll()))
            .min()
            .map_or(maximum, |time| {
                maximum.min(Duration::from_millis(time.saturating_sub(now)))
            });
        self.transport
            .wait_readiness(timeout)
            .map_err(|_| RuntimeError::Transport)
    }
}

/// Keep newest sanitized lines, reserving room for the response envelope.
fn bounded_plugin_log(
    plugin_id: String,
    mut lines: Vec<String>,
) -> Result<rad_api::PluginLogView, &'static str> {
    while serde_json::to_vec(&lines)
        .map_err(|_| "invalid_configuration")?
        .len()
        > 48 * 1024 - 1024
    {
        lines.remove(0);
    }
    Ok(rad_api::PluginLogView { plugin_id, lines })
}
