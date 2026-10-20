//! Pure publication actor. Persistence effects are trusted internal ports, not IPC views.
use rad_core::configuration::{
    Candidate, Configuration, ConfigurationData, ConfigurationError, Edit, Origin,
};
use rad_core::execution::{
    Completion, DispatchId, DispatchIntent, ExecutionError, ExecutionId, Target,
};
use rad_core::identity::EntityIdentity;
use std::collections::{BTreeMap, VecDeque};
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CommitId {
    session: u128,
    sequence: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestId {
    session: u128,
    sequence: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceLimit {
    DocumentBytes,
    StorageCapacity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    ResourceExhausted(ResourceLimit),
    Applied { revision: i64, changed: bool },
    NotApplied,
    Uncertain,
    RecoveryRequired,
}
#[derive(Debug)]
pub enum Effect {
    DisplayClockOverflow,
    Display {
        brightness: u8,
        asleep: bool,
    },
    ExportExpired {
        id: CommitId,
    },
    Collect {
        id: CommitId,
        roots: Vec<i64>,
    },
    CollectionFinished {
        outcome: Outcome,
    },
    Accepted {
        request: RequestId,
        execution: ExecutionId,
    },
    Dispatch(DispatchIntent),
    HardwareAccepted {
        execution: ExecutionId,
    },
    InputRejected {
        reason: ExecutionError,
    },
    Wake,
    SelectionChanged {
        folder: EntityIdentity,
    },
    ReadRevision {
        id: CommitId,
        revision: i64,
    },
    Unknown {
        request: RequestId,
    },
    Reconcile {
        id: CommitId,
    },
    Rejected {
        request: RequestId,
        reason: Rejection,
    },
    Persist {
        id: CommitId,
        candidate: Candidate,
        document: Vec<u8>,
    },
    Reply {
        request: RequestId,
        changed: bool,
    },
}
#[derive(Debug, PartialEq, Eq)]
pub struct EditorView {
    pub revision: i64,
    pub brightness: u8,
    pub history_operations: usize,
    pub active: EntityIdentity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageState {
    Ready,
    Uncertain,
    RecoveryRequired,
}
#[derive(Debug)]
pub struct Ticket(RequestId);
impl Ticket {
    pub fn request(&self) -> RequestId {
        self.0
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum AdmissionError {
    Capacity,
    Overflow,
    InvalidTime,
}
#[derive(PartialEq, Eq)]
enum Phase {
    Reserved,
    Queued,
    Started,
    Response,
}
struct Slot {
    connection: u64,
    origin: Origin,
    phase: Phase,
    deadline: u64,
}
struct Pending {
    id: CommitId,
    candidate: Option<Candidate>,
    cursor: Option<usize>,
    request: RequestId,
}
/// Trusted protocol DTO preparation, evaluated only at its FIFO turn. No I/O.
pub type EditPreparation =
    Box<dyn FnOnce(&Configuration) -> Result<Vec<Edit>, ConfigurationError> + Send>;
pub enum Command {
    PrepareEdits(Vec<EditPreparation>),
    RunId {
        id: rad_core::identity::ShortId,
    },
    Edits(Vec<Edit>),
    Undo,
    Redo,
    Run {
        target: Target,
    },
    RestorePortable {
        data: ConfigurationData,
        installation: u128,
        counters: rad_core::identity::IdentityCounters,
        confirmed: bool,
    },
    Restore {
        data: ConfigurationData,
        confirmed: bool,
    },
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Internal preparation closures and configuration payloads are not diagnostics.
        f.write_str(match self {
            Self::PrepareEdits(_) => "Command::PrepareEdits",
            Self::Edits(_) => "Command::Edits",
            Self::RunId { .. } => "Command::RunId",
            Self::Run { .. } => "Command::Run",
            Self::Undo => "Command::Undo",
            Self::Redo => "Command::Redo",
            Self::Restore { .. } => "Command::Restore",
            Self::RestorePortable { .. } => "Command::RestorePortable",
        })
    }
}
impl From<Vec<Edit>> for Command {
    fn from(v: Vec<Edit>) -> Self {
        Self::Edits(v)
    }
}
struct ExportPin {
    connection: u64,
    revision: i64,
    idle: u64,
    total: u64,
}
pub struct Authority {
    editor_projection: Option<queries::EditorProjection>,
    editor_revision: Option<u64>,
    observed_executions: std::collections::BTreeSet<ExecutionId>,
    automation_results: Vec<ResultView>,
    automation_active: Option<rad_core::identity::Binding>,
    editor_active: Option<rad_core::identity::Binding>,
    automation_folders: Vec<FolderView>,
    automation_appearance: Vec<AppearanceView>,
    automation_view: Vec<EntityView>,
    automation_revision: Option<u64>,
    last_activity: u64,
    held: [bool; 32],
    asleep: bool,
    sleep_deadline: Option<u64>,
    exports: BTreeMap<CommitId, ExportPin>,
    collection: Option<CommitId>,
    input: rad_core::input::Input,
    engine: rad_core::execution::Engine,
    installed: Vec<rad_core::configuration::ExternalType>,
    plugins: BTreeMap<String, plugins::Plugin>,
    next_plugin_selection: u64,
    plugin_barrier: Option<std::sync::Arc<plugins::Barrier>>,
    plugin_activation: bool,
    plugin_runs: BTreeMap<ExecutionId, BTreeMap<String, plugins::ExecutionBinding>>,
    plugin_jobs: BTreeMap<DispatchId, (String, plugins::ExecutionBinding)>,
    runs: BTreeMap<ExecutionId, i64>,
    jobs: BTreeMap<DispatchId, i64>,
    published: Candidate,
    active: EntityIdentity,
    revision: i64,
    history: Vec<i64>,
    cursor: usize,
    now: u64,
    session: u128,
    state: StorageState,
    pending: Option<Pending>,
    awaiting: bool,
    next: u64,
    tickets: BTreeMap<RequestId, Slot>,
    next_ticket: u64,
    queue: VecDeque<(RequestId, Command)>,
}
impl Authority {
    /// Convenience constructor for deterministic epoch-zero callers.
    pub fn new(published: Candidate, revision: i64, session: std::num::NonZeroU128) -> Self {
        Self::new_at(published, revision, session, 0)
    }
    /// All actor, input and engine deadlines share the adapter monotonic epoch.
    pub fn new_at(
        published: Candidate,
        revision: i64,
        session: std::num::NonZeroU128,
        now_ms: u64,
    ) -> Self {
        let mut actor = Self {
            editor_projection: None,
            editor_revision: Some(1),
            observed_executions: std::collections::BTreeSet::new(),
            automation_results: vec![],
            automation_active: None,
            editor_active: None,
            automation_folders: vec![],
            automation_appearance: vec![],
            automation_view: vec![],
            automation_revision: Some(1),
            collection: None,
            exports: BTreeMap::new(),
            last_activity: now_ms,
            held: [false; 32],
            asleep: false,
            sleep_deadline: None,
            input: rad_core::input::Input::default(),
            engine: rad_core::execution::Engine::new(session.get(), 1)
                .expect("nonzero session and counter"),
            runs: BTreeMap::new(),
            jobs: BTreeMap::new(),
            installed: vec![],
            plugins: BTreeMap::new(),
            next_plugin_selection: 1,
            plugin_barrier: None,
            plugin_activation: false,
            plugin_runs: BTreeMap::new(),
            plugin_jobs: BTreeMap::new(),
            active: published.configuration().data().principal,
            published,
            revision,
            history: vec![revision],
            cursor: 0,
            now: now_ms,
            session: session.get(),
            state: StorageState::Ready,
            pending: None,
            awaiting: false,
            next: 1,
            tickets: BTreeMap::new(),
            next_ticket: 1,
            queue: VecDeque::new(),
        };
        actor
            .engine
            .advance(now_ms)
            .expect("empty engine accepts initial epoch");
        let _ = actor.sync_input();
        let _ = actor.project_display();
        actor.refresh_views();
        actor
    }
    pub fn editor_view(&self, origin: Origin) -> Option<EditorView> {
        (origin == Origin::Editor).then(|| EditorView {
            revision: self.revision,
            brightness: self
                .published
                .configuration()
                .settings()
                .brightness_percent(),
            history_operations: self.history.len() - 1,
            active: self.active,
        })
    }
    pub fn storage_state(&self) -> StorageState {
        self.state
    }
    pub fn reserve(&mut self, connection: u64, origin: Origin) -> Result<Ticket, AdmissionError> {
        if self.tickets.len() >= 32
            || self
                .tickets
                .values()
                .filter(|s| s.connection == connection)
                .count()
                >= 4
        {
            return Err(AdmissionError::Capacity);
        }
        let deadline = self.now.checked_add(5000).ok_or(AdmissionError::Overflow)?;
        let sequence = self.next_ticket;
        self.next_ticket = sequence.checked_add(1).ok_or(AdmissionError::Overflow)?;
        let id = RequestId {
            session: self.session,
            sequence,
        };
        self.tickets.insert(
            id,
            Slot {
                connection,
                origin,
                phase: Phase::Reserved,
                deadline,
            },
        );
        Ok(Ticket(id))
    }
    pub fn submit(&mut self, ticket: Ticket, command: impl Into<Command>) -> Vec<Effect> {
        let request = ticket.0;
        let Some(slot) = self.tickets.get_mut(&request) else {
            return vec![];
        };
        if slot.phase != Phase::Reserved {
            return vec![];
        }
        slot.phase = Phase::Queued;
        if self.state != StorageState::Ready {
            return vec![self.reject(request, Rejection::StorageUncertain)];
        }
        let origin = slot.origin;
        let command = command.into();
        if let Command::RunId { id } = command {
            let data = self.published.configuration().data();
            let target = data
                .buttons
                .iter()
                .find(|b| b.binding.short() == id)
                .map(|b| Target::Button(b.binding.identity()))
                .or_else(|| {
                    data.buttons
                        .iter()
                        .flat_map(|b| b.actions.iter())
                        .find(|a| a.binding.short() == id)
                        .map(|a| Target::Action(a.binding.identity()))
                });
            return match target {
                Some(target) => self.run_request(request, origin, target),
                None => vec![self.reject(request, Rejection::Execution(ExecutionError::NotFound))],
            };
        }
        if let Command::Run { target } = command {
            return self.run_request(request, origin, target);
        }
        self.queue.push_back((request, command));
        self.drain()
    }
    /// Transport consumes a terminal response. Disconnect cannot release a started commit.
    pub fn finish(&mut self, request: RequestId) -> bool {
        if self
            .tickets
            .get(&request)
            .is_some_and(|s| s.phase == Phase::Response)
        {
            self.tickets.remove(&request);
            true
        } else {
            false
        }
    }
    fn reject(&mut self, request: RequestId, reason: Rejection) -> Effect {
        self.tickets.get_mut(&request).unwrap().phase = Phase::Response;
        Effect::Rejected { request, reason }
    }
    fn reply(&mut self, request: RequestId, changed: bool) -> Effect {
        self.tickets.get_mut(&request).unwrap().phase = Phase::Response;
        Effect::Reply { request, changed }
    }
    fn command_id(&mut self) -> Option<CommitId> {
        let sequence = self.next;
        self.next = sequence.checked_add(1)?;
        Some(CommitId {
            session: self.session,
            sequence,
        })
    }
    fn persist(
        &mut self,
        request: RequestId,
        candidate: Candidate,
        cursor: Option<usize>,
        reserve_counters: bool,
    ) -> Vec<Effect> {
        if self.plugin_barrier.is_some() {
            return vec![self.reject(request, Rejection::PublicationPending)];
        }
        let settings = candidate.configuration().settings().data();
        if self.last_activity.checked_add(settings.sleep_ms).is_none()
            || self.now.checked_add(settings.press_threshold_ms).is_none()
        {
            return vec![self.reject(request, Rejection::Invalid)];
        }
        if candidate.configuration() == self.published.configuration()
            && (!reserve_counters
                || candidate.registry().counters() == self.published.registry().counters())
        {
            return vec![self.reply(request, false)];
        }
        let document = match rad_codec::encode(candidate.configuration()) {
            Ok(document) => document,
            Err(rad_codec::CodecError::TooLarge) => {
                return vec![self.reject(
                    request,
                    Rejection::ResourceExhausted(ResourceLimit::DocumentBytes),
                )];
            }
            Err(_) => return vec![self.reject(request, Rejection::Invalid)],
        };
        let Some(id) = self.command_id() else {
            return vec![self.reject(request, Rejection::Capacity)];
        };
        self.pending = Some(Pending {
            id,
            candidate: Some(candidate.clone()),
            cursor,
            request,
        });
        self.awaiting = true;
        self.tickets.get_mut(&request).unwrap().phase = Phase::Started;
        vec![Effect::Persist {
            id,
            candidate,
            document,
        }]
    }
    fn drain(&mut self) -> Vec<Effect> {
        let mut effects = vec![];
        while self.pending.is_none()
            && self.collection.is_none()
            && self.state == StorageState::Ready
        {
            let Some((request, command)) = self.queue.pop_front() else {
                break;
            };
            let origin = self.tickets[&request].origin;
            match command {
                Command::Run { .. } | Command::RunId { .. } => {
                    effects.push(self.reject(request, Rejection::Invalid))
                }
                Command::RestorePortable {
                    data,
                    confirmed,
                    installation,
                    counters,
                } => {
                    match self.published.configuration().prepare_portable_restore(
                        self.published.registry(),
                        origin,
                        confirmed,
                        data,
                        installation,
                        counters,
                    ) {
                        Ok(restored) => {
                            effects.extend(self.persist(request, restored.candidate, None, true))
                        }
                        Err(error) => {
                            effects.push(self.reject(request, Rejection::Configuration(error)))
                        }
                    }
                }
                Command::Restore { data, confirmed } => {
                    match self.published.configuration().prepare_restore(
                        self.published.registry(),
                        origin,
                        confirmed,
                        data,
                    ) {
                        Ok(candidate) => {
                            effects.extend(self.persist(request, candidate, None, false))
                        }
                        Err(error) => {
                            effects.push(self.reject(request, Rejection::Configuration(error)))
                        }
                    }
                }
                Command::PrepareEdits(steps) => {
                    let candidate =
                        steps
                            .into_iter()
                            .try_fold(self.published.clone(), |candidate, prepare| {
                                let edits = prepare(candidate.configuration())?;
                                candidate.configuration().prepare_edits(
                                    candidate.registry(),
                                    origin,
                                    &edits,
                                )
                            });
                    match candidate {
                        Ok(candidate) => {
                            if self.validate_plugin_changes(&candidate).is_err() {
                                effects.push(self.reject(request, Rejection::Invalid));
                            } else {
                                effects.extend(self.persist(request, candidate, None, false));
                            }
                        }
                        Err(error) => {
                            effects.push(self.reject(request, Rejection::Configuration(error)))
                        }
                    }
                }
                Command::Edits(edits) => {
                    match self.published.configuration().prepare_edits(
                        self.published.registry(),
                        origin,
                        &edits,
                    ) {
                        Ok(candidate) => {
                            if self.validate_plugin_changes(&candidate).is_err() {
                                effects.push(self.reject(request, Rejection::Invalid));
                            } else {
                                effects.extend(self.persist(request, candidate, None, false));
                            }
                        }
                        Err(error) => {
                            effects.push(self.reject(request, Rejection::Configuration(error)))
                        }
                    }
                }
                Command::Undo | Command::Redo => {
                    if self.plugin_barrier.is_some() {
                        effects.push(self.reject(request, Rejection::PublicationPending));
                        continue;
                    }
                    let index = if matches!(command, Command::Undo) {
                        self.cursor.checked_sub(1)
                    } else {
                        self.cursor
                            .checked_add(1)
                            .filter(|i| *i < self.history.len())
                    };
                    if origin != Origin::Editor || index.is_none() {
                        effects.push(self.reject(request, Rejection::Invalid));
                        continue;
                    }
                    let index = index.unwrap();
                    let Some(id) = self.command_id() else {
                        effects.push(self.reject(request, Rejection::Capacity));
                        continue;
                    };
                    self.pending = Some(Pending {
                        id,
                        candidate: None,
                        cursor: Some(index),
                        request,
                    });
                    self.awaiting = true;
                    self.tickets.get_mut(&request).unwrap().phase = Phase::Started;
                    effects.push(Effect::ReadRevision {
                        id,
                        revision: self.history[index],
                    });
                }
            }
        }
        effects
    }
    pub fn reconcile(&mut self) -> Vec<Effect> {
        if self.state == StorageState::Uncertain && !self.awaiting {
            let Some(id) = self.command_id() else {
                self.state = StorageState::RecoveryRequired;
                return vec![];
            };
            if let Some(pending) = self.pending.as_mut() {
                pending.id = id;
            } else if self.collection.is_some() {
                self.collection = Some(id);
            } else {
                return vec![];
            }
            self.awaiting = true;
            return vec![Effect::Reconcile { id }];
        }
        vec![]
    }
    pub fn complete(&mut self, id: CommitId, outcome: Outcome) -> Vec<Effect> {
        if self.collection == Some(id) && self.awaiting {
            return self.collection_complete(outcome);
        }
        if !self.awaiting
            || self
                .pending
                .as_ref()
                .filter(|p| p.candidate.is_some())
                .map(|p| p.id)
                != Some(id)
        {
            return vec![];
        }
        let request = self.pending.as_ref().unwrap().request;
        if matches!(outcome, Outcome::Uncertain | Outcome::RecoveryRequired)
            || matches!(outcome, Outcome::Applied { changed: false, .. })
            || matches!(outcome,Outcome::Applied{revision,..} if revision<=self.revision)
        {
            self.state = if outcome == Outcome::RecoveryRequired {
                StorageState::RecoveryRequired
            } else {
                StorageState::Uncertain
            };
            self.awaiting = false;
            self.invalidate_input();
            let mut e = vec![Effect::Unknown { request }];
            while let Some((request, _)) = self.queue.pop_front() {
                e.push(self.reject(request, Rejection::StorageUncertain));
            }
            return e;
        }
        self.state = StorageState::Ready;
        self.awaiting = false;
        if matches!(outcome, Outcome::NotApplied | Outcome::ResourceExhausted(_)) {
            self.pending = None;
            let reason = match outcome {
                Outcome::ResourceExhausted(limit) => Rejection::ResourceExhausted(limit),
                _ => Rejection::Persistence,
            };
            let mut e = vec![self.reject(request, reason)];
            e.extend(self.drain());
            return e;
        }
        if let Outcome::Applied { revision, changed } = outcome {
            let pending = self.pending.take().unwrap();
            let previous_settings = self.published.configuration().settings().data();
            self.published = pending.candidate.unwrap();
            let settings = self.published.configuration().settings().data();
            let display_changed = previous_settings.brightness_percent
                != settings.brightness_percent
                || previous_settings.sleep_ms != settings.sleep_ms;
            if !self
                .published
                .configuration()
                .data()
                .folders
                .iter()
                .any(|f| f.binding.identity() == self.active)
            {
                self.active = self.published.configuration().data().principal;
            }
            self.revision = revision;
            if let Some(cursor) = pending.cursor {
                self.cursor = cursor;
            } else if changed {
                self.history.truncate(self.cursor + 1);
                self.history.push(revision);
                if self.history.len() > 26 {
                    self.history.remove(0);
                }
                self.cursor = self.history.len() - 1;
            }
            let mut e = if display_changed {
                self.project_display()
            } else {
                vec![]
            };
            e.extend(self.sync_input());
            e.push(self.reply(request, changed));
            self.refresh_views();
            e.extend(self.drain());
            return e;
        }
        vec![]
    }
}
impl Authority {
    pub fn advance(&mut self, now: u64) -> Result<Vec<Effect>, AdmissionError> {
        let mut e = self.advance_clock(now)?;
        let output = self
            .input
            .handle(now, rad_core::input::Change::Tick)
            .map_err(|_| AdmissionError::Overflow)?;
        e.extend(self.input_effects(output));
        e.extend(self.sleep_if_due());
        Ok(e)
    }
    fn advance_clock(&mut self, now: u64) -> Result<Vec<Effect>, AdmissionError> {
        if now < self.now {
            return Err(AdmissionError::InvalidTime);
        }
        let engine_due = self
            .engine
            .next_deadline()
            .is_some_and(|deadline| deadline <= now);
        let intents = self
            .engine
            .advance(now)
            .map_err(|_| AdmissionError::Overflow)?;
        self.now = now;
        let mut effects = if engine_due || !intents.is_empty() {
            self.dispatch_effects(intents)
        } else {
            vec![]
        };
        let expired: Vec<_> = self
            .tickets
            .iter()
            .filter(|(_, s)| {
                matches!(s.phase, Phase::Reserved | Phase::Queued) && s.deadline <= now
            })
            .map(|(id, _)| *id)
            .collect();
        self.queue.retain(|(id, _)| !expired.contains(id));
        effects.extend(
            expired
                .into_iter()
                .map(|id| self.reject(id, Rejection::Expired)),
        );
        let expired: Vec<_> = self
            .exports
            .iter()
            .filter(|(_, p)| p.idle.min(p.total) <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.exports.remove(&id);
            effects.push(Effect::ExportExpired { id });
        }
        Ok(effects)
    }
    pub fn next_deadline(&self) -> Option<u64> {
        self.tickets
            .values()
            .filter(|s| matches!(s.phase, Phase::Reserved | Phase::Queued))
            .map(|s| s.deadline)
            .chain(self.sleep_deadline)
            .chain(self.engine.next_deadline())
            .chain(self.input.next_deadline_ms())
            .chain(self.exports.values().map(|p| p.idle.min(p.total)))
            .min()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    Configuration(ConfigurationError),
    Execution(ExecutionError),
    Persistence,
    ResourceExhausted(ResourceLimit),
    Invalid,
    StorageUncertain,
    Expired,
    Capacity,
    PublicationPending,
}
impl Authority {
    pub fn disconnect(&mut self, connection: u64) {
        self.exports.retain(|_, pin| pin.connection != connection);
        let removed: Vec<_> = self
            .tickets
            .iter()
            .filter(|(_, s)| s.connection == connection && s.phase != Phase::Started)
            .map(|(id, _)| *id)
            .collect();
        self.queue.retain(|(id, _)| !removed.contains(id));
        for id in removed {
            self.tickets.remove(&id);
        }
    }
}

impl Authority {
    pub fn loaded(&mut self, id: CommitId, data: Result<ConfigurationData, ()>) -> Vec<Effect> {
        if !self.awaiting
            || self
                .pending
                .as_ref()
                .filter(|p| p.candidate.is_none())
                .map(|p| p.id)
                != Some(id)
        {
            return vec![];
        }
        let pending = self.pending.take().unwrap();
        self.awaiting = false;
        let candidate = data.ok().and_then(|data| {
            self.published
                .configuration()
                .prepare_restore(self.published.registry(), Origin::Editor, true, data)
                .ok()
        });
        let mut e = match candidate {
            Some(candidate) => self.persist(pending.request, candidate, pending.cursor, false),
            None => vec![self.reject(pending.request, Rejection::Persistence)],
        };
        e.extend(self.drain());
        e
    }
}

impl Authority {
    pub fn navigate(
        &mut self,
        origin: Origin,
        folder: EntityIdentity,
    ) -> Result<Vec<Effect>, Rejection> {
        if self.state != StorageState::Ready {
            return Err(Rejection::StorageUncertain);
        }
        if origin != Origin::Editor
            || !self
                .published
                .configuration()
                .data()
                .folders
                .iter()
                .any(|f| f.binding.identity() == folder)
        {
            return Err(Rejection::Invalid);
        }
        self.active = folder;
        let mut e = self.sync_input();
        e.push(Effect::SelectionChanged { folder });
        Ok(e)
    }
}

impl Authority {
    fn admit(
        &mut self,
        target: Target,
        trigger: rad_core::execution::Trigger,
    ) -> Result<(ExecutionId, Vec<Effect>), ExecutionError> {
        let plugins = self.plugin_selection(target);
        // Failed external preflight exposes no executors to Engine. Engine keeps
        // permission/busy error precedence and rejects before producing intents;
        // do not duplicate its authorization policy here.
        let installed = if plugins.is_ok() {
            self.installed.as_slice()
        } else {
            &[]
        };
        let empty_budgets = BTreeMap::new();
        let budgets = plugins.as_ref().map_or(&empty_budgets, |p| &p.budgets);
        let a = self.engine.admit_with_response_budgets(
            self.published.configuration(),
            target,
            trigger,
            installed,
            budgets,
        )?;
        let plugins = plugins?;
        self.observed_executions.insert(a.id);
        if self.engine.snapshot(a.id).is_some() {
            self.runs.insert(a.id, self.revision);
            self.plugin_runs.insert(a.id, plugins.bindings);
        }
        Ok((a.id, self.dispatch_effects(a.dispatches)))
    }
    fn run_request(&mut self, request: RequestId, origin: Origin, target: Target) -> Vec<Effect> {
        let trigger = if origin == Origin::Editor {
            rad_core::execution::Trigger::Editor
        } else {
            rad_core::execution::Trigger::Automation
        };
        match self.admit(target, trigger) {
            Ok((execution, dispatches)) => {
                self.tickets.get_mut(&request).unwrap().phase = Phase::Response;
                let mut e = vec![Effect::Accepted { request, execution }];
                e.extend(dispatches);
                e
            }
            Err(reason) => vec![self.reject(request, Rejection::Execution(reason))],
        }
    }
    pub fn job_complete(
        &mut self,
        id: DispatchId,
        result: Completion,
    ) -> Result<Vec<Effect>, ExecutionError> {
        if !self.jobs.contains_key(&id) {
            return Err(ExecutionError::UnknownDispatch);
        }
        let intents = self.engine.complete(id, result)?;
        self.jobs.remove(&id);
        self.plugin_jobs.remove(&id);
        Ok(self.dispatch_effects(intents))
    }
    fn dispatch_effects(&mut self, intents: Vec<DispatchIntent>) -> Vec<Effect> {
        let mut e = vec![];
        let mut intents: VecDeque<_> = intents.into();
        while let Some(intent) = intents.pop_front() {
            if let rad_core::execution::Operation::Navigate(folder) = intent.operation {
                let exists = self
                    .published
                    .configuration()
                    .data()
                    .folders
                    .iter()
                    .any(|f| f.binding.identity() == folder);
                let completion = if exists {
                    self.active = folder;
                    e.push(Effect::SelectionChanged { folder });
                    Completion::Succeeded
                } else {
                    Completion::Failed(rad_core::execution::Failure::ActionFailed)
                };
                intents.extend(
                    self.engine
                        .complete(intent.id, completion)
                        .expect("fresh navigation dispatch, prevalidated scheduling horizon"),
                );
                if exists {
                    e.extend(self.sync_input());
                }
            } else {
                let revision = self.runs[&intent.id.execution()];
                self.jobs.insert(intent.id, revision);
                if let rad_core::execution::Operation::External { type_key, .. } = &intent.operation
                    && let Some(g) = self
                        .plugin_runs
                        .get(&intent.id.execution())
                        .and_then(|p| p.get(type_key.plugin()))
                {
                    self.plugin_jobs
                        .insert(intent.id, (type_key.plugin().into(), g.clone()));
                }
                e.push(Effect::Dispatch(intent));
            }
        }
        self.runs
            .retain(|id, _| self.engine.snapshot(*id).is_some());
        self.plugin_runs
            .retain(|id, _| self.engine.snapshot(*id).is_some());
        self.refresh_views();
        e
    }
}
impl Authority {
    pub fn retained_revisions(&self) -> Vec<i64> {
        self.history
            .iter()
            .copied()
            .chain(std::iter::once(self.revision))
            .chain(self.runs.values().copied())
            .chain(self.jobs.values().copied())
            .chain(self.exports.values().map(|p| p.revision))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}
impl Authority {
    pub fn installed_types(&mut self, types: Vec<rad_core::configuration::ExternalType>) {
        self.installed = types;
        self.refresh_views();
    }
}
impl Authority {
    pub fn input_report(
        &mut self,
        now: u64,
        pressed: [bool; 32],
    ) -> Result<Vec<Effect>, AdmissionError> {
        let mut e = self.advance_clock(now)?;
        if pressed != self.held {
            self.last_activity = now;
            self.held = pressed;
            e.extend(self.reset_sleep_deadline());
        }
        let output = match self
            .input
            .handle(now, rad_core::input::Change::Report(pressed))
        {
            Ok(output) => output,
            Err(_) => {
                // Preserve Engine intents already produced; consume this report without
                // admitting a partial set of presses or retrying the impossible deadline.
                let folder = self
                    .published
                    .configuration()
                    .data()
                    .folders
                    .iter()
                    .find(|f| f.binding.identity() == self.active)
                    .unwrap()
                    .binding;
                self.input
                    .handle(
                        now,
                        rad_core::input::Change::Layout {
                            folder,
                            buttons: [None; 32],
                        },
                    )
                    .expect("valid empty layout");
                self.input
                    .handle(now, rad_core::input::Change::Report(pressed))
                    .expect("unassigned keys cannot overflow");
                e.extend(self.sync_input());
                e.push(Effect::InputRejected {
                    reason: ExecutionError::TimeOverflow,
                });
                return Ok(e);
            }
        };
        e.extend(self.input_effects(output));
        if self.state != StorageState::Ready {
            self.invalidate_input();
        }
        Ok(e)
    }
    fn invalidate_input(&mut self) {
        let folder = self
            .published
            .configuration()
            .data()
            .folders
            .iter()
            .find(|f| f.binding.identity() == self.active)
            .unwrap()
            .binding;
        let _ = self
            .input
            .handle(
                self.now,
                rad_core::input::Change::Layout {
                    folder,
                    buttons: [None; 32],
                },
            )
            .expect("validated empty layout");
        let _ = self.sync_input();
    }
    fn input_effects(&mut self, output: Vec<rad_core::input::Output>) -> Vec<Effect> {
        let mut e = vec![];
        for output in output {
            match output {
                rad_core::input::Output::Wake => {
                    self.asleep = false;
                    e.extend(self.reset_sleep_deadline());
                    e.push(Effect::Wake);
                    e.push(self.display_effect());
                }
                rad_core::input::Output::Candidate { button, .. } => {
                    if self.state == StorageState::Ready {
                        match self.admit(
                            Target::Button(button.identity()),
                            rad_core::execution::Trigger::Hardware,
                        ) {
                            Ok((execution, intents)) => {
                                e.push(Effect::HardwareAccepted { execution });
                                e.extend(intents);
                            }
                            Err(reason) => e.push(Effect::InputRejected { reason }),
                        }
                    }
                }
            }
        }
        e
    }
    fn sync_input(&mut self) -> Vec<Effect> {
        let data = self.published.configuration().data();
        let folder = data
            .folders
            .iter()
            .find(|f| f.binding.identity() == self.active)
            .expect("validated selection");
        let buttons = std::array::from_fn(|i| {
            folder.slots[i].and_then(|id| {
                data.buttons
                    .iter()
                    .find(|b| b.binding.identity() == id)
                    .map(|b| b.binding)
            })
        });
        let output = self
            .input
            .handle(
                self.now,
                rad_core::input::Change::Layout {
                    folder: folder.binding,
                    buttons,
                },
            )
            .expect("validated layout and monotonic clock");
        let more = self
            .input
            .handle(
                self.now,
                rad_core::input::Change::Threshold(data.settings.press_threshold_ms),
            )
            .expect("threshold update cannot overflow");
        let mut e = self.input_effects(output);
        e.extend(self.input_effects(more));
        self.plugin_runs
            .retain(|id, _| self.engine.snapshot(*id).is_some());
        self.refresh_views();
        e
    }
}

impl Authority {
    pub fn collect(&mut self) -> Result<Vec<Effect>, Rejection> {
        if self.state != StorageState::Ready {
            return Err(Rejection::StorageUncertain);
        }
        if self.pending.is_some() || self.collection.is_some() {
            return Err(Rejection::Capacity);
        }
        let id = self.command_id().ok_or(Rejection::Capacity)?;
        self.collection = Some(id);
        self.awaiting = true;
        Ok(vec![Effect::Collect {
            id,
            roots: self.retained_revisions(),
        }])
    }
    fn collection_complete(&mut self, outcome: Outcome) -> Vec<Effect> {
        let outcome = if matches!(outcome,Outcome::Applied{revision,changed} if changed || revision!=self.revision)
        {
            Outcome::Uncertain
        } else {
            outcome
        };
        self.awaiting = false;
        let mut e = vec![Effect::CollectionFinished { outcome }];
        if matches!(outcome, Outcome::Uncertain | Outcome::RecoveryRequired) {
            self.state = if outcome == Outcome::RecoveryRequired {
                StorageState::RecoveryRequired
            } else {
                StorageState::Uncertain
            };
            self.invalidate_input();
            while let Some((request, _)) = self.queue.pop_front() {
                e.push(self.reject(request, Rejection::StorageUncertain));
            }
        } else {
            self.collection = None;
            self.state = StorageState::Ready;
            e.extend(self.drain());
        }
        e
    }
}
#[derive(Debug)]
pub struct ExportLease {
    snapshot: Candidate,
    id: CommitId,
    revision: i64,
}
impl ExportLease {
    pub fn snapshot(&self) -> &Candidate {
        &self.snapshot
    }
    pub fn revision(&self) -> i64 {
        self.revision
    }
}
impl Authority {
    pub fn pin_export(
        &mut self,
        connection: u64,
        origin: Origin,
    ) -> Result<ExportLease, Rejection> {
        if origin != Origin::Editor {
            return Err(Rejection::Invalid);
        }
        if self.state != StorageState::Ready {
            return Err(Rejection::StorageUncertain);
        }
        if self.exports.len() >= 2 || self.exports.values().any(|p| p.connection == connection) {
            return Err(Rejection::Capacity);
        }
        let total = self.now.checked_add(120000).ok_or(Rejection::Capacity)?;
        let idle = self.now + 30000;
        let id = self.command_id().ok_or(Rejection::Capacity)?;
        self.exports.insert(
            id,
            ExportPin {
                connection,
                revision: self.revision,
                idle,
                total,
            },
        );
        Ok(ExportLease {
            snapshot: self.published.clone(),
            id,
            revision: self.revision,
        })
    }
    pub fn release_export(&mut self, lease: ExportLease) -> bool {
        self.exports.remove(&lease.id).is_some()
    }
    pub fn touch_export(&mut self, lease: &ExportLease) -> Result<(), Rejection> {
        let p = self.exports.get_mut(&lease.id).ok_or(Rejection::Invalid)?;
        p.idle = self
            .now
            .checked_add(30000)
            .ok_or(Rejection::Capacity)?
            .min(p.total);
        Ok(())
    }
}

impl Authority {
    /// Advance Engine and request/resource deadlines before a non-tick event. Input ordering is preserved.
    pub fn before_event(&mut self, now: u64) -> Result<Vec<Effect>, AdmissionError> {
        self.advance_clock(now)
    }
}

impl Authority {
    fn display_effect(&self) -> Effect {
        Effect::Display {
            brightness: self
                .published
                .configuration()
                .settings()
                .data()
                .brightness_percent,
            asleep: self.asleep,
        }
    }
    fn reset_sleep_deadline(&mut self) -> Vec<Effect> {
        let duration = self.published.configuration().settings().data().sleep_ms;
        self.sleep_deadline = if duration == 0 || self.asleep {
            None
        } else {
            self.last_activity.checked_add(duration)
        };
        if duration > 0 && !self.asleep && self.sleep_deadline.is_none() {
            vec![Effect::DisplayClockOverflow]
        } else {
            vec![]
        }
    }
    fn sleep_if_due(&mut self) -> Vec<Effect> {
        if self
            .sleep_deadline
            .is_some_and(|deadline| deadline <= self.now)
        {
            self.sleep_deadline = None;
            self.asleep = true;
            self.input
                .handle(self.now, rad_core::input::Change::Sleep)
                .expect("monotonic sleep");
            return vec![self.display_effect()];
        }
        vec![]
    }
    fn project_display(&mut self) -> Vec<Effect> {
        let mut e = self.reset_sleep_deadline();
        e.extend(self.sleep_if_due());
        if !e.iter().any(|e| matches!(e, Effect::Display { .. })) {
            e.push(self.display_effect());
        }
        e
    }
}

mod queries;
pub use queries::*;

impl Authority {
    pub fn input_disconnect(&mut self, now: u64) -> Result<Vec<Effect>, AdmissionError> {
        let e = self.before_event(now)?;
        self.input
            .handle(now, rad_core::input::Change::Disconnect)
            .expect("monotonic disconnect");
        self.held = [false; 32];
        Ok(e)
    }
}

impl Authority {
    pub fn navigate_id(
        &mut self,
        origin: Origin,
        id: rad_core::identity::ShortId,
    ) -> Result<Vec<Effect>, Rejection> {
        let folder = self
            .published
            .configuration()
            .data()
            .folders
            .iter()
            .find(|f| f.binding.short() == id)
            .ok_or(Rejection::Invalid)?
            .binding
            .identity();
        self.navigate(origin, folder)
    }
}

impl Authority {
    /// Correlation only for the current pending persistence/history operation.
    pub fn commit_request(&self, id: CommitId) -> Option<RequestId> {
        self.pending
            .as_ref()
            .filter(|pending| pending.id == id)
            .map(|pending| pending.request)
    }
}

pub mod plugins;

mod plugin_credentials;

impl Authority {
    /// Store's confirmed collection returns the surviving ledger before queued edits resume.
    pub fn collected(
        &mut self,
        id: CommitId,
        outcome: Outcome,
        registry: Option<rad_core::identity::IdentityRegistry>,
    ) -> Vec<Effect> {
        if self.collection != Some(id) || !self.awaiting {
            return vec![];
        }
        if let Outcome::Applied {
            revision,
            changed: false,
        } = outcome
        {
            let candidate = registry
                .filter(|r| {
                    r.installation() == self.published.registry().installation()
                        && r.counters() == self.published.registry().counters()
                })
                .and_then(|registry| {
                    Configuration::from_data(
                        self.published.configuration().data().clone(),
                        &registry,
                    )
                    .ok()
                    .map(|configuration| (configuration, registry))
                })
                .and_then(|(configuration, registry)| {
                    configuration
                        .prepare_restore(
                            &registry,
                            Origin::Editor,
                            true,
                            configuration.data().clone(),
                        )
                        .ok()
                });
            match candidate {
                Some(candidate) if revision == self.revision => self.published = candidate,
                _ => return self.complete(id, Outcome::RecoveryRequired),
            }
        }
        self.complete(id, outcome)
    }
}
