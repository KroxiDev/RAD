//! The sole runtime owner. Clients never own publication, history or execution.
mod credentials;
mod options;
mod transfers;
use crate::{bootstrap, persistence::Persistence};
use rad_authority::{Authority, Effect, RequestId};
use rad_core::configuration::Origin;
use rad_launcher::monotonic_ms;
use rad_protocol::{Capabilities, External, IncomingContext, Protocol, Resolution};
use rad_transport::{RequestKey, Transport};
use rad_wire::response::{Error, Outcome, Response, Status};
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
    failed_images: BTreeSet<String>,
    display_dirty: bool,
    shutting_down: bool,
    reconciliation_attempted: bool,
    deck: Option<Box<dyn crate::display::DeckPort>>,
    deck_connected: bool,
    images: BTreeMap<String, Arc<rad_media::NormalizedImage>>,
    asleep: bool,
    brightness: u8,
    _wake: Arc<mio::Waker>,
    pub recovered_previous: bool,
}
impl Server {
    pub fn new(runtime: &Path, data: &Path, session: NonZeroU128) -> Result<Self, RuntimeError> {
        // Instance lock precedes opening Store, so a second process cannot reaffirm it.
        let transport =
            Transport::bind(runtime, session.get()).map_err(|_| RuntimeError::Transport)?;
        let boot = bootstrap::open(
            data,
            session,
            monotonic_ms().map_err(|_| RuntimeError::Clock)?,
        )
        .map_err(|_| RuntimeError::Startup)?;
        let wake = Arc::new(
            transport
                .worker_waker()
                .map_err(|_| RuntimeError::Transport)?,
        );
        let transfer_root = data.join("transfers");
        let transfers = match std::fs::symlink_metadata(&transfer_root) {
            Ok(_) => crate::transfers::Transfers::recover(&transfer_root, session.get()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                crate::transfers::Transfers::create(&transfer_root, session.get())
            }
            Err(_) => return Err(RuntimeError::Storage),
        }
        .map_err(|_| RuntimeError::Storage)?;
        let persistence = Persistence::with_transfers(boot.store, transfers, wake.clone())
            .map_err(|_| RuntimeError::Storage)?;
        let brightness = boot
            .authority
            .editor_view(Origin::Editor)
            .expect("Editor view")
            .brightness;
        Ok(Self {
            transport,
            actor: boot.authority,
            persistence,
            credentials: credentials::Credentials::new(data.to_path_buf(), wake.clone())?,
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
            failed_images: BTreeSet::new(),
            display_dirty: true,
            shutting_down: false,
            deck: None,
            deck_connected: false,
            images: BTreeMap::new(),
            asleep: false,
            brightness,
            _wake: wake,
            recovered_previous: boot.recovered_previous,
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
    fn respond(&mut self, key: RequestKey, response: Response) -> Result<(), RuntimeError> {
        let bytes = serde_json::to_vec(&response).map_err(|_| RuntimeError::Protocol)?;
        self.transport
            .finish(key, &bytes)
            .map_err(|_| RuntimeError::Transport)
    }
    fn error(&self, context: IncomingContext, code: &str, mutation: bool) -> Response {
        Response {
            v: 1,
            id: Some(context.request_id.to_string()),
            session_id: Some(format!("{:032x}", self.session)),
            status: Status::Error,
            data: Value::Null,
            error: Some(Error {
                code: code.into(),
                message: "Request rejected".into(),
                details: json!({}),
                outcome: mutation.then_some(Outcome::NotApplied),
            }),
        }
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
                        self.failed_images.clear();
                    }
                    if let Some(mut target) = self.requests.remove(request) {
                        if !target.sent {
                            let response = self
                                .protocol
                                .response_for(&self.actor, target.context, *request, &effect)
                                .ok_or(RuntimeError::Protocol)?;
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
                Effect::DisplayClockOverflow => return Err(RuntimeError::Clock),
                Effect::InputRejected { .. }
                | Effect::CollectionFinished { .. }
                | Effect::ExportExpired { .. } => {}
                Effect::Persist { .. }
                | Effect::ReadRevision { .. }
                | Effect::Reconcile { .. }
                | Effect::Collect { .. } => return Err(RuntimeError::Storage),
            }
        }
        Ok(())
    }
    pub fn attach_deck_port(
        &mut self,
        deck: Box<dyn crate::display::DeckPort>,
    ) -> Result<(), RuntimeError> {
        if self.deck.is_some() {
            return Err(RuntimeError::Protocol);
        }
        self.deck = Some(deck);
        self.display_dirty = true;
        Ok(())
    }
    /// Trusted bootstrap only, after the user's exclusive StreamController handoff.
    pub fn attach_hardware(&mut self, config: rad_deck::DeviceConfig) -> Result<(), RuntimeError> {
        if self.deck.is_some() {
            return Err(RuntimeError::Protocol);
        }
        let desired = crate::display::project_with_plugins(
            &self.actor,
            &self.images,
            self.brightness,
            self.asleep,
            self.plugin_visuals.as_ref(),
        );
        let deck = rad_deck::Deck::spawn(config, desired, self._wake.clone())
            .map_err(|_| RuntimeError::Protocol)?;
        self.attach_deck_port(Box::new(deck))
    }
    fn pump_deck(&mut self) -> Result<(), RuntimeError> {
        // Bound matches the adapter event budget; reports are never coalesced.
        for _ in 0..64 {
            let Some(event) = self.deck.as_mut().and_then(|deck| deck.next_event()) else {
                break;
            };
            let now = monotonic_ms().map_err(|_| RuntimeError::Clock)?;
            let effects = match event {
                rad_deck::Event::Connected => {
                    self.deck_connected = true;
                    self.display_dirty = true;
                    self.actor.input_disconnect(now)
                }
                rad_deck::Event::Report(keys) => self.actor.input_report(now, keys),
                rad_deck::Event::Disconnected(_) | rad_deck::Event::Stopped { .. } => {
                    self.deck_connected = false;
                    self.actor.input_disconnect(now)
                }
            }
            .map_err(|_| RuntimeError::Clock)?;
            self.effects(effects)?;
        }
        Ok(())
    }
    fn paint(&mut self) {
        if !self.display_dirty {
            return;
        }
        self.display_dirty = false;
        if let Some(deck) = &mut self.deck {
            let desired = crate::display::project_with_plugins(
                &self.actor,
                &self.images,
                self.brightness,
                self.asleep,
                self.plugin_visuals.as_ref(),
            );
            if deck.set_desired(desired).is_err() {
                self.deck_connected = false;
            }
        }
    }
    pub fn attach_launcher(
        &mut self,
        launcher: rad_launcher::Launcher,
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
            Box::new(|source, cancel| {
                let source = rad_plugin_manager::GithubSource::parse(source)?;
                rad_plugin_manager::Downloader::new().download(
                    &source,
                    rad_plugin_host::TARGET,
                    cancel.clone(),
                )
            }),
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
    fn sync_credentials(&mut self) -> Result<(), RuntimeError> {
        if let Some(native) = &mut self.native {
            native
                .sync_credentials(
                    &self.actor,
                    &self
                        .waiting_options
                        .iter()
                        .filter_map(options::WaitingOptions::binding)
                        .collect::<Vec<_>>(),
                    &self.transport,
                )
                .map_err(|_| RuntimeError::Protocol)?;
            if !self.shutting_down
                && let Some(visuals) = &mut self.plugin_visuals
            {
                self.display_dirty |= visuals.poll(&self.actor, native.plugin_host_mut());
            }
        }
        Ok(())
    }
    fn pump_native(&mut self) -> Result<(), RuntimeError> {
        if let Some(native) = &mut self.native {
            let effects = native
                .pump(&mut self.actor, &self.transport)
                .map_err(|_| RuntimeError::Protocol)?;
            self.effects(effects)?;
        }
        Ok(())
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
                | External::PluginVisual { .. }
                | External::PluginVisualRead { .. }
        ) {
            let mutation = matches!(operation, External::PluginAdmin(_));
            let result = self.plugin_external(context, operation);
            let response = match result {
                Ok(data) => Response {
                    v: 1,
                    id: Some(context.request_id.to_string()),
                    session_id: Some(format!("{:032x}", self.session)),
                    status: Status::Ok,
                    data,
                    error: None,
                },
                Err(code) => self.error(context, code, mutation),
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
            Ok(data) => Response {
                v: 1,
                id: Some(context.request_id.to_string()),
                session_id: Some(format!("{:032x}", self.session)),
                status: Status::Ok,
                data,
                error: None,
            },
            Err(code) => self.error(context, code, false),
        };
        self.respond(key, response)
    }
    fn plugin_external(
        &mut self,
        context: IncomingContext,
        operation: External,
    ) -> Result<Value, &'static str> {
        match operation {
            External::PluginAdmin(request) => {
                if context.origin != Origin::Editor { return Err("permission_denied"); }
                let residual = request.plugin_id.as_ref().is_some_and(|id| {
                    self.native.as_ref().is_some_and(|native| {
                        native.plugin_host().state(id) == Some(rad_plugin_host::State::Failed)
                    })
                });
                if residual { return Err("plugin_residual"); }
                self.plugin_admin.as_mut().ok_or("executor_unavailable")?
                    .submit(context.origin, request, &self.actor)
            }
            External::PluginStatus => {
                if context.origin != Origin::Editor { return Err("permission_denied"); }
                let (admin, native) = self.plugin_admin.as_ref().zip(self.native.as_ref())
                    .ok_or("executor_unavailable")?;
                let mut status=admin.status(native.plugin_host());
                if let Some(items)=status["items"].as_array_mut() {
                    for item in items {
                        let id=item["id"].as_str().unwrap_or_default();
                        let needs=self.actor.plugin_credential_status(id).iter().any(|(_,present)|!present);
                        item["needs_credentials"]=json!(needs);
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
    pub fn turn(&mut self) -> Result<(), RuntimeError> {
        self.clock()?;
        self.poll_credentials()?;
        self.sync_credentials()?;
        self.pump_native()?;
        if let Some(native) = &mut self.native {
            if let Some(admin) = &mut self.plugin_admin {
                admin.set_extra_demand(
                    self.waiting_options
                        .iter()
                        .map(options::WaitingOptions::plugin)
                        .collect(),
                );
                admin.poll(
                    &mut self.actor,
                    native.plugin_host_mut(),
                    monotonic_ms().map_err(|_| RuntimeError::Clock)?,
                );
            }
            if !self.shutting_down
                && let Some(visuals) = &mut self.plugin_visuals
            {
                self.display_dirty |= visuals.poll(&self.actor, native.plugin_host_mut());
            }
        }
        self.poll_credentials()?;
        self.sync_credentials()?;
        if !self.shutting_down {
            self.pump_deck()?;
        }
        self.transport
            .drive(monotonic_ms().map_err(|_| RuntimeError::Clock)?)
            .map_err(|_| RuntimeError::Transport)?;
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
                    {
                        response.data["deck_state"] = json!(if self.deck.is_none() {
                            "disabled"
                        } else if self.deck_connected {
                            "connected"
                        } else {
                            "disconnected"
                        });
                        response.data["recovered_previous"] = json!(self.recovered_previous);
                    }
                    self.effects(effects)?;
                    self.respond(incoming.key, response)?;
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
        if let (Some(native), Some(admin)) = (&mut self.native, &mut self.plugin_admin) {
            admin.set_extra_demand(
                self.waiting_options
                    .iter()
                    .map(options::WaitingOptions::plugin)
                    .collect(),
            );
            admin.poll(
                &mut self.actor,
                native.plugin_host_mut(),
                monotonic_ms().map_err(|_| RuntimeError::Clock)?,
            );
        }
        self.pump_native()?;
        // This pump can consume Ready after the earlier admin poll. Publish its
        // binding before credentials/options settle, rather than sleeping with
        // an unbound Ready host and no remaining handshake readiness/deadline.
        if let (Some(native), Some(admin)) = (&mut self.native, &mut self.plugin_admin) {
            admin.poll(
                &mut self.actor,
                native.plugin_host_mut(),
                monotonic_ms().map_err(|_| RuntimeError::Clock)?,
            );
        }
        // Last on purpose: the transport stamps a response with the time of its latest
        // `drive`, so answering before it after a long sleep would look expired and be dropped.
        self.pump_options()?;
        self.poll_credentials()?;
        self.sync_credentials()?;
        self.respond_credentials()?;
        let effects = self.reconcile_once();
        self.effects(effects)?;
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
        self.credentials.shutdown();
        if let Some(admin) = &mut self.plugin_admin {
            admin.shutdown();
        }
        if let Some(deck) = &mut self.deck {
            deck.shutdown();
        }
        if let Some(native) = &mut self.native {
            let _ = native.launcher.shutdown();
            native.shutdown_plugins();
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
