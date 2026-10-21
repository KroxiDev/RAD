//! Recovery mode (ADR 0011): the runtime that cannot open the configuration keeps the
//! instance lock and the sockets, takes neither the deck nor plugins, and serves only
//! `hello`, `recovery` and `recovery_reset`, plus the upload of a copy and its
//! confirmation. The Editor never writes the configuration: «Empezar de nuevo» and
//! «Restaurar una copia» are done here, and the same process then becomes a [`Server`].
//!
//! The damaged file is never opened for writing from this mode; `peek_counters` reads it
//! once, `immutable`. Uploading and verifying a copy is blocking work, so it runs on a
//! [`StorageWorker`] that has transfers but no Store, one operation at a time: the next
//! request waits in the transport's queue until the worker answers.
use crate::{
    bootstrap::{self, Bootstrap, Diagnosis, ReplaceFailed, RestoreRefused},
    events::{ConnectionFailures, Events},
    exit,
    server::{RuntimeError, Server},
    storage::{Job, JobId, ResultValue, StorageWorker},
    transfers::{self, Context, Kind, Operation, Output, Transfers},
};
use rad_api::{
    EventCode, RecoveryAction, RecoveryCause, RecoveryDetail, RecoveryView,
    response::{Error, Outcome, Response, Status},
};
use rad_core::configuration::Origin;
use rad_launcher::monotonic_ms;
use rad_protocol::{External, TransferKind, parse_transfer, recovery_hello, recovery_operations};
use rad_store::PeekedCounters;
use rad_store::ReplaceError;
use rad_transport::{Incoming, RequestKey, Transport, TransportError};
use serde_json::json;
use std::{
    num::NonZeroU128,
    path::{Path, PathBuf},
    time::Duration,
};

/// The one operation the worker is running, and what to do with its result.
struct Flight {
    job: JobId,
    key: RequestKey,
    stage: Stage,
}
enum Stage {
    /// The output is the answer.
    Reply,
    /// `restore_commit`: the copy is verified; next the new base is built from it.
    Verify {
        connection: u64,
        transfer_id: String,
    },
    /// The new base is being put in place of the damaged one.
    Replace,
    /// The copy is let go, then the request is refused with this code.
    Refuse(rad_api::ErrorCode),
}

pub struct Recovery {
    transport: Transport,
    session: NonZeroU128,
    data: PathBuf,
    diagnosis: Diagnosis,
    peeked: Option<PeekedCounters>,
    events: Events,
    connection_failures: ConnectionFailures,
    shutting_down: bool,
    restarted: Option<Bootstrap>,
    /// Started with the first upload; nothing is created in the data folder before.
    worker: Option<StorageWorker>,
    flight: Option<Flight>,
}
impl Recovery {
    pub(crate) fn new(
        transport: Transport,
        session: NonZeroU128,
        data: &Path,
        diagnosis: Diagnosis,
        peeked: Option<PeekedCounters>,
    ) -> Self {
        let mut events = Events::new();
        events.record(EventCode::Started, None);
        events.record(EventCode::RecoveryMode, Some(diagnosis.cause.as_str()));
        Self {
            transport,
            session,
            data: data.to_path_buf(),
            diagnosis,
            peeked,
            events,
            connection_failures: ConnectionFailures::default(),
            shutting_down: false,
            restarted: None,
            worker: None,
            flight: None,
        }
    }
    pub fn cause(&self) -> RecoveryCause {
        self.diagnosis.cause
    }
    /// What the Editor may ask for; «Ver detalles» is always there.
    fn actions(&self) -> Vec<RecoveryAction> {
        match self.diagnosis.cause {
            RecoveryCause::Unreadable => vec![
                RecoveryAction::Restore,
                RecoveryAction::Reset,
                RecoveryAction::Details,
            ],
            RecoveryCause::NewerVersion | RecoveryCause::UnsafeFolder => {
                vec![RecoveryAction::Details]
            }
        }
    }
    fn view(&self) -> RecoveryView {
        RecoveryView {
            cause: self.diagnosis.cause,
            actions: self.actions(),
            detail: RecoveryDetail {
                code: self.diagnosis.code,
                counters_read: self.peeked.is_some(),
            },
        }
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
    pub fn request_shutdown(&mut self) {
        if !self.shutting_down {
            self.shutting_down = true;
            self.events.record(EventCode::ShutdownRequested, None);
        }
    }
    /// An operation of the worker is let finish: it may be replacing the base.
    pub fn shutdown_complete(&self) -> bool {
        self.shutting_down && self.flight.is_none()
    }
    /// The transport already clips the wait to its own deadlines.
    pub fn wait(&mut self, maximum: Duration) -> Result<(), RuntimeError> {
        self.transport
            .wait_readiness(maximum)
            .map_err(|_| RuntimeError::Transport)
    }
    pub fn turn(&mut self) -> Result<(), RuntimeError> {
        let now = monotonic_ms().map_err(|_| RuntimeError::Clock)?;
        self.transport
            .drive(now)
            .map_err(|_| RuntimeError::Transport)?;
        let failures = self.transport.take_connection_failures();
        if let Some(count) = self.connection_failures.note(failures, now) {
            self.events
                .record(EventCode::ConnectionFailed, Some(&count.to_string()));
        }
        // After a restart the rest of the queue belongs to the Server; while the worker
        // is busy it waits there.
        while self.restarted.is_none() {
            self.poll_flight()?;
            if self.flight.is_some() {
                break;
            }
            let Some(incoming) = self.transport.take_next() else {
                break;
            };
            if let Some(response) = self.answer(&incoming)? {
                self.respond(incoming.key, response)?;
            }
        }
        Ok(())
    }
    /// A restart is done; [`Recovery::into_server`] continues in the same process.
    pub fn reset_done(&self) -> bool {
        self.restarted.is_some()
    }
    /// Consumes the record of the previous exit now: until here the data folder was not
    /// known to be safe.
    pub fn into_server(mut self) -> Result<Server, RuntimeError> {
        let boot = self.restarted.ok_or(RuntimeError::Protocol)?;
        // The server recovers the transfer folder: no thread of this mode may keep it.
        if let Some(worker) = self.worker.take() {
            worker.stop();
        }
        let previous_exit = exit::take_previous(&self.data);
        Server::from_parts(
            self.transport,
            boot,
            &self.data,
            self.session,
            previous_exit,
            self.events,
        )
    }

    /// The answer, or `None` when the worker will give it (see [`Recovery::poll_flight`]).
    fn answer(&mut self, incoming: &Incoming) -> Result<Option<Response>, RuntimeError> {
        use rad_api::{ErrorCode as Code, Operation, Request};
        let request = &incoming.request;
        let id = incoming.key.id();
        let mutation = Operation::parse(request.operation()).is_some_and(Operation::is_mutation);
        let refuse = |code, mutation| self.refusal(id, code, mutation);
        if self.shutting_down {
            return Ok(Some(refuse(Code::RuntimeUnavailable, mutation)));
        }
        let Ok(parsed) = Request::parse(
            request.operation(),
            serde_json::Value::Object(request.args().clone()),
        ) else {
            return Ok(Some(refuse(Code::InvalidRequest, mutation)));
        };
        let origin = match incoming.origin {
            rad_transport::Origin::Editor => Origin::Editor,
            rad_transport::Origin::Automation => Origin::Automation,
        };
        Ok(Some(match parsed {
            Request::Hello {} => self.reply(
                id,
                Status::Ok,
                rad_api::ResponseData::Hello(recovery_hello(self.session, origin)),
            ),
            Request::Recovery {}
            | Request::RecoveryReset { .. }
            | Request::TransferBegin { .. }
            | Request::TransferWrite { .. }
            | Request::TransferFinish { .. }
            | Request::TransferDrop { .. }
            | Request::RestoreCommit { .. }
                if !recovery_operations(origin).contains(&parsed.operation()) =>
            {
                refuse(Code::PermissionDenied, mutation)
            }
            Request::Recovery {} => {
                self.reply(id, Status::Ok, rad_api::ResponseData::Recovery(self.view()))
            }
            Request::RecoveryReset { confirm } if !confirm => {
                refuse(Code::ConfirmationRequired, true)
            }
            Request::RecoveryReset { .. } if !self.actions().contains(&RecoveryAction::Reset) => {
                refuse(Code::OperationNotAllowed, true)
            }
            Request::RecoveryReset { .. } => self.start_again(id)?,
            Request::RestoreCommit {
                confirm_replace: false,
                ..
            } => refuse(Code::ConfirmationRequired, true),
            Request::TransferBegin { .. }
            | Request::TransferWrite { .. }
            | Request::TransferFinish { .. }
            | Request::TransferDrop { .. }
            | Request::RestoreCommit { .. }
                if !self.actions().contains(&RecoveryAction::Restore) =>
            {
                refuse(Code::OperationNotAllowed, true)
            }
            Request::TransferBegin { kind, .. } if kind != rad_api::TransferKind::Restore => {
                refuse(Code::OperationNotAllowed, true)
            }
            Request::TransferBegin { .. }
            | Request::TransferWrite { .. }
            | Request::TransferFinish { .. }
            | Request::TransferDrop { .. }
            | Request::RestoreCommit { .. } => {
                return match parse_transfer(origin, &parsed) {
                    Ok(external) => self.start_transfer(incoming.key, external),
                    Err(code) => Ok(Some(self.refusal(
                        id,
                        Code::parse(code).unwrap_or(Code::InvalidRequest),
                        true,
                    ))),
                };
            }
            _ => refuse(Code::RecoveryRequired, mutation),
        }))
    }
    /// Gives the operation to the worker, which is started with the first one.
    fn start_transfer(
        &mut self,
        key: RequestKey,
        external: External,
    ) -> Result<Option<Response>, RuntimeError> {
        use rad_api::ErrorCode as Code;
        let (operation, stage) = match external {
            External::TransferBegin {
                kind: TransferKind::Restore,
                size,
            } => (
                Operation::Begin {
                    kind: Kind::Restore,
                    size,
                    export_revision: None,
                    export_snapshot: None,
                },
                Stage::Reply,
            ),
            External::TransferWrite {
                transfer_id,
                offset,
                bytes,
            } => (
                Operation::Write {
                    transfer_id,
                    offset,
                    bytes,
                },
                Stage::Reply,
            ),
            External::TransferFinish { transfer_id } => {
                (Operation::Finish { transfer_id }, Stage::Reply)
            }
            External::TransferDrop { transfer_id } => {
                (Operation::Drop { transfer_id }, Stage::Reply)
            }
            External::RestoreCommit {
                transfer_id,
                confirmed,
            } => (
                Operation::RestoreData {
                    transfer_id: transfer_id.clone(),
                    confirmed,
                },
                Stage::Verify {
                    connection: key.connection(),
                    transfer_id,
                },
            ),
            _ => return Ok(Some(self.refusal(key.id(), Code::InvalidRequest, true))),
        };
        let context = Context {
            connection: key.connection(),
            origin: Origin::Editor,
            now_ms: monotonic_ms().map_err(|_| RuntimeError::Clock)?,
        };
        self.submit(key, Job::Transfer { context, operation }, stage)
    }
    /// Runs `job`; the answer to `key` comes when it ends. A worker that cannot be had or
    /// is not free answers now, with nothing applied.
    fn submit(
        &mut self,
        key: RequestKey,
        job: Job,
        stage: Stage,
    ) -> Result<Option<Response>, RuntimeError> {
        use rad_api::ErrorCode as Code;
        let worker = match self.worker.as_mut() {
            Some(worker) => worker,
            None => {
                let started = Transfers::open(&self.data.join("transfers"), self.session.get())
                    .ok()
                    .zip(self.transport.worker_waker().ok())
                    .and_then(|(transfers, waker)| {
                        StorageWorker::transfers_only(transfers, std::sync::Arc::new(waker)).ok()
                    });
                match started {
                    Some(worker) => self.worker.insert(worker),
                    None => return Ok(Some(self.refusal(key.id(), Code::StorageError, true))),
                }
            }
        };
        match worker.submit(job) {
            Ok(job) => {
                self.flight = Some(Flight { job, key, stage });
                Ok(None)
            }
            Err(_) => Ok(Some(self.refusal(key.id(), Code::ResourceExhausted, true))),
        }
    }
    /// What the worker finished, if it did: the answer, or the next step of a restoration.
    fn poll_flight(&mut self) -> Result<(), RuntimeError> {
        use rad_api::ErrorCode as Code;
        let Some(worker) = self.worker.as_mut() else {
            return Ok(());
        };
        let Some((job, result)) = worker.take().map_err(|_| RuntimeError::Storage)? else {
            return Ok(());
        };
        let flight = self.flight.take().ok_or(RuntimeError::Storage)?;
        if job != flight.job {
            return Err(RuntimeError::Storage);
        }
        let (key, id) = (flight.key, flight.key.id());
        let response = match (flight.stage, result) {
            (Stage::Refuse(code), ResultValue::Released(_)) => Some(self.refusal(id, code, true)),
            (Stage::Replace, ResultValue::Replaced(replaced)) => match replaced {
                Ok(()) => Some(self.restored(id)?),
                Err(ReplaceError::NotApplied(_)) => {
                    Some(self.refusal(id, Code::StorageError, true))
                }
                // The new base is in place or may be: the next start opens what is there.
                Err(ReplaceError::AfterExchange) => return Err(RuntimeError::Storage),
            },
            (_, ResultValue::Transfer(Err(error))) => Some(self.failure(id, error)),
            (
                Stage::Verify {
                    connection,
                    transfer_id,
                },
                ResultValue::Transfer(Ok(Output::RestoreData { data })),
            ) => match bootstrap::restored_candidate(self.peeked, data) {
                Ok(candidate) => {
                    let job = Job::ReplaceFromMedia {
                        path: self.data.join(bootstrap::DATABASE),
                        stamp: exit::now_unix_ms(),
                        candidate,
                        connection,
                        transfer_id,
                    };
                    self.submit(key, job, Stage::Replace)?
                }
                Err(refused) => {
                    let code = match refused {
                        RestoreRefused::Copy => Code::InvalidRequest,
                        RestoreRefused::Unavailable => Code::StorageError,
                    };
                    self.submit(key, Job::ReleaseMedia { transfer_id }, Stage::Refuse(code))?
                }
            },
            (Stage::Reply, ResultValue::Transfer(Ok(output))) => match output.into_data() {
                Some(data) => Some(self.reply(id, Status::Ok, data)),
                None => return Err(RuntimeError::Storage),
            },
            _ => return Err(RuntimeError::Storage),
        };
        if let Some(response) = response {
            self.respond(key, response)?;
        }
        Ok(())
    }
    fn failure(&self, id: u64, error: transfers::Error) -> Response {
        Response {
            error: Some(Error {
                code: rad_api::ErrorCode::parse(error.wire_code()).expect("defined response code"),
                message: "Request rejected".into(),
                details: error.details(),
                outcome: Some(Outcome::NotApplied),
            }),
            ..self.reply(id, Status::Error, rad_api::ResponseData::None)
        }
    }
    /// The new base holds the copy: open it, and go on as the Server.
    fn restored(&mut self, id: u64) -> Result<Response, RuntimeError> {
        let now = monotonic_ms().map_err(|_| RuntimeError::Clock)?;
        let boot = bootstrap::open_replaced(&self.data, self.session, now)
            .map_err(|_| RuntimeError::Storage)?;
        self.events.record(EventCode::RecoveryRestored, None);
        self.become_normal(id, boot)
    }
    fn start_again(&mut self, id: u64) -> Result<Response, RuntimeError> {
        let now = monotonic_ms().map_err(|_| RuntimeError::Clock)?;
        match bootstrap::start_again(
            &self.data,
            self.peeked,
            self.session,
            now,
            exit::now_unix_ms(),
        ) {
            Ok(boot) => {
                self.events.record(EventCode::RecoveryReset, None);
                self.become_normal(id, boot)
            }
            Err(ReplaceFailed::NotApplied) => {
                Ok(self.refusal(id, rad_api::ErrorCode::StorageError, true))
            }
            // The new base may be in place: the next start opens whatever is there.
            Err(ReplaceFailed::Unfinished) => Err(RuntimeError::Storage),
        }
    }
    /// The way out of recovery is done: the answer carries the new configuration's revision.
    fn become_normal(&mut self, id: u64, boot: Bootstrap) -> Result<Response, RuntimeError> {
        let revision = boot
            .authority
            .view_revision(Origin::Editor)
            .ok_or(RuntimeError::Protocol)?
            .value()
            .to_string();
        self.restarted = Some(boot);
        Ok(self.reply(
            id,
            Status::Applied,
            rad_api::ResponseData::Applied(rad_api::Applied {
                changed: true,
                revision,
            }),
        ))
    }
    fn reply(&self, id: u64, status: Status, data: rad_api::ResponseData) -> Response {
        Response {
            v: 1,
            id: Some(id.to_string()),
            session_id: Some(format!("{:032x}", self.session)),
            status,
            data,
            error: None,
        }
    }
    fn refusal(&self, id: u64, code: rad_api::ErrorCode, mutation: bool) -> Response {
        Response {
            error: Some(Error {
                code,
                message: "Request rejected".into(),
                details: json!({}),
                outcome: mutation.then_some(Outcome::NotApplied),
            }),
            ..self.reply(id, Status::Error, rad_api::ResponseData::None)
        }
    }
    fn respond(&mut self, key: RequestKey, response: Response) -> Result<(), RuntimeError> {
        let bytes = serde_json::to_vec(&response).map_err(|_| RuntimeError::Protocol)?;
        match self.transport.finish(key, &bytes) {
            Ok(()) => Ok(()),
            // Answered or gone already: nothing is left to answer.
            Err(TransportError::UnknownRequest) => {
                self.events.record(EventCode::ResponseUnknownRequest, None);
                Ok(())
            }
            Err(_) => Err(RuntimeError::Transport),
        }
    }
}
