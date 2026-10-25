//! One serialized persistence lane, plus at most one Authority operation already started.
use crate::{
    storage::{Job, JobId, ResultValue, StorageWorker, WorkerError},
    transfers::{self, Context, Metadata, Operation, Output, Transfers},
};
use rad_authority::{Authority, CommitId, Effect, Outcome, RequestId};
use rad_store::{CommitOutcome, Store};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};
#[derive(Clone, Copy)]
enum Active {
    Authority {
        job: JobId,
        commit: CommitId,
        reading: bool,
    },
    Transfer(JobId),
    Cleanup {
        job: JobId,
        request: Option<RequestId>,
    },
}
impl Active {
    fn job(self) -> JobId {
        match self {
            Self::Authority { job, .. } | Self::Transfer(job) | Self::Cleanup { job, .. } => job,
        }
    }
}
#[derive(PartialEq)]
enum BindingState {
    Bound,
    InFlight,
    Release,
}
struct Binding {
    connection: u64,
    token: String,
    state: BindingState,
}
type TransferResult = (JobId, Result<Output, transfers::Error>);
pub struct Persistence {
    worker: StorageWorker,
    active: Option<Active>,
    deferred: Option<(CommitId, Job)>,
    transfer_result: Option<TransferResult>,
    bindings: BTreeMap<RequestId, Binding>,
    released: Vec<String>,
    unbound: BTreeSet<String>,
    unbound_active: Option<String>,
}
impl Persistence {
    pub fn can_submit_transfer(&self) -> bool {
        self.active.is_none()
            && self.deferred.is_none()
            && self.transfer_result.is_none()
            && self.released.len() < 2
            && self.unbound.is_empty()
            && !self
                .bindings
                .values()
                .any(|b| b.state == BindingState::Release)
    }
    /// Retained preflight for which no Authority ticket could be bound. At most
    /// two lease handles, not an external job FIFO. Cleanup happens via poll.
    pub fn release_unbound(&mut self, transfer_id: String) -> Result<(), WorkerError> {
        if self.unbound.contains(&transfer_id) {
            return Ok(());
        }
        if self.unbound.len() >= 2 || self.bindings.values().any(|b| b.token == transfer_id) {
            return Err(WorkerError::Busy);
        }
        if !self.transfer_metadata().is_some_and(|m| {
            m.handles
                .iter()
                .any(|h| h.transfer_id == transfer_id && h.pending)
        }) {
            return Err(WorkerError::Unavailable);
        }
        self.unbound.insert(transfer_id);
        Ok(())
    }

    fn from_worker(worker: StorageWorker) -> Self {
        Self {
            worker,
            active: None,
            deferred: None,
            transfer_result: None,
            bindings: BTreeMap::new(),
            released: vec![],
            unbound: BTreeSet::new(),
            unbound_active: None,
        }
    }
    pub fn new(store: Store, wake: Arc<mio::Waker>) -> Result<Self, WorkerError> {
        Ok(Self::from_worker(StorageWorker::spawn(store, wake)?))
    }
    pub fn with_transfers(
        store: Store,
        transfers: Transfers,
        wake: Arc<mio::Waker>,
    ) -> Result<Self, WorkerError> {
        Ok(Self::from_worker(StorageWorker::with_transfers(
            store, transfers, wake,
        )?))
    }
    pub fn submit_transfer(
        &mut self,
        context: Context,
        operation: Operation,
    ) -> Result<JobId, WorkerError> {
        if !self.can_submit_transfer() {
            return Err(WorkerError::Busy);
        }
        let id = self.worker.submit(Job::Transfer { context, operation })?;
        self.active = Some(Active::Transfer(id));
        Ok(id)
    }
    pub fn take_transfer(&mut self) -> Option<TransferResult> {
        self.transfer_result.take()
    }
    pub fn transfer_metadata(&self) -> Option<&Metadata> {
        self.worker.transfer_metadata()
    }
    pub fn take_released(&mut self) -> Vec<String> {
        std::mem::take(&mut self.released)
    }
    pub fn bind_media(
        &mut self,
        request: RequestId,
        connection: u64,
        transfer_id: String,
    ) -> Result<(), WorkerError> {
        if self.bindings.len() >= 2
            || self.bindings.contains_key(&request)
            || self.bindings.values().any(|b| b.token == transfer_id)
            || self.unbound.contains(&transfer_id)
        {
            return Err(WorkerError::Busy);
        }
        let valid = self.transfer_metadata().is_some_and(|m| {
            m.handles
                .iter()
                .any(|h| h.transfer_id == transfer_id && h.connection == connection && h.pending)
        });
        if !valid {
            return Err(WorkerError::Unavailable);
        }
        self.bindings.insert(
            request,
            Binding {
                connection,
                token: transfer_id,
                state: BindingState::Bound,
            },
        );
        Ok(())
    }
    /// For an Authority ticket removed before starting (e.g. connection gone).
    /// Started bindings are never cancelled. Cleanup is scheduled by poll on worker.
    pub fn release_media(&mut self, request: RequestId) {
        if let Some(b) = self.bindings.get_mut(&request)
            && b.state == BindingState::Bound
        {
            b.state = BindingState::Release;
        }
    }
    pub fn route(&mut self, actor: &mut Authority, effects: Vec<Effect>) -> Vec<Effect> {
        let mut queue: VecDeque<_> = effects.into();
        let mut output = vec![];
        while let Some(effect) = queue.pop_front() {
            let (commit, job) = match effect {
                Effect::Persist { id, candidate, .. } => {
                    let binding = actor
                        .commit_request(id)
                        .and_then(|r| self.bindings.get_mut(&r));
                    let job = if let Some(b) = binding {
                        b.state = BindingState::InFlight;
                        Job::CommitMedia {
                            candidate,
                            connection: b.connection,
                            transfer_id: b.token.clone(),
                        }
                    } else {
                        Job::Commit(candidate)
                    };
                    (id, job)
                }
                Effect::ReadRevision { id, revision } => (id, Job::ReadRevision(revision)),
                Effect::Reconcile { id } => (id, Job::Reconcile),
                Effect::Collect { id, roots } => (id, Job::Collect(roots)),
                other => {
                    if let Effect::Rejected { request, .. } = &other {
                        self.release_media(*request);
                    }
                    output.push(other);
                    continue;
                }
            };
            // Authority emits one outstanding storage operation. Preserve its owned candidate
            // while an unrelated transfer/cleanup is in flight, not a queue of new commands.
            if self.active.is_some() {
                if self.deferred.is_none() {
                    self.deferred = Some((commit, job));
                } else {
                    queue.extend(Self::failed(
                        actor,
                        commit,
                        matches!(job, Job::ReadRevision(_)),
                    ));
                }
            } else {
                queue.extend(self.start_authority(actor, commit, job));
            }
        }
        let next = self.drive(actor);
        if !next.is_empty() {
            output.extend(self.route(actor, next));
        }
        output
    }
    fn failed(actor: &mut Authority, commit: CommitId, reading: bool) -> Vec<Effect> {
        if reading {
            actor.loaded(commit, Err(()))
        } else {
            actor.complete(commit, Outcome::RecoveryRequired)
        }
    }
    fn start_authority(
        &mut self,
        actor: &mut Authority,
        commit: CommitId,
        job: Job,
    ) -> Vec<Effect> {
        let reading = matches!(job, Job::ReadRevision(_));
        match self.worker.try_submit(job) {
            Ok(job) => {
                self.active = Some(Active::Authority {
                    job,
                    commit,
                    reading,
                });
                vec![]
            }
            Err((WorkerError::Busy, job)) => {
                self.deferred = Some((commit, *job));
                vec![]
            }
            Err((WorkerError::Unavailable, _)) => Self::failed(actor, commit, reading),
        }
    }
    fn drive(&mut self, actor: &mut Authority) -> Vec<Effect> {
        if self.active.is_some() {
            return vec![];
        }
        if let Some((commit, job)) = self.deferred.take() {
            return self.start_authority(actor, commit, job);
        }
        if let Some((&request, binding)) = self
            .bindings
            .iter()
            .find(|(_, b)| b.state == BindingState::Release)
            && let Ok(job) = self.worker.submit(Job::ReleaseMedia {
                transfer_id: binding.token.clone(),
            })
        {
            self.active = Some(Active::Cleanup {
                job,
                request: Some(request),
            });
        }
        if self.active.is_none()
            && let Some(token) = self.unbound.iter().next().cloned()
            && let Ok(job) = self.worker.submit(Job::ReleaseMedia {
                transfer_id: token.clone(),
            })
        {
            self.unbound_active = Some(token);
            self.active = Some(Active::Cleanup { job, request: None });
        }
        vec![]
    }
    pub fn poll(&mut self, actor: &mut Authority) -> Vec<Effect> {
        let Some(active) = self.active else {
            let effects = self.drive(actor);
            return self.route(actor, effects);
        };
        let completion = match self.worker.take() {
            Ok(None) => return vec![],
            other => other,
        };
        self.active = None;
        let released = self.worker.take_released();
        self.bindings.retain(|_, b| !released.contains(&b.token));
        self.released.extend(released);
        let effects = match completion {
            Ok(Some((id, value))) if id == active.job() => match (active, value) {
                (Active::Authority { commit, .. }, ResultValue::Commit(outcome)) => {
                    actor.complete(commit, outcome_from_store(outcome))
                }
                (
                    Active::Authority { commit, .. },
                    ResultValue::Collected { outcome, registry },
                ) => actor.collected(commit, outcome_from_store(outcome), registry),
                (Active::Authority { commit, .. }, ResultValue::Revision(data)) => {
                    actor.loaded(commit, data.map_err(|_| ()))
                }
                (Active::Transfer(job), ResultValue::Transfer(result)) => {
                    self.transfer_result = Some((job, result));
                    vec![]
                }
                (Active::Cleanup { request, .. }, ResultValue::Released(_)) => {
                    if let Some(request) = request {
                        self.bindings.remove(&request);
                    }
                    if let Some(token) = self.unbound_active.take() {
                        self.unbound.remove(&token);
                    }
                    vec![]
                }
                _ => self.lost(actor, active),
            },
            _ => self.lost(actor, active),
        };
        self.route(actor, effects)
    }
    fn lost(&mut self, actor: &mut Authority, active: Active) -> Vec<Effect> {
        match active {
            Active::Authority {
                commit, reading, ..
            } => Self::failed(actor, commit, reading),
            Active::Transfer(job) => {
                self.transfer_result = Some((job, Err(transfers::Error::Io)));
                vec![]
            }
            Active::Cleanup { .. } => vec![],
        }
    }
}
fn outcome_from_store(value: CommitOutcome) -> Outcome {
    match value {
        CommitOutcome::Applied { revision, changed } => Outcome::Applied { revision, changed },
        CommitOutcome::NotApplied(rad_store::StoreError::Capacity) => {
            Outcome::ResourceExhausted(rad_authority::ResourceLimit::StorageCapacity)
        }
        CommitOutcome::NotApplied(_) => Outcome::NotApplied,
        CommitOutcome::Uncertain { .. } => Outcome::Uncertain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rad_core::{
        configuration::{Configuration, Edit, Origin, SettingsData},
        identity::IdentityRegistry,
    };
    #[test]
    fn lost_history_read_rejects_and_releases_authority_instead_of_ignored_complete() {
        let base = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
        let mut actor = Authority::new(base, 1, std::num::NonZeroU128::new(99).unwrap());
        let ticket = actor.reserve(1, Origin::Editor).unwrap();
        let first = ticket.request();
        let effects = actor.submit(
            ticket,
            vec![Edit::SetSettings(SettingsData {
                brightness_percent: 11,
                ..Default::default()
            })],
        );
        let commit = effects
            .iter()
            .find_map(|e| {
                if let Effect::Persist { id, .. } = e {
                    Some(*id)
                } else {
                    None
                }
            })
            .unwrap();
        // Pure callback routing test, not evidence of a real SQLite commit.
        actor.complete(
            commit,
            Outcome::Applied {
                revision: 2,
                changed: true,
            },
        );
        actor.finish(first);
        let ticket = actor.reserve(1, Origin::Editor).unwrap();
        let request = ticket.request();
        let effects = actor.submit(ticket, rad_authority::Command::Undo);
        let commit = effects
            .iter()
            .find_map(|e| {
                if let Effect::ReadRevision { id, .. } = e {
                    Some(*id)
                } else {
                    None
                }
            })
            .unwrap();
        let effects = Persistence::failed(&mut actor, commit, true);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e,Effect::Rejected{request:r,..} if *r==request))
        );
        actor.finish(request);
        let ticket = actor.reserve(1, Origin::Editor).unwrap();
        assert!(
            actor
                .submit(ticket, vec![Edit::SetSettings(SettingsData::default())])
                .iter()
                .any(|e| matches!(e, Effect::Persist { .. }))
        );
    }
}

#[cfg(test)]
mod capacity_contract {
    use super::*;
    #[test]
    fn storage_capacity_preserves_resource_cause() {
        assert_eq!(
            outcome_from_store(CommitOutcome::NotApplied(rad_store::StoreError::Capacity)),
            Outcome::ResourceExhausted(rad_authority::ResourceLimit::StorageCapacity)
        );
    }
}
