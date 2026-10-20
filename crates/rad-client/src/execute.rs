use crate::{
    cli::Command,
    connection::{Client, Failure},
    output,
    response::{Response, Status},
};
use serde_json::json;
use std::time::{Duration, Instant};
pub fn command(client: &mut Client, command: &Command) -> Result<Response, Failure> {
    let operation = command.request.operation();
    let execution_id = match &command.request {
        rad_api::Request::Result { execution_id } => Some(execution_id.as_str()),
        _ => None,
    };
    let first_deadline = if operation == rad_api::Operation::Result {
        command
            .wait_ms
            .map(|ms| Instant::now() + Duration::from_millis(ms))
    } else {
        None
    };
    let first = match first_deadline {
        Some(deadline) => client.call_until(command.request.clone(), deadline),
        None => client.call(command.request.clone()),
    };
    let response = match first {
        Ok(response) => response,
        Err(error)
            if first_deadline.is_some_and(|d| Instant::now() >= d)
                && error.code == "deadline_exceeded" =>
        {
            return Ok(timeout(&output::failure(error), execution_id.unwrap_or("")));
        }
        Err(error) => return Err(error),
    };
    if response.status == Status::Error {
        return Ok(response);
    }
    if operation == rad_api::Operation::Get {
        let rad_api::Request::Get { id } = &command.request else {
            unreachable!()
        };
        return crate::query::assemble(response, id, |request| client.call(request));
    }
    if operation == rad_api::Operation::Result {
        terminal(&response, execution_id.ok_or_else(|| invalid(&response))?)?;
    }
    let Some(wait_ms) = command.wait_ms else {
        return Ok(response);
    };
    let execution = if operation == rad_api::Operation::Run {
        match &response.data {
            rad_api::ResponseData::Admitted(value) => Some(value.execution_id.as_str()),
            _ => None,
        }
    } else {
        execution_id
    }
    .ok_or_else(|| invalid(&response))?
    .to_owned();
    let deadline =
        first_deadline.unwrap_or_else(|| Instant::now() + Duration::from_millis(wait_ms));
    let mut response = response;
    if operation == rad_api::Operation::Result {
        if terminal(&response, &execution)? {
            return Ok(response);
        }
        pause(deadline);
    }
    loop {
        if Instant::now() >= deadline {
            return Ok(timeout(&response, &execution));
        }
        response = match client.call_until(
            rad_api::Request::Result {
                execution_id: execution.clone(),
            },
            deadline,
        ) {
            Ok(response) => response,
            Err(error) if Instant::now() >= deadline && error.code == "deadline_exceeded" => {
                return Ok(timeout(&output::failure(error), &execution));
            }
            Err(error) => {
                let mut response = output::failure(error);
                response.error.as_mut().unwrap().details = json!({"execution_id":execution});
                return Ok(response);
            }
        };
        if response.status == Status::Error {
            response.error.as_mut().unwrap().details["execution_id"] = json!(execution);
            return Ok(response);
        }
        match terminal(&response, &execution) {
            Ok(true) => return Ok(response),
            Ok(false) => {}
            Err(error) => {
                let mut response = output::failure(error);
                response.error.as_mut().unwrap().details = json!({"execution_id":execution});
                return Ok(response);
            }
        }
        pause(deadline);
    }
}
fn terminal(response: &Response, execution: &str) -> Result<bool, Failure> {
    let rad_api::ResponseData::Execution(result) = &response.data else {
        return Err(invalid(response));
    };
    if response.status != Status::Ok || result.execution_id != execution {
        return Err(invalid(response));
    }
    Ok(result.state != rad_api::ExecutionState::Running)
}

fn invalid(response: &Response) -> Failure {
    Failure {
        code: "invalid_request",
        id: response.id.as_deref().and_then(|id| id.parse().ok()),
        session_id: response.session_id.clone(),
        outcome: None,
    }
}
fn pause(deadline: Instant) {
    std::thread::sleep(
        deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(250)),
    );
}
fn timeout(last: &Response, execution: &str) -> Response {
    let mut response = output::failure(Failure {
        code: "wait_timeout",
        id: last.id.as_deref().and_then(|id| id.parse().ok()),
        session_id: last.session_id.clone(),
        outcome: None,
    });
    response.error.as_mut().unwrap().details = json!({"execution_id":execution});
    response.data = rad_api::ResponseData::None;
    response
}
