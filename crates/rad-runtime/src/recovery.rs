//! Recovery mode (ADR 0011): the runtime that cannot open the configuration keeps the
//! instance lock and the sockets, takes neither the deck nor plugins, and serves only
//! `hello`, `recovery` and `recovery_reset`. The Editor never writes the configuration:
//! «Empezar de nuevo» is done here, and the same process then becomes a [`Server`].
//!
//! The damaged file is never opened for writing from this mode; `peek_counters` reads it
//! once, `immutable`. A new operation (restoring a copy) is one more arm in `answer`, one
//! more [`RecoveryAction`] and a different `create` for [`bootstrap::replace_and_open`].
use crate::{
    bootstrap::{self, Bootstrap, Diagnosis, ReplaceFailed},
    events::{ConnectionFailures, Events},
    exit,
    server::{RuntimeError, Server},
};
use rad_api::{
    EventCode, RecoveryAction, RecoveryCause, RecoveryDetail, RecoveryView,
    response::{Error, Outcome, Response, Status},
};
use rad_core::configuration::Origin;
use rad_launcher::monotonic_ms;
use rad_protocol::{recovery_hello, recovery_operations};
use rad_store::PeekedCounters;
use rad_transport::{Incoming, Transport, TransportError};
use serde_json::json;
use std::{
    num::NonZeroU128,
    path::{Path, PathBuf},
    time::Duration,
};

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
        }
    }
    pub fn cause(&self) -> RecoveryCause {
        self.diagnosis.cause
    }
    /// What the Editor may ask for; «Ver detalles» is always there.
    fn actions(&self) -> Vec<RecoveryAction> {
        match self.diagnosis.cause {
            RecoveryCause::Unreadable => vec![RecoveryAction::Reset, RecoveryAction::Details],
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
    pub fn shutdown_complete(&self) -> bool {
        self.shutting_down
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
        // After a restart the rest of the queue belongs to the Server.
        while self.restarted.is_none() {
            let Some(incoming) = self.transport.take_next() else {
                break;
            };
            let response = self.answer(&incoming)?;
            self.respond(&incoming, response)?;
        }
        Ok(())
    }
    /// A restart is done; [`Recovery::into_server`] continues in the same process.
    pub fn reset_done(&self) -> bool {
        self.restarted.is_some()
    }
    /// Consumes the record of the previous exit now: until here the data folder was not
    /// known to be safe.
    pub fn into_server(self) -> Result<Server, RuntimeError> {
        let boot = self.restarted.ok_or(RuntimeError::Protocol)?;
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

    fn answer(&mut self, incoming: &Incoming) -> Result<Response, RuntimeError> {
        use rad_api::{ErrorCode as Code, Operation, Request};
        let request = &incoming.request;
        let id = incoming.key.id();
        let mutation = Operation::parse(request.operation()).is_some_and(Operation::is_mutation);
        let refuse = |code, mutation| self.refusal(id, code, mutation);
        if self.shutting_down {
            return Ok(refuse(Code::RuntimeUnavailable, mutation));
        }
        let Ok(parsed) = Request::parse(
            request.operation(),
            serde_json::Value::Object(request.args().clone()),
        ) else {
            return Ok(refuse(Code::InvalidRequest, mutation));
        };
        let origin = match incoming.origin {
            rad_transport::Origin::Editor => Origin::Editor,
            rad_transport::Origin::Automation => Origin::Automation,
        };
        Ok(match parsed {
            Request::Hello {} => self.reply(
                id,
                Status::Ok,
                rad_api::ResponseData::Hello(recovery_hello(self.session, origin)),
            ),
            Request::Recovery {} | Request::RecoveryReset { .. }
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
            _ => refuse(Code::RecoveryRequired, mutation),
        })
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
            Err(ReplaceFailed::NotApplied) => {
                Ok(self.refusal(id, rad_api::ErrorCode::StorageError, true))
            }
            // The new base may be in place: the next start opens whatever is there.
            Err(ReplaceFailed::Unfinished) => Err(RuntimeError::Storage),
        }
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
    fn respond(&mut self, incoming: &Incoming, response: Response) -> Result<(), RuntimeError> {
        let bytes = serde_json::to_vec(&response).map_err(|_| RuntimeError::Protocol)?;
        match self.transport.finish(incoming.key, &bytes) {
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
