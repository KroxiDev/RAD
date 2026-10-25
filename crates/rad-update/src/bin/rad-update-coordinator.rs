fn main() -> std::process::ExitCode {
    match rad_update::coordinator::run(std::env::args_os().skip(1)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("RAD update coordinator: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
