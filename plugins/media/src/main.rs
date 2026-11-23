use rad_plugin_media::{MediaPlugin, Timing, config, session_bus_address};
use std::{path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    if let Some(exit) = rad_plugin_sdk::licenses_exit() {
        return exit;
    }
    let bus = session_bus_address(
        std::env::var("DBUS_SESSION_BUS_ADDRESS").ok(),
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
    );
    let plugin = MediaPlugin::new(bus, Timing::default());
    match rad_plugin_sdk::run_stdio(config(), plugin) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rad-plugin-media: {error}");
            ExitCode::FAILURE
        }
    }
}
