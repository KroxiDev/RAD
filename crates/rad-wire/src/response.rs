use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Admitted,
    Applied,
    Error,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    NotApplied,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Error {
    pub code: String,
    pub message: String,
    pub details: Value,
    pub outcome: Option<Outcome>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub v: u8,
    pub id: Option<String>,
    pub session_id: Option<String>,
    pub status: Status,
    pub data: Value,
    pub error: Option<Error>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidResponse;
impl Response {
    pub fn decode(bytes: &[u8], id: u64, session: Option<&str>) -> Result<Self, InvalidResponse> {
        let value = crate::parse_json(bytes).map_err(|_| InvalidResponse)?;
        let object = value.as_object().ok_or(InvalidResponse)?;
        if object.len() != 6
            || !["v", "id", "session_id", "status", "data", "error"]
                .iter()
                .all(|k| object.contains_key(*k))
        {
            return Err(InvalidResponse);
        }
        if let Some(error) = object["error"].as_object()
            && (error.len() != 4
                || !["code", "message", "details", "outcome"]
                    .iter()
                    .all(|k| error.contains_key(*k)))
        {
            return Err(InvalidResponse);
        }
        let response: Self = serde_json::from_value(value).map_err(|_| InvalidResponse)?;
        if response.v != 1
            || id == 0
            || response.id.as_deref() != Some(id.to_string().as_str())
            || session.is_some_and(|s| response.session_id.as_deref() != Some(s))
        {
            return Err(InvalidResponse);
        }
        let token = response.session_id.as_deref().ok_or(InvalidResponse)?;
        if token.len() != 32
            || !token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || token.bytes().all(|b| b == b'0')
        {
            return Err(InvalidResponse);
        }
        match (&response.status, &response.error) {
            (Status::Error, Some(error))
                if response.data.is_null()
                    && error.details.is_object()
                    && error.message.len() <= 256
                    && valid_code(&error.code) => {}
            (Status::Ok | Status::Admitted | Status::Applied, None)
                if response.data.is_object() => {}
            _ => return Err(InvalidResponse),
        }
        match response.status {
            Status::Applied
                if !response.data["changed"].is_boolean()
                    || !response.data["revision"].as_str().is_some_and(counter) =>
            {
                return Err(InvalidResponse);
            }
            Status::Admitted => {
                let execution = response.data["execution_id"]
                    .as_str()
                    .ok_or(InvalidResponse)?;
                let (session, sequence) = execution.split_once(':').ok_or(InvalidResponse)?;
                if session != token || sequence == "0" || !counter(sequence) {
                    return Err(InvalidResponse);
                }
            }
            _ => {}
        }
        Ok(response)
    }
}

fn valid_code(code: &str) -> bool {
    matches!(
        code,
        "invalid_request"
            | "unsupported_version"
            | "not_found"
            | "permission_denied"
            | "operation_not_allowed"
            | "invalid_configuration"
            | "identity_collision"
            | "busy"
            | "resource_exhausted"
            | "executor_unavailable"
            | "service_unavailable"
            | "action_failed"
            | "action_timeout"
            | "storage_error"
            | "storage_uncertain"
            | "recovery_required"
            | "cursor_expired"
            | "result_expired"
            | "deadline_exceeded"
            | "runtime_unavailable"
            | "wait_timeout"
            | "plugin_limit"
            | "options_timeout"
            | "cancelled"
            | "plugin_in_use"
            | "plugin_residual"
            | "publication_pending"
            | "incompatible_plugin"
            | "invalid_source"
            | "invalid_package"
            | "network"
            | "deadline"
            | "io"
            | "confirmation_required"
    )
}

fn counter(value: &str) -> bool {
    !value.is_empty()
        && (value == "0" || !value.starts_with('0'))
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u64>().is_ok()
}
