use rad_client::{
    cli::{self, Invocation},
    connection::{Client, Endpoint, Failure},
    output,
    response::Outcome,
};
fn main() {
    std::process::exit(run() as i32);
}
fn run() -> u8 {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let structured = args.iter().any(|s| s == "--json");
    let args: Option<Vec<String>> = args.into_iter().map(|s| s.into_string().ok()).collect();
    let parsed = args.and_then(|a| cli::parse(&a).ok());
    let Some(invocation) = parsed else {
        return emit(
            output::failure(Failure {
                code: "invalid_request",
                id: None,
                session_id: None,
                outcome: Some(Outcome::NotApplied),
            }),
            structured,
            None,
        );
    };
    let Invocation::Command(command) = invocation else {
        println!(
            "rad-cli status\nrad-cli get ID\nrad-cli list [--cursor TOKEN]\nrad-cli schema TYPE [--cursor TOKEN]\nrad-cli options ID FIELD [--cursor TOKEN] [--with DEP=VALUE]...\nrad-cli options --type TYPE FIELD [--cursor TOKEN] [--with DEP=VALUE]...\nrad-cli set ID FIELD JSON_VALUE\nrad-cli move BUTTON POSITION\nrad-cli order BUTTON --indices JSON_ARRAY\nrad-cli run ID [--wait] [--wait-timeout-ms N]\nrad-cli result EXECUTION_ID [--wait] [--wait-timeout-ms N]\nAll commands accept --json."
        );
        return 0;
    };
    let prior = matches!(command.request.operation().as_str(), "edit" | "run")
        .then_some(Outcome::NotApplied);
    let result = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or(Failure {
            code: "runtime_unavailable",
            id: None,
            session_id: None,
            outcome: prior,
        })
        .and_then(|p| {
            Client::connect(std::path::Path::new(&p), Endpoint::Automation).map_err(|mut error| {
                error.outcome = prior;
                error
            })
        })
        .and_then(|mut client| rad_client::execute::command(&mut client, &command));
    let response = match result {
        Ok(response) => response,
        Err(error) => output::failure(error),
    };
    if response.error.as_ref().is_some_and(|e| {
        matches!(e.code.as_str(), "runtime_unavailable" | "deadline_exceeded")
            || e.outcome == Some(Outcome::Unknown)
    }) {
        eprintln!("Runtime unavailable or response outcome unknown");
    }
    emit(
        response,
        structured,
        Some(command.request.operation().as_str()),
    )
}
fn emit(response: rad_client::response::Response, structured: bool, operation: Option<&str>) -> u8 {
    let written = match operation {
        Some(operation) => {
            output::write_command(operation, &response, structured, std::io::stdout().lock())
        }
        None => output::write(&response, structured, std::io::stdout().lock()),
    };
    if written.is_err() {
        8
    } else {
        output::exit_code(&response)
    }
}
