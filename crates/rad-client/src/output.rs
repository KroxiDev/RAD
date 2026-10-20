use crate::{
    connection::Failure,
    response::{Error, Outcome, Response, Status},
};
use serde_json::{Value, json};
use std::io::{self, Write};
pub fn failure(error: Failure) -> Response {
    Response {
        v: 1,
        id: error.id.map(|id| id.to_string()),
        session_id: error.session_id,
        status: Status::Error,
        data: Value::Null,
        error: Some(Error {
            code: error.code.into(),
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
        match error.code.as_str() {
            "invalid_request"
            | "unsupported_version"
            | "invalid_configuration"
            | "identity_collision"
            | "cursor_expired" => 2,
            "permission_denied" | "operation_not_allowed" => 3,
            "not_found" | "result_expired" => 4,
            "busy"
            | "resource_exhausted"
            | "executor_unavailable"
            | "service_unavailable"
            | "options_timeout" => 5,
            "action_failed" | "action_timeout" => 6,
            "storage_error" | "storage_uncertain" | "recovery_required" => 7,
            "wait_timeout" => 9,
            _ => 8,
        }
    } else if matches!(
        response.data["state"].as_str(),
        Some("failed" | "timed_out")
    ) {
        6
    } else {
        0
    }
}
pub fn write(response: &Response, structured: bool, mut to: impl Write) -> io::Result<()> {
    if structured {
        serde_json::to_writer(&mut to, response)?;
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
                    options_failure(&error.code, &error.message)
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
fn options_text(data: &Value) -> Option<String> {
    let available = match data["availability"].as_str()? {
        "available" => true,
        "unavailable" => false,
        _ => return None,
    };
    let items = data["items"].as_array()?;
    let mut text = format!(
        "availability: {}\n",
        if available {
            "available"
        } else {
            "unavailable"
        }
    );
    for item in items {
        text += &format!(
            "{} — {}\n",
            printable(item["value"].as_str()?),
            printable(item["label"].as_str()?)
        );
    }
    if available && items.is_empty() {
        text += "(no options)\n";
    }
    if let Some(saved) = data.get("saved") {
        let state = match (saved.as_str(), data["saved_listed"].as_bool()?) {
            (Some(value), listed) => format!(
                "{}{}",
                printable(value),
                if listed { "" } else { " (not available)" }
            ),
            (None, _) => "(none or unreadable)".into(),
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
