//! Bounded process owner. Pump never waits on a child or a pipe; no reader threads.
mod holds;
mod stderr;
pub use holds::{HoldTicket, MAX_HOLDS, MAX_PLUGIN_HOLDS};
mod worker;
use rad_plugin_contract::{Correlation, Manifest, Message, Outcome, Parameters, VisualState};
use std::{collections::BTreeMap, process::Command, sync::Arc};
pub use stderr::{MAX_LINE_BYTES as MAX_STDERR_LINE_BYTES, MAX_LINES as MAX_STDERR_LINES};
use worker::{Pending, Visual, Worker};
pub const TARGET: &str = env!("RAD_PLUGIN_TARGET");
pub use rad_plugin_contract::{ChoiceList, MAX_PLUGINS, OptionsScope};
pub const MAX_JOBS: usize = 64;
pub const MAX_PLUGIN_JOBS: usize = 16;
/// Options queries pending per plugin, expired ones included until their reply is drained.
pub const MAX_PLUGIN_OPTIONS: usize = 2;
pub const MAX_VISUALS: usize = 32;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Capacity,
    PluginLimit,
    Unavailable,
    Residual,
    Io,
    Clock,
    Exhausted,
}
/// Which dynamic field to list, given the values of the fields it depends on.
#[derive(Debug, Clone, PartialEq)]
pub struct OptionsQuery {
    pub scope: OptionsScope,
    pub definition: String,
    pub field: String,
    pub context: Parameters,
}
/// How an options query ended; consumed once through `Host::take_options`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionsOutcome {
    /// The plugin answered, or the process ended first (then `Unavailable`).
    Choices(ChoiceList),
    /// The deadline passed. The plugin stays up; its late reply is discarded.
    TimedOut,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Starting,
    Ready,
    Failed,
    Reaped,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Diagnostic {
    Protocol,
    Pipe,
    Deadline,
    Exited,
    Backpressure,
}
/// Immutable per-operation portable settings and physical control event.
#[derive(Debug, Clone, Default)]
pub struct InvokeContext {
    pub settings: Option<Parameters>,
    pub control: Option<rad_plugin_contract::ControlEvent>,
}
#[derive(Default)]
pub struct Host {
    // Tickets keep this allocation alive: address identity cannot ABA across Hosts.
    owner: Arc<()>,
    workers: BTreeMap<String, Worker>,
    next_generation: u64,
    now: u64,
}
impl Host {
    /// Convenience for standalone/off-loop users. Runtime uses reserve_launch/spawn/attach.
    ///
    /// The plugin leads its own process group and gets `PR_SET_PDEATHSIG` SIGKILL, which
    /// fires when the *thread that spawns it* exits, not the process. Call this from a
    /// thread that lives as long as the plugins it launches: if that thread ends, its
    /// plugins receive SIGKILL. Stopping, failing, reaping or dropping a plugin kills its
    /// whole group (ADR 0013).
    pub fn activate(
        &mut self,
        command: Command,
        manifest: Manifest,
        now: u64,
    ) -> Result<u64, Error> {
        let request = self.reserve_launch(manifest, now)?;
        let mut prepared = Some(request.spawn(command));
        self.attach(&mut prepared, now)
    }
    pub fn pump(&mut self, now: u64) -> Result<(), Error> {
        if now < self.now {
            return Err(Error::Clock);
        }
        self.now = now;
        for worker in self.workers.values_mut() {
            worker.pump(now);
        }
        Ok(())
    }
    pub fn state(&self, id: &str) -> Option<State> {
        self.workers.get(id).map(|w| w.state)
    }
    /// Capability agreed by this live process generation, never merely declared metadata.
    pub fn negotiated_capability(&self, id: &str, generation: u64, capability: &str) -> bool {
        self.workers.get(id).is_some_and(|w| {
            w.generation == generation
                && w.state == State::Ready
                && rad_plugin_contract::supports(&w.capabilities, capability)
        })
    }
    pub fn bindings(&self) -> impl Iterator<Item = (&Manifest, u64, State)> {
        self.workers
            .values()
            .map(|w| (&w.manifest, w.generation, w.state))
    }
    pub fn invoke(
        &mut self,
        id: &str,
        generation: u64,
        action: &str,
        params: Parameters,
        deadline: u64,
    ) -> Result<Correlation, Error> {
        self.invoke_with_credentials(id, generation, action, params, (deadline, None))
    }
    pub fn invoke_with_credentials(
        &mut self,
        id: &str,
        generation: u64,
        action: &str,
        params: Parameters,
        timing: (u64, Option<u64>),
    ) -> Result<Correlation, Error> {
        self.invoke_with_context(
            id,
            generation,
            action,
            params,
            timing,
            InvokeContext::default(),
        )
    }
    pub fn invoke_with_context(
        &mut self,
        id: &str,
        generation: u64,
        action: &str,
        params: Parameters,
        timing: (u64, Option<u64>),
        context: InvokeContext,
    ) -> Result<Correlation, Error> {
        if context.control.as_ref().is_some_and(|c| {
            matches!(
                c.gesture,
                rad_plugin_contract::ControlGesture::HoldStart
                    | rad_plugin_contract::ControlGesture::HoldEnd
            )
        }) {
            return Err(Error::Invalid);
        }
        self.invoke_context_inner(id, generation, action, params, timing, context)
    }
    #[allow(clippy::too_many_arguments)]
    fn invoke_context_inner(
        &mut self,
        id: &str,
        generation: u64,
        action: &str,
        params: Parameters,
        timing: (u64, Option<u64>),
        context: InvokeContext,
    ) -> Result<Correlation, Error> {
        let (deadline, credential_revision) = timing;
        if self
            .workers
            .values()
            .map(|w| w.pending.values().filter(|p| !p.reserved_end).count())
            .sum::<usize>()
            >= MAX_JOBS
        {
            return Err(Error::Capacity);
        }
        if deadline <= self.now {
            return Err(Error::Clock);
        }
        let w = self.workers.get_mut(id).ok_or(Error::Unavailable)?;
        if w.generation != generation || w.state != State::Ready {
            return Err(Error::Unavailable);
        }
        let definition = w
            .manifest
            .actions
            .iter()
            .find(|d| d.id == action)
            .ok_or(Error::Invalid)?;
        params.validate(definition).map_err(|_| Error::Invalid)?;
        validate_settings(&w.manifest, context.settings.as_ref())?;
        if let Some(event) = &context.control {
            use rad_plugin_contract::{ControlGesture, Gesture};
            let gesture = match event.gesture {
                ControlGesture::Press => Gesture::Press,
                ControlGesture::HoldStart | ControlGesture::HoldEnd => Gesture::Hold,
                ControlGesture::LongPress => Gesture::LongPress,
            };
            if !definition.gestures.contains(&gesture) {
                return Err(Error::Invalid);
            }
        }
        if w.pending.values().filter(|p| !p.reserved_end).count() >= MAX_PLUGIN_JOBS {
            return Err(Error::Capacity);
        }
        w.check_credentials(credential_revision)?;
        let c = w.send(Message::Invoke {
            settings: context.settings,
            control: context.control,
            credential_revision,
            action: action.into(),
            parameters: params,
        })?;
        w.pending.insert(
            c.request,
            Pending::with_credentials(deadline, credential_revision),
        );
        Ok(c)
    }
    /// Ask a plugin to list the options of one dynamic field. Not a job: it takes no
    /// action slot, creates no execution or reservation, and never sends `invoke`.
    /// Nothing is sent unless the plugin is ready on API 2 and the query is valid for
    /// its manifest. The result arrives through `take_options`.
    pub fn options(
        &mut self,
        id: &str,
        generation: u64,
        query: OptionsQuery,
        deadline: u64,
    ) -> Result<Correlation, Error> {
        self.options_with_credentials(id, generation, query, deadline, None)
    }
    pub fn options_with_credentials(
        &mut self,
        id: &str,
        generation: u64,
        query: OptionsQuery,
        deadline: u64,
        credential_revision: Option<u64>,
    ) -> Result<Correlation, Error> {
        self.options_with_settings(id, generation, query, deadline, credential_revision, None)
    }
    pub fn options_with_settings(
        &mut self,
        id: &str,
        generation: u64,
        query: OptionsQuery,
        deadline: u64,
        credential_revision: Option<u64>,
        settings: Option<Parameters>,
    ) -> Result<Correlation, Error> {
        if deadline <= self.now {
            return Err(Error::Clock);
        }
        let w = self.workers.get_mut(id).ok_or(Error::Unavailable)?;
        if w.generation != generation
            || w.state != State::Ready
            || !rad_plugin_contract::supports(&w.capabilities, rad_plugin_contract::DYNAMIC_OPTIONS)
        {
            return Err(Error::Unavailable);
        }
        let definitions = match query.scope {
            OptionsScope::Action => &w.manifest.actions,
            OptionsScope::Visual => &w.manifest.visuals,
        };
        definitions
            .iter()
            .find(|d| d.id == query.definition)
            .ok_or(Error::Invalid)?
            .validate_options_context(&query.field, &query.context)
            .map_err(|_| Error::Invalid)?;
        if w.queries.len() >= MAX_PLUGIN_OPTIONS {
            return Err(Error::Capacity);
        }
        w.check_credentials(credential_revision)?;
        validate_settings(&w.manifest, settings.as_ref())?;
        let c = w.send(Message::Options {
            settings,
            credential_revision,
            scope: query.scope,
            definition: query.definition,
            field: query.field,
            context: query.context,
        })?;
        w.queries.insert(
            c.request,
            Pending::with_credentials(deadline, credential_revision),
        );
        Ok(c)
    }
    /// Slots currently occupied by this generation, including undrained results
    /// and timed-out requests whose real replies have not arrived. Does not reserve.
    /// Missing/stale generations report zero; callers must still check availability.
    pub fn options_in_use(&self, id: &str, generation: u64) -> usize {
        self.workers
            .get(id)
            .filter(|worker| worker.generation == generation)
            .map_or(0, |worker| worker.queries.len())
    }

    /// Next finished options query, once. See `OptionsOutcome`.
    pub fn take_options(&mut self) -> Option<(Correlation, OptionsOutcome)> {
        self.workers.values_mut().find_map(|w| {
            let generation = w.generation;
            worker::take(&mut w.queries).map(|(request, outcome)| {
                (
                    Correlation {
                        generation,
                        request,
                    },
                    outcome,
                )
            })
        })
    }
    /// Next finished invocation, each reported exactly once. An invocation past its
    /// deadline is reported as `Failure { code: TimedOut }` and stays a residual: it
    /// keeps counting in `resources`, retains its credential revision and blocks
    /// `idle`/`remove` until its late reply is drained or the process is reaped, and
    /// neither reports it again. Termination before the deadline is reported as `Failed`.
    pub fn take_completion(&mut self) -> Option<(Correlation, Outcome)> {
        self.workers.values_mut().find_map(|w| {
            let generation = w.generation;
            worker::take(&mut w.pending).map(|(request, outcome)| {
                (
                    Correlation {
                        generation,
                        request,
                    },
                    outcome,
                )
            })
        })
    }
    pub fn resources(&self) -> (usize, usize) {
        (
            self.workers
                .values()
                .filter(|w| w.child.is_some() || w.launch_pending)
                .count(),
            self.workers.values().map(|w| w.pending.len()).sum(),
        )
    }
    pub fn remove(&mut self, id: &str) -> Result<(), Error> {
        let Some(w) = self.workers.get_mut(id) else {
            return Ok(());
        };
        if w.child.is_some() || w.launch_pending || !w.pending.is_empty() || !w.queries.is_empty() {
            return Err(Error::Residual);
        }
        self.workers.remove(id);
        Ok(())
    }
    /// Best effort kill; slots remain occupied until pump observes reap and results drain.
    pub fn retire(&mut self, id: &str) {
        if let Some(w) = self.workers.get_mut(id) {
            w.stop();
        }
    }
    /// No queued, in-flight, or residual work remains. A child is still owned until reap.
    pub fn idle(&self, id: &str) -> bool {
        self.workers.get(id).is_none_or(Worker::idle)
    }
    pub fn stopped_normally(&self, id: &str) -> bool {
        self.workers.get(id).is_some_and(|w| w.stopped_normally)
    }
    pub fn shutdown(&mut self) {
        for w in self.workers.values_mut() {
            w.stop();
        }
    }
    pub fn subscribe(
        &mut self,
        id: &str,
        source: &str,
        parameters: Parameters,
        ttl_ms: u64,
    ) -> Result<Correlation, Error> {
        self.subscribe_with_credentials(id, source, parameters, ttl_ms, None)
    }
    pub fn subscribe_with_credentials(
        &mut self,
        id: &str,
        source: &str,
        parameters: Parameters,
        ttl_ms: u64,
        credential_revision: Option<u64>,
    ) -> Result<Correlation, Error> {
        self.subscribe_with_settings(id, source, parameters, ttl_ms, credential_revision, None)
    }
    pub fn subscribe_with_settings(
        &mut self,
        id: &str,
        source: &str,
        parameters: Parameters,
        ttl_ms: u64,
        credential_revision: Option<u64>,
        settings: Option<Parameters>,
    ) -> Result<Correlation, Error> {
        if !(1..=60000).contains(&ttl_ms) {
            return Err(Error::Invalid);
        }
        let expires = self.now.checked_add(ttl_ms).ok_or(Error::Clock)?;
        let w = self.workers.get_mut(id).ok_or(Error::Unavailable)?;
        if w.state != State::Ready {
            return Err(Error::Unavailable);
        }
        if w.visuals.len() >= MAX_VISUALS || !w.unsubscribes.is_empty() {
            return Err(Error::Capacity);
        }
        parameters
            .validate(
                w.manifest
                    .visuals
                    .iter()
                    .find(|d| d.id == source)
                    .ok_or(Error::Invalid)?,
            )
            .map_err(|_| Error::Invalid)?;
        w.check_credentials(credential_revision)?;
        validate_settings(&w.manifest, settings.as_ref())?;
        let c = w.send(Message::Subscribe {
            settings,
            credential_revision,
            source: source.into(),
            parameters,
        })?;
        w.visuals.insert(
            c.request,
            Visual {
                observed_current: false,
                received_state: false,
                renewals: 0,
                credential_revision,
                state: VisualState::Unavailable,
                expires,
                ttl: ttl_ms,
            },
        );
        Ok(c)
    }
    pub fn unsubscribe(&mut self, id: &str, key: Correlation) -> Result<(), Error> {
        let w = self.workers.get_mut(id).ok_or(Error::Unavailable)?;
        if key.generation != w.generation || !w.visuals.contains_key(&key.request) {
            return Err(Error::Invalid);
        }
        w.visuals.remove(&key.request);
        if w.state == State::Ready {
            match w.send(Message::Unsubscribe { subscription: key }) {
                Ok(_) => {}
                Err(Error::Capacity) => w.unsubscribes.push_back(key),
                Err(error) => {
                    w.fail(Diagnostic::Protocol);
                    return Err(error);
                }
            }
        }
        Ok(())
    }
    pub fn visual(&self, id: &str, key: Correlation) -> Option<&VisualState> {
        let w = self.workers.get(id)?;
        (w.generation == key.generation)
            .then(|| w.visuals.get(&key.request))
            .flatten()
            .map(|v| &v.state)
    }
    /// Accepted payloadless renewals for the current subscription, saturating.
    pub fn visual_renewals(&self, id: &str, key: Correlation) -> u64 {
        self.workers
            .get(id)
            .filter(|w| w.generation == key.generation)
            .and_then(|w| w.visuals.get(&key.request))
            .map_or(0, |v| v.renewals)
    }
    pub fn visual_valid_until(&self, id: &str, key: Correlation) -> Option<u64> {
        self.workers
            .get(id)
            .filter(|w| w.generation == key.generation)
            .and_then(|w| w.visuals.get(&key.request))
            .map(|v| v.expires)
    }
    /// Owned process only; callers must not interpret absence as group cleanup.
    pub fn process_id(&self, id: &str) -> Option<u32> {
        self.workers
            .get(id)?
            .child
            .as_ref()
            .map(std::process::Child::id)
    }
    pub fn credential_leak_detected(&self, id: &str) -> bool {
        self.workers
            .get(id)
            .is_some_and(|w| w.stderr.credential_leak_detected || w.protocol_credential_leak)
    }
    /// Most recent sanitized stderr lines, bounded and rate-limited per process.
    pub fn stderr(&self, id: &str) -> Vec<String> {
        self.workers
            .get(id)
            .map(|w| w.stderr.lines.iter().cloned().collect())
            .unwrap_or_default()
    }
    pub fn diagnostics(&self, id: &str) -> Vec<Diagnostic> {
        self.workers
            .get(id)
            .map(|w| w.diagnostics.iter().copied().collect())
            .unwrap_or_default()
    }
    /// Structured bounded diagnosis; no plugin-controlled stderr or message text.
    pub fn last_diagnostic(&self, id: &str) -> Option<Diagnostic> {
        self.workers.get(id)?.diagnostics.back().copied()
    }
    /// Borrowed descriptors remain valid until the next mutable host operation.
    pub fn io_sources(&self) -> Vec<(u64, std::os::fd::BorrowedFd<'_>, bool)> {
        self.workers
            .values()
            .flat_map(|w| {
                w.io_sources()
                    .into_iter()
                    .map(|(fd, write)| (w.generation, fd, write))
            })
            .collect()
    }
    /// Earliest real deadline or retained budget continuation; pipe and exit use readiness.
    pub fn next_poll(&self) -> Option<u64> {
        self.workers
            .values()
            .filter_map(|w| w.next_deadline(self.now))
            .min()
    }
}
/// Move this ticket to a bounded setup worker, never spawn on the runtime event loop.
/// Dropping a ticket intentionally leaves its slot reserved: timeout is not proof
/// that an in-flight setup worker has stopped. Complete it even on spawn failure.
pub struct LaunchRequest {
    owner: Arc<()>,
    manifest: Manifest,
    generation: u64,
    deadline: u64,
}
pub struct Prepared {
    owner: Arc<()>,
    id: String,
    generation: u64,
    worker: Option<Worker>,
}
impl LaunchRequest {
    /// Spawns the plugin in its own process group with `PR_SET_PDEATHSIG` SIGKILL.
    /// The death signal is tied to the *calling thread*: run this on a thread that
    /// outlives the plugin (the runtime's long-lived admin thread). If that thread
    /// terminates, every plugin it spawned receives SIGKILL.
    pub fn spawn(self, command: Command) -> Prepared {
        let worker = Worker::spawn(
            command,
            self.manifest.clone(),
            self.generation,
            self.deadline,
        )
        .ok();
        Prepared {
            owner: self.owner,
            id: self.manifest.id,
            generation: self.generation,
            worker,
        }
    }
}
impl Host {
    pub fn reserve_launch(&mut self, manifest: Manifest, now: u64) -> Result<LaunchRequest, Error> {
        if now < self.now {
            return Err(Error::Clock);
        }
        manifest
            .validate(rad_plugin_contract::HOST_API, TARGET)
            .map_err(|_| Error::Invalid)?;
        if self.workers.contains_key(&manifest.id) {
            return Err(Error::Residual);
        }
        if self.workers.len() >= MAX_PLUGINS {
            return Err(Error::PluginLimit);
        }
        let generation = self
            .next_generation
            .checked_add(1)
            .ok_or(Error::Exhausted)?;
        let deadline = now.checked_add(5000).ok_or(Error::Clock)?;
        self.next_generation = generation;
        self.now = now;
        self.workers.insert(
            manifest.id.clone(),
            Worker::reserved(manifest.clone(), generation, deadline),
        );
        Ok(LaunchRequest {
            owner: Arc::clone(&self.owner),
            manifest,
            generation,
            deadline,
        })
    }
    /// On a wrong-host/stale token error, retain caller ownership instead of dropping a child.
    pub fn attach(&mut self, prepared: &mut Option<Prepared>, now: u64) -> Result<u64, Error> {
        if now < self.now {
            return Err(Error::Clock);
        }
        let p = prepared.as_ref().ok_or(Error::Invalid)?;
        if !Arc::ptr_eq(&self.owner, &p.owner) {
            return Err(Error::Invalid);
        }
        let slot = self
            .workers
            .get_mut(&p.id)
            .filter(|w| w.launch_pending && w.generation == p.generation)
            .ok_or(Error::Invalid)?;
        let expired = slot.state != State::Starting || now >= slot.hello_deadline;
        let p = prepared.take().expect("checked token");
        if let Some(mut worker) = p.worker {
            if slot.stopped_normally {
                worker.stop();
            } else if expired {
                worker.fail(Diagnostic::Deadline);
            }
            *slot = worker;
        } else {
            slot.launch_pending = false;
            slot.state = State::Reaped;
            slot.diagnostics.push_back(Diagnostic::Exited);
        }
        self.now = now;
        Ok(p.generation)
    }
}

mod credentials;
pub use credentials::CredentialState;

fn validate_settings(manifest: &Manifest, settings: Option<&Parameters>) -> Result<(), Error> {
    if let Some(settings) = settings {
        let schema = manifest.settings.as_ref().ok_or(Error::Invalid)?;
        let definition = rad_plugin_contract::Definition {
            id: "settings".into(),
            label: "Settings".into(),
            schema: schema.clone(),
            credential_slots: vec![],
            response_timeout: None,
            gestures: rad_plugin_contract::default_gestures(),
        };
        settings.validate(&definition).map_err(|_| Error::Invalid)?;
    }
    Ok(())
}
