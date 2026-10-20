//! Fixture-only bridge: scenario vectors use the canonical typed client, never a wire parser.
use std::io::{self, BufRead, Read, Write};
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("pack") {
        assert_eq!(args.len(), 4);
        let manifest = std::fs::read(&args[1]).unwrap();
        let executable = std::fs::read(&args[2]).unwrap();
        let candidate = rad_plugin_package::Candidate::from_parts(
            &manifest,
            &executable,
            rad_plugin_contract::HOST_API,
            rad_plugin_host::TARGET,
        )
        .unwrap();
        std::fs::write(&args[3], rad_plugin_package::encode(&candidate)).unwrap();
        return;
    }
    assert_eq!(args.len(), 2);
    let endpoint = match args[1].as_str() {
        "editor" => rad_client::connection::Endpoint::Editor,
        "automation" => rad_client::connection::Endpoint::Automation,
        _ => panic!("unknown fixture endpoint"),
    };
    let mut client =
        rad_client::connection::Client::connect(std::path::Path::new(&args[0]), endpoint).unwrap();
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let mut line = String::new();
        let size = input.by_ref().take(65537).read_line(&mut line).unwrap();
        if size == 0 {
            break;
        }
        assert!(size <= 65536 && line.ends_with('\n'));
        let (operation, arguments): (String, serde_json::Value) =
            serde_json::from_str(&line).unwrap();
        let request = rad_api::Request::parse(&operation, arguments).unwrap();
        let response = client
            .call(request)
            .unwrap_or_else(rad_client::output::failure);
        rad_client::output::write(&response, true, &mut output).unwrap();
        output.flush().unwrap();
    }
}
