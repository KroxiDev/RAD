use super::*;
use rad_authority::Visible;
use rad_core::execution::{ExecutionError, ExecutionStatus, Failure, StepStatus};
pub(super) fn result(a: &Authority, c: IncomingContext, text: &str) -> Checked<Value> {
    let (session, counter) = text.split_once(':').ok_or("invalid_request")?;
    if session.len() != 32
        || !session
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid_request");
    };
    let session = u128::from_str_radix(session, 16).map_err(|_| "invalid_request")?;
    let counter = decimal(counter)?;
    if session == 0 || counter == 0 {
        return Err("invalid_request");
    };
    let r = a.result_id(c.origin, session, counter).ok_or("not_found")?;
    let state = match r.status {
        ExecutionStatus::Running => "running",
        ExecutionStatus::Succeeded => "succeeded",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::TimedOut => "timed_out",
    };
    let steps = r
        .steps
        .into_iter()
        .map(|s| match s {
            Visible::Restricted => json!({"visibility":"restricted"}),
            Visible::Visible(s) => {
                let (state, code) = match s.status {
                    StepStatus::Waiting => ("waiting", None),
                    StepStatus::Dispatched => ("dispatched", None),
                    StepStatus::Succeeded => ("succeeded", None),
                    StepStatus::TimedOut => ("timed_out", Some("deadline_exceeded")),
                    StepStatus::NotDispatched => ("not_dispatched", None),
                    StepStatus::Failed(f) => (
                        "failed",
                        Some(match f {
                            Failure::ActionFailed => "action_failed",
                            Failure::ServiceUnavailable => "service_unavailable",
                            Failure::ExecutorUnavailable => "executor_unavailable",
                        }),
                    ),
                };
                json!({"index":s.index,"state":state,"code":code})
            }
        })
        .collect::<Vec<_>>();
    Ok(json!({"execution_id":text,"state":state,"steps":steps}))
}
pub(super) fn error_code(e: ExecutionError) -> &'static str {
    match e {
        ExecutionError::NotFound => "not_found",
        ExecutionError::PermissionDenied => "permission_denied",
        ExecutionError::ExecutorUnavailable => "executor_unavailable",
        ExecutionError::Busy => "busy",
        ExecutionError::Capacity | ExecutionError::CounterExhausted => "resource_exhausted",
        _ => "invalid_request",
    }
}
