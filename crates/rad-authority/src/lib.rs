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
    ReleaseHold {
        execution: ExecutionId,
    },
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
#[derive(Clone, Copy)]
enum PublicationTarget {
    Client(RequestId),
    Brightness(DispatchId),
}
struct NativeBrightness {
    id: DispatchId,
    direction: rad_core::configuration::BrightnessDirection,
    deadline: u64,
}
struct Pending {
    id: CommitId,
    candidate: Option<Candidate>,
    cursor: Option<usize>,
    target: PublicationTarget,
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
    physical: BTreeMap<ExecutionId, rad_core::input::ControlEvent>,
    holds: [Option<ExecutionId>; 32],
    releasing_holds: std::collections::BTreeSet<ExecutionId>,
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
    execution_revision: u64,
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
    brightness_queue: VecDeque<NativeBrightness>,
    draining: bool,
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
            physical: BTreeMap::new(),
            holds: [None; 32],
            releasing_holds: Default::default(),
            engine: rad_core::execution::Engine::new(session.get(), 1)
                .expect("nonzero session and counter"),
            runs: BTreeMap::new(),
            jobs: BTreeMap::new(),
            execution_revision: 0,
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
            brightness_queue: VecDeque::new(),
            draining: false,
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
    /// Invalidation epoch, not a durable identity. Equality alone is meaningful.
    pub fn execution_revision(&self) -> u64 {
        self.execution_revision
    }
    pub fn input_clock_ms(&self) -> u64 {
        self.now
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

/// Proof that a configuration snapshot is pinned for an export in progress. Authority
/// retains the snapshot's revision until the lease is released, expires (30 s idle, 120 s
/// in total, measured on `advance` time; reported as `Effect::ExportExpired`) or the
/// connection that took it disconnects. At most two leases exist, one per connection.
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

mod clock;
mod display;
mod execution;
mod export;
mod input;
mod navigation;
mod publication;
mod queries;
pub use queries::*;

pub mod plugins;

mod plugin_credentials;
