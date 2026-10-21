//! Pure execution admission and scheduling. Adapters execute returned intents.
//!
//! The caller serializes events and advances the logical clock before reporting
//! completions. At an exact deadline, advance first: timeout wins. A timeout is
//! not cancellation; report actual termination later to release the residual job.
//! Triggers and compatible installed types are trusted authority inputs, not
//! client-supplied credentials. This module does not authenticate or perform I/O.
use crate::configuration::{ActionKind, ButtonData, Configuration, ExternalType, Mode};
use crate::identity::EntityIdentity;

pub const RESPONSE_TIMEOUT_MS: u64 = 5_000;
pub const MAX_EXECUTIONS: usize = 16;
pub const MAX_JOBS: usize = 32;
pub const MAX_RESULTS: usize = 256;
pub const RESULT_TTL_MS: u64 = 600_000;

/// Validated response budget supplied by the authority, never parsed from client JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseBudget(u64);
impl ResponseBudget {
    pub fn new(milliseconds: u64) -> Result<Self, ExecutionError> {
        if !(1000..=120_000).contains(&milliseconds) {
            return Err(ExecutionError::InvalidResponseBudget);
        }
        Ok(Self(milliseconds))
    }
    pub fn milliseconds(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExecutionId {
    session: u128,
    counter: u64,
}
impl ExecutionId {
    pub fn session(self) -> u128 {
        self.session
    }
    pub fn counter(self) -> u64 {
        self.counter
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Hardware,
    Editor,
    Automation,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Button(EntityIdentity),
    Action(EntityIdentity),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionError {
    InvalidSeed,
    InvalidResponseBudget,
    NotFound,
    PermissionDenied,
    ExecutorUnavailable,
    Busy,
    Capacity,
    CounterExhausted,
    TimeOverflow,
    TimeReversed,
    UnknownDispatch,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStatus {
    Running,
    Succeeded,
    Failed,
    TimedOut,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionResult {
    pub id: ExecutionId,
    pub owner: EntityIdentity,
    pub target: Target,
    pub status: ExecutionStatus,
    pub steps: Vec<StepResult>,
}
#[derive(Debug)]
pub struct Admission {
    pub id: ExecutionId,
    pub dispatches: Vec<DispatchIntent>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DispatchId {
    execution: ExecutionId,
    index: usize,
}
impl DispatchId {
    pub fn execution(self) -> ExecutionId {
        self.execution
    }
    pub fn index(self) -> usize {
        self.index
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Launcher(crate::configuration::DesktopEntry),
    External {
        type_key: ExternalType,
        parameters: Vec<u8>,
    },
    Navigate(EntityIdentity),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchIntent {
    pub id: DispatchId,
    pub action: crate::identity::Binding,
    pub operation: Operation,
    pub deadline_ms: u64,
    pub control: Option<crate::input::ControlEvent>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    button: ButtonData,
    global: crate::permissions::Permissions,
    folder: EntityIdentity,
    target: Target,
    trigger: Trigger,
}
impl Snapshot {
    pub fn button(&self) -> &ButtonData {
        &self.button
    }
    pub fn global_permissions(&self) -> crate::permissions::Permissions {
        self.global
    }
    pub fn folder(&self) -> EntityIdentity {
        self.folder
    }
    pub fn target(&self) -> Target {
        self.target
    }
    pub fn trigger(&self) -> Trigger {
        self.trigger
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    ActionFailed,
    ServiceUnavailable,
    ExecutorUnavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Waiting,
    Dispatched,
    Succeeded,
    Failed(Failure),
    TimedOut,
    NotDispatched,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepResult {
    pub action: crate::identity::Binding,
    pub index: usize,
    pub status: StepStatus,
}
/// What a step does when its turn comes.
#[derive(Debug, Clone)]
enum Planned {
    Dispatch {
        operation: Operation,
        response_budget_ms: u64,
    },
    /// Deliberate pause: nothing is dispatched and the engine ends it at its deadline.
    Wait(u64),
}
#[derive(Debug, Clone)]
struct Run {
    result: ExecutionResult,
    snapshot: std::sync::Arc<Snapshot>,
    plan: Vec<Planned>,
    deadlines: Vec<Option<u64>>,
    next_index: usize,
    next_due: Option<u64>,
    failure: Option<ExecutionStatus>,
}
#[derive(Debug, Clone)]
struct Terminal {
    result: ExecutionResult,
    finished_at: u64,
}
#[derive(Debug, Clone)]
struct State {
    session: u128,
    next: Option<u64>,
    now: u64,
    residuals: std::collections::BTreeSet<DispatchId>,
    runs: Vec<Run>,
    results: std::collections::VecDeque<std::sync::Arc<Terminal>>,
    result_pins: std::collections::BTreeSet<ExecutionId>,
}
#[derive(Debug)]
pub struct Engine {
    state: State,
}
impl Engine {
    /// Start a fresh engine. The authority must never reuse this session/range
    /// while old dispatches may still return; normally `first_counter` is one.
    pub fn new(session: u128, first_counter: u64) -> Result<Self, ExecutionError> {
        if session == 0 || first_counter == 0 {
            return Err(ExecutionError::InvalidSeed);
        }
        Ok(Self {
            state: State {
                session,
                next: Some(first_counter),
                now: 0,
                residuals: std::collections::BTreeSet::new(),
                runs: vec![],
                results: std::collections::VecDeque::new(),
                result_pins: Default::default(),
            },
        })
    }
    /// Validate the entire selection and reserve all its jobs before dispatch.
    /// Service/visual availability is deliberately absent from this contract.
    pub fn admit(
        &mut self,
        configuration: &Configuration,
        target: Target,
        trigger: Trigger,
        installed: &[ExternalType],
    ) -> Result<Admission, ExecutionError> {
        self.admit_with_response_budgets(
            configuration,
            target,
            trigger,
            installed,
            &std::collections::BTreeMap::new(),
        )
    }
    /// Capture trusted per-action response budgets; ordinary admission stays unchanged.
    pub fn admit_with_response_budgets(
        &mut self,
        configuration: &Configuration,
        target: Target,
        trigger: Trigger,
        installed: &[ExternalType],
        budgets: &std::collections::BTreeMap<EntityIdentity, ResponseBudget>,
    ) -> Result<Admission, ExecutionError> {
        self.transition(|state| state.admit(configuration, target, trigger, installed, budgets))
    }
    /// Compact active or retained terminal result. The runtime must apply current
    /// consultation permissions before exposing it to automation clients.
    pub fn result(&self, id: ExecutionId) -> Option<ExecutionResult> {
        self.state
            .runs
            .iter()
            .map(|r| &r.result)
            .chain(self.state.results.iter().map(|r| &r.result))
            .find(|r| r.id == id)
            .cloned()
            .map(|mut result| {
                if self.state.result_pins.contains(&id)
                    && result.status == ExecutionStatus::Succeeded
                {
                    result.status = ExecutionStatus::Running;
                }
                result
            })
    }
    /// Retain an execution while a bounded external cleanup obligation is outstanding.
    pub fn pin_result(&mut self, id: ExecutionId) -> Result<(), ExecutionError> {
        if self.state.result_pins.len() >= MAX_RESULTS {
            return Err(ExecutionError::Capacity);
        }
        if self.result(id).is_none() {
            return Err(ExecutionError::NotFound);
        }
        self.state.result_pins.insert(id);
        Ok(())
    }
    pub fn release_result(&mut self, id: ExecutionId) {
        if self.state.result_pins.remove(&id)
            && let Some(result) = self.state.results.iter_mut().find(|r| r.result.id == id)
        {
            std::sync::Arc::make_mut(result).finished_at = self.state.now;
        }
    }
    /// Cleanup failure never restarts a plan or replaces an earlier failure with success.
    pub fn report_cleanup_failure(&mut self, id: ExecutionId) {
        if let Some(run) = self.state.runs.iter_mut().find(|r| r.result.id == id) {
            run.failure.get_or_insert(ExecutionStatus::Failed);
        } else if let Some(result) = self.state.results.iter_mut().find(|r| r.result.id == id)
            && result.result.status == ExecutionStatus::Succeeded
        {
            std::sync::Arc::make_mut(result).result.status = ExecutionStatus::Failed;
        }
    }
    /// Admission-time configuration, available only while the execution is active.
    pub fn snapshot(&self, id: ExecutionId) -> Option<&Snapshot> {
        self.state
            .runs
            .iter()
            .find(|r| r.result.id == id)
            .map(|r| r.snapshot.as_ref())
    }
    fn transition<T>(
        &mut self,
        operation: impl FnOnce(&mut State) -> Result<T, ExecutionError>,
    ) -> Result<T, ExecutionError> {
        let mut next = self.state.clone();
        let result = operation(&mut next)?;
        self.state = next;
        Ok(result)
    }
}
impl State {
    fn admit(
        &mut self,
        configuration: &Configuration,
        target: Target,
        trigger: Trigger,
        installed: &[ExternalType],
        budgets: &std::collections::BTreeMap<EntityIdentity, ResponseBudget>,
    ) -> Result<Admission, ExecutionError> {
        let data = configuration.data();
        let owner = match target {
            Target::Button(id) => data.buttons.iter().find(|b| b.binding.identity() == id),
            Target::Action(id) => data
                .buttons
                .iter()
                .find(|b| b.actions.iter().any(|a| a.binding.identity() == id)),
        }
        .ok_or(ExecutionError::NotFound)?;
        if self
            .runs
            .iter()
            .any(|r| r.result.owner == owner.binding.identity())
        {
            return Err(ExecutionError::Busy);
        }
        let selected: Vec<_> = owner
            .actions
            .iter()
            .enumerate()
            .filter(|(_, a)| match target {
                Target::Button(_) => true,
                Target::Action(id) => a.binding.identity() == id,
            })
            .collect();
        let permissions = owner.permissions.resolve(data.global_permissions);
        if trigger == Trigger::Automation
            && ((matches!(target, Target::Button(_)) && !permissions.execute())
                || selected
                    .iter()
                    .any(|(_, a)| !a.permissions.resolve(permissions).execute()))
        {
            return Err(ExecutionError::PermissionDenied);
        }
        if selected.iter().any(|(_,a)| matches!(&a.kind, ActionKind::External { type_key, .. } if !installed.contains(type_key))) { return Err(ExecutionError::ExecutorUnavailable); }
        let jobs = self
            .runs
            .iter()
            .flat_map(|r| r.result.steps.iter())
            .filter(|s| matches!(s.status, StepStatus::Waiting | StepStatus::Dispatched))
            .count()
            + self.residuals.len();
        if self.runs.len() >= MAX_EXECUTIONS || jobs + selected.len() > MAX_JOBS {
            return Err(ExecutionError::Capacity);
        }
        let folder = data
            .folders
            .iter()
            .find(|f| f.slots.contains(&Some(owner.binding.identity())))
            .ok_or(ExecutionError::NotFound)?;
        let plan: Vec<_> = selected
            .iter()
            .map(|(_, a)| {
                let response_budget_ms = if matches!(a.kind, ActionKind::External { .. }) {
                    budgets
                        .get(&a.binding.identity())
                        .map_or(RESPONSE_TIMEOUT_MS, |b| b.milliseconds())
                } else {
                    RESPONSE_TIMEOUT_MS
                };
                Ok(Planned::Dispatch {
                    response_budget_ms,
                    operation: match &a.kind {
                        ActionKind::Launcher(entry) => Operation::Launcher(entry.clone()),
                        ActionKind::External {
                            type_key,
                            parameters,
                        } => Operation::External {
                            type_key: type_key.clone(),
                            parameters: parameters.clone(),
                        },
                        ActionKind::EnterFolder(id) => Operation::Navigate(*id),
                        ActionKind::Return => {
                            Operation::Navigate(folder.parent.ok_or(ExecutionError::NotFound)?)
                        }
                        ActionKind::Wait(wait) => return Ok(Planned::Wait(wait.milliseconds())),
                    },
                })
            })
            .collect::<Result<_, ExecutionError>>()?;
        let id = ExecutionId {
            session: self.session,
            counter: self.next.ok_or(ExecutionError::CounterExhausted)?,
        };
        let steps: Vec<_> = selected
            .iter()
            .map(|(index, a)| StepResult {
                action: a.binding,
                index: *index,
                status: StepStatus::Waiting,
            })
            .collect();
        let count = steps.len();
        let result = ExecutionResult {
            id,
            owner: owner.binding.identity(),
            target,
            status: if count == 0 {
                ExecutionStatus::Succeeded
            } else {
                ExecutionStatus::Running
            },
            steps,
        };
        self.next = id.counter.checked_add(1);
        let mut dispatches = vec![];
        if count == 0 {
            self.retain_result(result);
        } else {
            self.runs.push(Run {
                result,
                snapshot: std::sync::Arc::new(Snapshot {
                    button: owner.clone(),
                    global: data.global_permissions,
                    folder: folder.binding.identity(),
                    target,
                    trigger,
                }),
                plan,
                deadlines: vec![None; count],
                next_index: 0,
                next_due: Some(self.now),
                failure: None,
            });
            dispatches = self.pump()?;
        }
        Ok(Admission { id, dispatches })
    }
}
impl Run {
    /// Longest the step may stay in flight: its wait, or the response budget.
    fn budget(&self, i: usize) -> u64 {
        match self.plan[i] {
            Planned::Wait(milliseconds) => milliseconds,
            Planned::Dispatch {
                response_budget_ms, ..
            } => response_budget_ms,
        }
    }
    /// Starts the next step. Only a dispatch yields an intent; a wait just holds the plan.
    fn start(&mut self, now: u64) -> Result<Option<DispatchIntent>, ExecutionError> {
        let i = self.next_index;
        let count = self.result.steps.len();
        // Reserve a representable worst-case response+wait horizon before emitting, so
        // that no later step of this plan can fail to be scheduled.
        let mut horizon = now;
        for position in i..count {
            horizon = horizon
                .checked_add(self.budget(position))
                .ok_or(ExecutionError::TimeOverflow)?;
        }
        let deadline = now
            .checked_add(self.budget(i))
            .ok_or(ExecutionError::TimeOverflow)?;
        self.result.steps[i].status = StepStatus::Dispatched;
        self.deadlines[i] = Some(deadline);
        self.next_index += 1;
        self.next_due = None;
        let Planned::Dispatch { operation, .. } = &self.plan[i] else {
            return Ok(None);
        };
        if self.snapshot.button.mode == Mode::Parallel && self.next_index < count {
            self.next_due = Some(now);
        }
        let step = &self.result.steps[i];
        Ok(Some(DispatchIntent {
            id: DispatchId {
                execution: self.result.id,
                index: step.index,
            },
            action: step.action,
            operation: operation.clone(),
            deadline_ms: deadline,
            control: None,
        }))
    }
    /// Lets the next step start now that nothing holds the plan back.
    fn resume(&mut self, now: u64) {
        if self.failure.is_none() && self.next_index < self.result.steps.len() {
            self.next_due = Some(now);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    Succeeded,
    Failed(Failure),
    TimedOut,
}
impl Engine {
    /// Stop only steps not dispatched yet; in-flight jobs retain their credit and outcome.
    pub fn cancel_pending(&mut self, id: ExecutionId) -> Result<(), ExecutionError> {
        self.transition(|state| {
            if let Some(run) = state.runs.iter_mut().find(|r| r.result.id == id) {
                if run
                    .result
                    .steps
                    .iter()
                    .any(|step| step.status == StepStatus::Waiting)
                {
                    run.stop_pending(ExecutionStatus::Failed);
                }
                let intents = state.pump()?;
                debug_assert!(intents.is_empty());
            }
            Ok(())
        })
    }
    /// Report how a dispatched job ended. `Succeeded` and `Failed` report its real
    /// termination. `TimedOut` hands the job over: Engine ends the execution as
    /// `TimedOut` (once, whether or not its own deadline was processed yet) and frees
    /// the job's credit, while the adapter that owns the residual keeps it under its
    /// own limits until the late reply or its reap. A second report is `UnknownDispatch`.
    pub fn complete(
        &mut self,
        id: DispatchId,
        completion: Completion,
    ) -> Result<Vec<DispatchIntent>, ExecutionError> {
        self.transition(|state| state.complete(id, completion))
    }
    /// Process deadlines at an explicit monotonic millisecond time. Due dispatches
    /// start at this observed time, not at a reconstructed past time. Errors leave
    /// the engine unchanged and return no intents.
    pub fn advance(&mut self, now_ms: u64) -> Result<Vec<DispatchIntent>, ExecutionError> {
        self.transition(|state| {
            if now_ms < state.now {
                return Err(ExecutionError::TimeReversed);
            }
            state.now = now_ms;
            state.results.retain(|r| {
                state.result_pins.contains(&r.result.id) || now_ms - r.finished_at < RESULT_TTL_MS
            });
            state.pump()
        })
    }
    /// Earliest response, dispatch or retention deadline; residuals have no
    /// synthetic termination deadline. Unrepresentable TTL deadlines are omitted.
    pub fn next_deadline(&self) -> Option<u64> {
        self.state
            .runs
            .iter()
            .flat_map(|r| r.deadlines.iter().copied().flatten().chain(r.next_due))
            .chain(
                self.state
                    .results
                    .iter()
                    .filter(|r| !self.state.result_pins.contains(&r.result.id))
                    .filter_map(|r| r.finished_at.checked_add(RESULT_TTL_MS)),
            )
            .min()
    }
}

impl State {
    fn complete(
        &mut self,
        id: DispatchId,
        completion: Completion,
    ) -> Result<Vec<DispatchIntent>, ExecutionError> {
        if self.residuals.remove(&id) {
            // The deadline already ended the step and nothing visible changes: only the credit is freed.
            return Ok(vec![]);
        }
        let run = self
            .runs
            .iter_mut()
            .find(|r| r.result.id == id.execution)
            .ok_or(ExecutionError::UnknownDispatch)?;
        let i = run
            .result
            .steps
            .iter()
            .position(|s| s.index == id.index && s.status == StepStatus::Dispatched)
            // A wait was never dispatched to anyone: only its deadline ends it.
            .filter(|i| matches!(run.plan[*i], Planned::Dispatch { .. }))
            .ok_or(ExecutionError::UnknownDispatch)?;
        run.deadlines[i] = None;
        match completion {
            Completion::Succeeded => run.result.steps[i].status = StepStatus::Succeeded,
            Completion::Failed(reason) => {
                run.result.steps[i].status = StepStatus::Failed(reason);
                run.stop_pending(ExecutionStatus::Failed);
            }
            Completion::TimedOut => {
                // The adapter keeps the residual: Engine holds no credit for it.
                run.result.steps[i].status = StepStatus::TimedOut;
                run.stop_pending(ExecutionStatus::TimedOut);
            }
        }
        if run.snapshot.button.mode == Mode::Sequential {
            run.resume(self.now);
        }
        self.pump()
    }
    fn pump(&mut self) -> Result<Vec<DispatchIntent>, ExecutionError> {
        let mut intents = vec![];
        for run in &mut self.runs {
            // A wait of zero ends in the turn that starts it: repeat until nothing is due.
            loop {
                let mut progressed = false;
                for i in 0..run.result.steps.len() {
                    if !run.deadlines[i].is_some_and(|deadline| deadline <= self.now) {
                        continue;
                    }
                    run.deadlines[i] = None;
                    progressed = true;
                    if matches!(run.plan[i], Planned::Wait(_)) {
                        run.result.steps[i].status = StepStatus::Succeeded;
                        run.resume(self.now);
                    } else {
                        run.result.steps[i].status = StepStatus::TimedOut;
                        self.residuals.insert(DispatchId {
                            execution: run.result.id,
                            index: run.result.steps[i].index,
                        });
                        run.stop_pending(ExecutionStatus::TimedOut);
                    }
                }
                while run.next_due.is_some_and(|due| due <= self.now) {
                    intents.extend(run.start(self.now)?);
                    progressed = true;
                }
                if !progressed {
                    break;
                }
            }
        }
        let mut i = 0;
        while i < self.runs.len() {
            if self.runs[i]
                .result
                .steps
                .iter()
                .all(|s| !matches!(s.status, StepStatus::Waiting | StepStatus::Dispatched))
            {
                let mut run = self.runs.remove(i);
                run.result.status = run.failure.unwrap_or(ExecutionStatus::Succeeded);
                self.retain_result(run.result);
            } else {
                i += 1;
            }
        }
        Ok(intents)
    }
}

impl Run {
    fn stop_pending(&mut self, cause: ExecutionStatus) {
        self.failure.get_or_insert(cause);
        self.next_due = None;
        for (i, step) in self.result.steps.iter_mut().enumerate() {
            // A wait in flight has no effect to collect, so it ends with the plan.
            let waiting_out =
                step.status == StepStatus::Dispatched && matches!(self.plan[i], Planned::Wait(_));
            if step.status == StepStatus::Waiting || waiting_out {
                step.status = StepStatus::NotDispatched;
                self.deadlines[i] = None;
            }
        }
        self.next_index = self.result.steps.len();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub active_executions: usize,
    pub jobs: usize,
    pub residual_jobs: usize,
}
impl Engine {
    pub fn usage(&self) -> Usage {
        Usage {
            active_executions: self.state.runs.len(),
            jobs: self
                .state
                .runs
                .iter()
                .flat_map(|r| r.result.steps.iter())
                .filter(|s| matches!(s.status, StepStatus::Waiting | StepStatus::Dispatched))
                .count()
                + self.state.residuals.len(),
            residual_jobs: self.state.residuals.len(),
        }
    }
}

impl State {
    fn retain_result(&mut self, result: ExecutionResult) {
        self.results.push_back(std::sync::Arc::new(Terminal {
            result,
            finished_at: self.now,
        }));
        while self.results.len() > MAX_RESULTS {
            let index = self
                .results
                .iter()
                .position(|r| !self.result_pins.contains(&r.result.id))
                .expect("bounded result pins");
            self.results.remove(index);
        }
    }
}
