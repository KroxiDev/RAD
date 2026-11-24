use rad_launcher::{Launcher, LauncherError};
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

pub fn spawn(root: &Path) -> Result<Launcher, LauncherError> {
    let helper =
        std::env::var_os("RAD_TEST_LAUNCHER_HELPER").expect("runner builds the private helper");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut command = Command::new(&helper);
        command
            .env_clear()
            .env("HOME", root.join("home"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_DATA_DIRS", root.join("system"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .env("PATH", "/nonexistent-rad-private-bin")
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent-rad-private-bus",
            );
        match Launcher::spawn(command) {
            Err(LauncherError::Unavailable) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            result => return result,
        }
    }
}
