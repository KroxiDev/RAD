//! Publication, persistence outcomes, history, and retained revision collection.
use super::{
    AdmissionError, Authority, Candidate, Command, CommitId, Completion, Configuration,
    ConfigurationData, Effect, ExecutionError, Origin, Outcome, Pending, Phase, PublicationTarget,
    Rejection, RequestId, ResourceLimit, Slot, StorageState, Target, Ticket,
};

impl Authority {
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
    ///
    /// The runtime calls it once the terminal effect of a request (reply, rejection,
    /// acceptance) has been answered to the client; that frees the request's admission
    /// ticket. Returns `false` while the request is not in its response phase, in
    /// particular a commit whose outcome is unknown, which keeps its ticket until
    /// `reconcile` settles it: the caller keeps the response target and tries again.
    /// A ticket that is never finished keeps occupying admission capacity.
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
    pub(super) fn reject(&mut self, request: RequestId, reason: Rejection) -> Effect {
        self.tickets.get_mut(&request).unwrap().phase = Phase::Response;
        Effect::Rejected { request, reason }
    }
    fn reply(&mut self, request: RequestId, changed: bool) -> Effect {
        self.tickets.get_mut(&request).unwrap().phase = Phase::Response;
        Effect::Reply { request, changed }
    }
    pub(super) fn command_id(&mut self) -> Option<CommitId> {
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
            target: PublicationTarget::Client(request),
        });
        self.awaiting = true;
        self.tickets.get_mut(&request).unwrap().phase = Phase::Started;
        vec![Effect::Persist {
            id,
            candidate,
            document,
        }]
    }
    pub(super) fn drain(&mut self) -> Vec<Effect> {
        // Native completions can immediately dispatch another step. The outer drain owns
        // the bounded loop so chains of saturated/no-op adjustments do not recurse.
        if self.draining {
            return vec![];
        }
        self.draining = true;
        let mut effects = vec![];
        while self.pending.is_none()
            && self.collection.is_none()
            && self.state == StorageState::Ready
        {
            let Some((request, command)) = self.queue.pop_front() else {
                // Client edits already queued have priority. Native jobs use the same
                // publication lane and evaluate their delta against the published value
                // at this turn, never the value captured at admission.
                if let Some(job) = self.brightness_queue.pop_front() {
                    effects.extend(self.persist_brightness(job));
                    continue;
                }
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
                        target: PublicationTarget::Client(request),
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
        self.draining = false;
        effects
    }
    /// Starts the single reaffirmation of a commit whose durable outcome is unknown
    /// (`StorageState::Uncertain`). Returns the `Effect::Reconcile` to execute, or nothing
    /// when storage is not uncertain or a commit is already in flight (and moves to
    /// `RecoveryRequired` if no commit id is left). The runtime calls it once per
    /// incident; without it the state stays `Uncertain` and every new request is
    /// rejected as `StorageUncertain`. Its result arrives through `complete`.
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
        let target = self.pending.as_ref().unwrap().target;
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
            let mut e = self.invalidate_input();
            if let PublicationTarget::Client(request) = target {
                e.push(Effect::Unknown { request });
            }
            while let Some((request, _)) = self.queue.pop_front() {
                e.push(self.reject(request, Rejection::StorageUncertain));
            }
            e.extend(self.fail_queued_brightness());
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
            let mut e = self.publication_finished(target, false, Err(reason));
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
            e.extend(self.publication_finished(target, changed, Ok(())));
            self.refresh_views();
            e.extend(self.drain());
            return e;
        }
        vec![]
    }
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
        let PublicationTarget::Client(request) = pending.target else {
            unreachable!("only client history operations read revisions")
        };
        let candidate = data.ok().and_then(|data| {
            self.published
                .configuration()
                .prepare_restore(self.published.registry(), Origin::Editor, true, data)
                .ok()
        });
        let mut e = match candidate {
            Some(candidate) => self.persist(request, candidate, pending.cursor, false),
            None => vec![self.reject(request, Rejection::Persistence)],
        };
        e.extend(self.drain());
        e
    }
    /// Correlation only for the current pending persistence/history operation.
    pub fn commit_request(&self, id: CommitId) -> Option<RequestId> {
        self.pending
            .as_ref()
            .filter(|pending| pending.id == id)
            .and_then(|pending| match pending.target {
                PublicationTarget::Client(request) => Some(request),
                PublicationTarget::Brightness(_) => None,
            })
    }
    fn publication_finished(
        &mut self,
        target: PublicationTarget,
        changed: bool,
        result: Result<(), Rejection>,
    ) -> Vec<Effect> {
        match target {
            PublicationTarget::Client(request) => vec![match result {
                Ok(()) => self.reply(request, changed),
                Err(reason) => self.reject(request, reason),
            }],
            PublicationTarget::Brightness(id) => self
                .job_complete(
                    id,
                    if result.is_ok() {
                        Completion::Succeeded
                    } else {
                        Completion::Failed(rad_core::execution::Failure::ActionFailed)
                    },
                )
                .expect("native publication retains its dispatch"),
        }
    }
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
            e.extend(self.invalidate_input());
            while let Some((request, _)) = self.queue.pop_front() {
                e.push(self.reject(request, Rejection::StorageUncertain));
            }
            e.extend(self.fail_queued_brightness());
        } else {
            self.collection = None;
            self.state = StorageState::Ready;
            e.extend(self.drain());
        }
        e
    }
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
