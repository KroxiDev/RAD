//! Build and deliver client responses while containing invalid adapter payloads.
use super::{
    Effect, Error, EventCode, IncomingContext, Outcome, RequestKey, Response, RuntimeError, Server,
    Status, TransportError, Value, json,
};

impl Server {
    /// Answers a started request. A response the transport refuses (empty or too large)
    /// becomes a generic error for that client; it never ends the runtime.
    pub(super) fn respond(
        &mut self,
        key: RequestKey,
        response: Response,
    ) -> Result<(), RuntimeError> {
        if let Ok(bytes) = serde_json::to_vec(&response) {
            match self.transport.finish(key, &bytes) {
                Ok(()) => return Ok(()),
                Err(TransportError::InvalidResponse) => {}
                // Already answered or gone: nothing is left to answer.
                Err(TransportError::UnknownRequest) => {
                    self.events.record(EventCode::ResponseUnknownRequest, None);
                    return Ok(());
                }
                Err(_) => return Err(RuntimeError::Transport),
            }
        }
        self.events.record(EventCode::ResponseRefused, None);
        let generic = self.generic_error(&response);
        let bytes = serde_json::to_vec(&generic).map_err(|_| RuntimeError::Protocol)?;
        if self.transport.finish(key, &bytes).is_err() {
            self.events.record(EventCode::ResponseDropped, None);
        }
        Ok(())
    }
    /// A change that was answered is not "not applied", whatever its answer looked like.
    fn generic_error(&self, refused: &Response) -> Response {
        let outcome = match refused.status {
            Status::Applied | Status::Admitted => Some(Outcome::Unknown),
            Status::Error => refused.error.as_ref().and_then(|error| error.outcome),
            Status::Ok => None,
        };
        self.failure(
            refused.id.clone(),
            rad_api::ErrorCode::ResourceExhausted,
            outcome,
        )
    }
    /// The answer to a request that Authority resolved. When none can be built (the view
    /// it would carry is gone), the client still gets a generic error, and what the effect
    /// says happened is not denied: only a rejection is "not applied".
    pub(super) fn response_or_generic(
        &mut self,
        found: Option<Response>,
        context: IncomingContext,
        effect: &Effect,
    ) -> Response {
        if let Some(response) = found {
            return response;
        }
        self.events.record(EventCode::ResponseUnbuildable, None);
        let outcome = match effect {
            Effect::Rejected { .. } => Outcome::NotApplied,
            _ => Outcome::Unknown,
        };
        self.failure(
            Some(context.request_id.to_string()),
            rad_api::ErrorCode::ResourceExhausted,
            Some(outcome),
        )
    }
    /// What a plugin answered, as the client's response. Data that does not decode is that
    /// plugin's failure and that request's error, never the runtime's: a change that was
    /// already submitted stays "unknown".
    pub(super) fn plugin_response(
        &mut self,
        context: IncomingContext,
        operation: rad_api::Operation,
        mutation: bool,
        data: Value,
    ) -> Response {
        match rad_api::ResponseData::decode(operation, Status::Ok, data) {
            Ok(data) => self.ok_response(context, data),
            Err(_) => {
                self.events.record(EventCode::PluginResponseInvalid, None);
                self.failure(
                    Some(context.request_id.to_string()),
                    rad_api::ErrorCode::ExecutorUnavailable,
                    mutation.then_some(Outcome::Unknown),
                )
            }
        }
    }
    /// The launcher's catalog page as the client's response; a page that does not decode
    /// is an error for this request only.
    pub(super) fn launcher_response(&mut self, context: IncomingContext, data: Value) -> Response {
        match serde_json::from_value(data) {
            Ok(page) => self.ok_response(context, rad_api::ResponseData::Launcher(page)),
            Err(_) => {
                self.events.record(EventCode::CatalogResponseInvalid, None);
                self.failure(
                    Some(context.request_id.to_string()),
                    rad_api::ErrorCode::ServiceUnavailable,
                    None,
                )
            }
        }
    }
    fn ok_response(&self, context: IncomingContext, data: rad_api::ResponseData) -> Response {
        Response {
            v: 1,
            id: Some(context.request_id.to_string()),
            session_id: Some(format!("{:032x}", self.session)),
            status: Status::Ok,
            data,
            error: None,
        }
    }
    fn failure(
        &self,
        id: Option<String>,
        code: rad_api::ErrorCode,
        outcome: Option<Outcome>,
    ) -> Response {
        Response {
            v: 1,
            id,
            session_id: Some(format!("{:032x}", self.session)),
            status: Status::Error,
            data: rad_api::ResponseData::None,
            error: Some(Error {
                code,
                message: "Request rejected".into(),
                details: json!({}),
                outcome,
            }),
        }
    }
    pub(super) fn error(&self, context: IncomingContext, code: &str, mutation: bool) -> Response {
        self.error_with_details(context, code, json!({}), mutation)
    }
    pub(super) fn error_with_details(
        &self,
        context: IncomingContext,
        code: &str,
        details: Value,
        mutation: bool,
    ) -> Response {
        Response {
            v: 1,
            id: Some(context.request_id.to_string()),
            session_id: Some(format!("{:032x}", self.session)),
            status: Status::Error,
            data: rad_api::ResponseData::None,
            error: Some(Error {
                code: rad_api::ErrorCode::parse(code).expect("defined response code"),
                message: "Request rejected".into(),
                details,
                outcome: mutation.then_some(Outcome::NotApplied),
            }),
        }
    }
}
