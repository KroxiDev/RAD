//! Execution admission and dispatch, using the published configuration snapshot.
use super::{
    Authority, BTreeMap, Completion, DispatchId, DispatchIntent, Effect, ExecutionError,
    ExecutionId, NativeBrightness, Origin, Phase, Rejection, RequestId, StorageState, Target,
    VecDeque,
};

impl Authority {
    fn admit(
        &mut self,
        target: Target,
        trigger: rad_core::execution::Trigger,
    ) -> Result<(ExecutionId, Vec<Effect>), ExecutionError> {
        self.admit_control(target, trigger, None)
    }
    pub(super) fn admit_control(
        &mut self,
        target: Target,
        trigger: rad_core::execution::Trigger,
        control: Option<rad_core::input::ControlEvent>,
    ) -> Result<(ExecutionId, Vec<Effect>), ExecutionError> {
        if control.is_some_and(|c| c.gesture == rad_core::input::ControlGesture::HoldStart)
            && self.holds.iter().flatten().count() + self.releasing_holds.len()
                >= rad_core::input::KEY_COUNT
        {
            return Err(ExecutionError::Capacity);
        }
        let plugins = self.plugin_selection(target);
        if let Some(control) = control {
            self.validate_control(target, control)?;
        }
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
        if let Some(control) = control {
            self.physical.insert(a.id, control);
            if control.gesture == rad_core::input::ControlGesture::HoldStart {
                self.engine
                    .pin_result(a.id)
                    .expect("physical holds bounded by key count below result capacity");
                self.holds[control.key] = Some(a.id);
            }
        }
        if self.engine.snapshot(a.id).is_some() {
            self.runs.insert(a.id, self.revision);
            self.plugin_runs.insert(a.id, plugins.bindings);
        }
        Ok((a.id, self.dispatch_effects(a.dispatches)))
    }
    pub(super) fn run_request(
        &mut self,
        request: RequestId,
        origin: Origin,
        target: Target,
    ) -> Vec<Effect> {
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
    /// Report how a dispatched job ended. `Succeeded` and `Failed` are its real
    /// termination; `TimedOut` is the hand-over of its residual to the adapter, which
    /// keeps it under its own limits (see `Engine::complete`). Either way the job's
    /// retained configuration revision and plugin binding are released here, as
    /// nothing after dispatch reads them.
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
    pub(super) fn dispatch_effects(&mut self, intents: Vec<DispatchIntent>) -> Vec<Effect> {
        self.execution_revision = self.execution_revision.wrapping_add(1);
        let mut e = vec![];
        let mut intents: VecDeque<_> = intents.into();
        while let Some(mut intent) = intents.pop_front() {
            intent.control = self.physical.get(&intent.id.execution()).copied();
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
            } else if intent.operation == rad_core::execution::Operation::Sleep {
                e.extend(self.enter_sleep());
                intents.extend(
                    self.engine
                        .complete(intent.id, Completion::Succeeded)
                        .expect("fresh sleep dispatch, prevalidated scheduling horizon"),
                );
            } else if let rad_core::execution::Operation::Brightness(direction) = intent.operation {
                let revision = self.runs[&intent.id.execution()];
                self.jobs.insert(intent.id, revision);
                if self.state == StorageState::Ready {
                    // Engine credit bounds this queue to MAX_JOBS, including residuals.
                    self.brightness_queue.push_back(NativeBrightness {
                        id: intent.id,
                        direction,
                        deadline: intent.deadline_ms,
                    });
                } else {
                    e.extend(
                        self.job_complete(
                            intent.id,
                            Completion::Failed(rad_core::execution::Failure::ActionFailed),
                        )
                        .expect("new brightness dispatch"),
                    );
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
        self.runs.retain(|id, _| {
            self.engine.snapshot(*id).is_some()
                || self.holds.contains(&Some(*id))
                || self.releasing_holds.contains(id)
        });
        self.plugin_runs.retain(|id, _| {
            self.engine.snapshot(*id).is_some()
                || self.holds.contains(&Some(*id))
                || self.releasing_holds.contains(id)
        });
        self.physical.retain(|id, _| {
            self.engine.snapshot(*id).is_some()
                || self.holds.contains(&Some(*id))
                || self.releasing_holds.contains(id)
        });
        self.refresh_views();
        e.extend(self.drain());
        e
    }
    pub fn installed_types(&mut self, types: Vec<rad_core::configuration::ExternalType>) {
        self.installed = types;
        self.refresh_views();
    }
}
