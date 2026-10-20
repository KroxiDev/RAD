use crate::{
    cli::Command,
    connection::{Client, Failure},
    output,
    response::{Response, Status},
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
pub fn command(client: &mut Client, command: &Command) -> Result<Response, Failure> {
    let first_deadline = if command.operation == "result" {
        command
            .wait_ms
            .map(|ms| Instant::now() + Duration::from_millis(ms))
    } else {
        None
    };
    let first = match first_deadline {
        Some(deadline) => {
            client.request_until(&command.operation, command.arguments.clone(), deadline)
        }
        None => client.request(&command.operation, command.arguments.clone()),
    };
    let response = match first {
        Ok(response) => response,
        Err(error)
            if first_deadline.is_some_and(|d| Instant::now() >= d)
                && error.code == "deadline_exceeded" =>
        {
            return Ok(timeout(
                &output::failure(error),
                command.arguments["execution_id"].as_str().unwrap_or(""),
            ));
        }
        Err(error) => return Err(error),
    };
    if response.status == Status::Error {
        return Ok(response);
    }
    if command.operation == "get" {
        let id = command.arguments["id"]
            .as_str()
            .ok_or_else(|| invalid(&response))?;
        return crate::query::assemble(response, id, |op, args| client.request(op, args));
    }
    if command.operation == "result" {
        terminal(
            &response,
            command.arguments["execution_id"]
                .as_str()
                .ok_or_else(|| invalid(&response))?,
        )?;
    }
    let Some(wait_ms) = command.wait_ms else {
        return Ok(response);
    };
    let execution = if command.operation == "run" {
        response.data["execution_id"].as_str()
    } else {
        command.arguments["execution_id"].as_str()
    }
    .ok_or_else(|| invalid(&response))?
    .to_owned();
    let deadline =
        first_deadline.unwrap_or_else(|| Instant::now() + Duration::from_millis(wait_ms));
    let mut response = response;
    if command.operation == "result" {
        if terminal(&response, &execution)? {
            return Ok(response);
        }
        pause(deadline);
    }
    loop {
        if Instant::now() >= deadline {
            return Ok(timeout(&response, &execution));
        }
        response = match client.request_until("result", json!({"execution_id":execution}), deadline)
        {
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
    if response.status != Status::Ok
        || response.data["execution_id"] != execution
        || !response.data["steps"].is_array()
    {
        return Err(invalid(response));
    }
    match response.data["state"].as_str() {
        Some("running") => Ok(false),
        Some("succeeded" | "failed" | "timed_out") => Ok(true),
        _ => Err(invalid(response)),
    }
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
    response.data = Value::Null;
    response
}
