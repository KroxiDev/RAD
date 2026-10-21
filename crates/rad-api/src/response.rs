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
    pub code: crate::ErrorCode,
    pub message: String,
    pub details: Value,
    pub outcome: Option<Outcome>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Response {
    pub v: u8,
    pub id: Option<String>,
    pub session_id: Option<String>,
    pub status: Status,
    pub data: crate::ResponseData,
    pub error: Option<Error>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidResponse;
impl Response {
    /// JSON resource bounds and duplicate-key rejection belong to rad-wire;
    /// callers pass its validated value into this I/O-free contract decoder.
    pub fn decode_value(
        value: Value,
        id: u64,
        session: Option<&str>,
        operation: crate::Operation,
    ) -> Result<Self, InvalidResponse> {
        #[derive(Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Envelope {
            v: u8,
            id: Option<String>,
            session_id: Option<String>,
            status: Status,
            data: Value,
            error: Option<Error>,
        }
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
        let raw: Envelope = crate::strict::from_value(value).map_err(|_| InvalidResponse)?;
        if raw.v != crate::VERSION
            || id == 0
            || raw.id.as_deref() != Some(id.to_string().as_str())
            || session.is_some_and(|s| raw.session_id.as_deref() != Some(s))
        {
            return Err(InvalidResponse);
        }
        let token = raw.session_id.as_deref().ok_or(InvalidResponse)?;
        if token.len() != 32
            || !token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || token.bytes().all(|b| b == b'0')
        {
            return Err(InvalidResponse);
        }
        if raw.status != Status::Error && raw.status != operation.success_status() {
            return Err(InvalidResponse);
        }
        if operation.is_mutation()
            && raw
                .error
                .as_ref()
                .is_some_and(|error| error.outcome.is_none())
        {
            return Err(InvalidResponse);
        }
        match (&raw.status, &raw.error) {
            (Status::Error, Some(error))
                if raw.data.is_null()
                    && error.details.is_object()
                    && error.message.len() <= 256 => {}
            (Status::Ok | Status::Admitted | Status::Applied, None) if raw.data.is_object() => {}
            _ => return Err(InvalidResponse),
        }
        let data = crate::ResponseData::decode(operation, raw.status, raw.data)
            .map_err(|_| InvalidResponse)?;
        match &data {
            crate::ResponseData::Applied(applied) if !counter(&applied.revision) => {
                return Err(InvalidResponse);
            }
            crate::ResponseData::Admitted(admitted) => {
                let (session, sequence) = admitted
                    .execution_id
                    .split_once(':')
                    .ok_or(InvalidResponse)?;
                if session != token || sequence == "0" || !counter(sequence) {
                    return Err(InvalidResponse);
                }
            }
            _ => {}
        }
        Ok(Self {
            v: raw.v,
            id: raw.id,
            session_id: raw.session_id,
            status: raw.status,
            data,
            error: raw.error,
        })
    }
}

fn counter(value: &str) -> bool {
    !value.is_empty()
        && (value == "0" || !value.starts_with('0'))
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u64>().is_ok()
}

impl crate::Operation {
    pub const fn success_status(self) -> Status {
        use crate::Operation::*;
        match self {
            Run => crate::response::Status::Admitted,
            Edit | CreateFolder | CreateButton | AddAction | Duplicate | Delete | MoveFolder
            | SetPermissions | Undo | Redo | SetSettings | SetPluginSettings | RestoreCommit
            | RecoveryReset => crate::response::Status::Applied,
            _ => crate::response::Status::Ok,
        }
    }
}
