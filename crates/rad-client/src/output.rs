use crate::{
    connection::Failure,
    response::{Error, Outcome, Response, Status},
};
use serde_json::json;
use std::io::{self, Write};
pub fn failure(error: Failure) -> Response {
    Response {
        v: 1,
        id: error.id.map(|id| id.to_string()),
        session_id: error.session_id,
        status: Status::Error,
        data: rad_api::ResponseData::None,
        error: Some(Error {
            code: rad_api::ErrorCode::parse(error.code).expect("defined client error"),
            message: match error.code {
                "invalid_request" => "Invalid arguments or protocol",
                "wait_timeout" => "Wait timed out; execution was not cancelled",
                "resource_exhausted" => "Response capacity exceeded",
                "cursor_expired" => "Visible configuration changed; repeat the query",
                "deadline_exceeded" => "Response deadline exceeded",
                _ => "Runtime unavailable",
            }
            .into(),
            details: json!({}),
            outcome: error.outcome,
        }),
    }
}
pub fn exit_code(response: &Response) -> u8 {
    if let Some(error) = &response.error {
        if error.outcome == Some(Outcome::Unknown) {
            return 8;
        }
        error.code.exit_code() as u8
    } else if matches!(&response.data, rad_api::ResponseData::Execution(result) if matches!(result.state,rad_api::ExecutionState::Failed | rad_api::ExecutionState::TimedOut))
    {
        6
    } else {
        0
    }
}
pub fn write(response: &Response, structured: bool, mut to: impl Write) -> io::Result<()> {
    if structured {
        let mut value = serde_json::to_value(response)?;
        value["cli_version"] = json!(rad_api::CLI_VERSION);
        serde_json::to_writer(&mut to, &value)?;
        writeln!(to)
    } else if let Some(error) = &response.error {
        writeln!(to, "{}: {}", error.code, error.message)?;
        if let Some(execution) = error.details["execution_id"]
            .as_str()
            .filter(|id| crate::cli::execution_id(id))
        {
            writeln!(to, "Execution: {execution}")?;
        }
        Ok(())
    } else {
        serde_json::to_writer_pretty(&mut to, &response.data)?;
        writeln!(to)
    }
}

/// Render the answer to `operation`: `options` gets a list a person can read, every
/// other command (and `--json`) the generic output of `write`.
pub fn write_command(
    operation: &str,
    response: &Response,
    structured: bool,
    mut to: impl Write,
) -> io::Result<()> {
    let text = (operation == "options" && !structured && response.error.is_none())
        .then(|| options_text(&response.data))
        .flatten();
    match text {
        Some(text) => to.write_all(text.as_bytes()),
        None => match &response.error {
            Some(error) if !structured && operation == "options" => {
                writeln!(
                    to,
                    "{}: {}",
                    error.code,
                    options_failure(error.code.as_str(), &error.message)
                )
            }
            _ => write(response, structured, to),
        },
    }
}
fn options_failure<'a>(code: &str, server_message: &'a str) -> &'a str {
    match code {
        "options_timeout" => "The plugin did not answer in time; try again",
        "resource_exhausted" => {
            "Too many option queries are pending for this plugin, or the list is too large to return"
        }
        "executor_unavailable" => "The plugin is not installed or not ready",
        "invalid_request" => "Not a dynamic field of this plugin, or the context is invalid",
        "not_found" => "No such entity",
        _ => server_message,
    }
}
/// `None` when the data is not a well-formed options answer.
fn options_text(data: &rad_api::ResponseData) -> Option<String> {
    let rad_api::ResponseData::Options(data) = data else {
        return None;
    };
    let available = data.availability == rad_api::OptionsAvailability::Available;
    let mut text = format!(
        "availability: {}\n",
        if available {
            "available"
        } else {
            "unavailable"
        }
    );
    for item in &data.items {
        text += &format!("{} — {}\n", printable(&item.value), printable(&item.label));
    }
    if available && data.items.is_empty() {
        text += "(no options)\n";
    }
    if let Some(saved) = &data.saved {
        let listed = data.saved_listed?;
        let state = match saved {
            Some(value) => format!(
                "{}{}",
                printable(value),
                if listed { "" } else { " (not available)" }
            ),
            None => "(none or unreadable)".into(),
        };
        text += &format!("saved: {state}\n");
    }
    Some(text)
}
/// Plugin text goes to a terminal: drop what could drive it.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .collect()
}
