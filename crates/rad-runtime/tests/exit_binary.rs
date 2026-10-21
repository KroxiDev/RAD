//! The real binary ends in the class ADR 0011 gives each cause (see `rad_runtime::exit`).
use rad_client::connection::{Client, Endpoint};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const BINARY: &str = env!("CARGO_BIN_EXE_rad-runtime");

/// Private directories under tmp; the environment of each run is built from scratch.
struct Sandbox(PathBuf);
impl Sandbox {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("rad-exit-bin-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        for sub in ["runtime", "home", "data", "empty-system", "config", "cache"] {
            fs::create_dir_all(base.join(sub)).unwrap();
            fs::set_permissions(base.join(sub), fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(base)
    }
    fn runtime(&self) -> PathBuf {
        self.0.join("runtime")
    }
    /// Where the runtime keeps its data (`$XDG_DATA_HOME/rad`).
    fn data(&self) -> PathBuf {
        self.0.join("data/rad")
    }
    fn command(&self) -> Command {
        let mut command = Command::new(BINARY);
        command
            .env_clear()
            .env("XDG_RUNTIME_DIR", self.runtime())
            .env("XDG_DATA_HOME", self.0.join("data"))
            .env("HOME", self.0.join("home"))
            .env("XDG_DATA_DIRS", self.0.join("empty-system"))
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_CACHE_HOME", self.0.join("cache"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent-rad-exit-private-bus",
            )
            .env("PATH", "/nonexistent-rad-exit-bin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Never leaves a runtime behind, whatever the test does.
struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn exit_code(command: &mut Command) -> (Option<i32>, String) {
    let output = command.output().unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn blocking_conditions_exit_75_and_leave_no_record() {
    // No environment at all, and arguments the runtime does not know.
    let (code, stderr) = exit_code(Command::new(BINARY).env_clear());
    assert_eq!(code, Some(75), "{stderr}");
    assert!(stderr.contains("XDG_RUNTIME_DIR"), "{stderr}");
    let sandbox = Sandbox::new("blocking");
    let (code, _) = exit_code(sandbox.command().arg("--unknown"));
    assert_eq!(code, Some(75));
    // The deck is a setting of the machine now, not an argument.
    let (code, _) = exit_code(sandbox.command().arg("--xl-exclusive"));
    assert_eq!(code, Some(75));
    // A runtime directory open to others, and one behind a symlink.
    fs::set_permissions(sandbox.runtime(), fs::Permissions::from_mode(0o755)).unwrap();
    let (code, stderr) = exit_code(&mut sandbox.command());
    assert_eq!(code, Some(75), "{stderr}");
    fs::set_permissions(sandbox.runtime(), fs::Permissions::from_mode(0o700)).unwrap();
    let link = sandbox.0.join("runtime-link");
    symlink(sandbox.runtime(), &link).unwrap();
    let (code, stderr) = exit_code(sandbox.command().env("XDG_RUNTIME_DIR", &link));
    assert_eq!(code, Some(75), "{stderr}");
    // Blocking exits are not recorded: the unit state already tells the Editor.
    assert!(!sandbox.data().join("last-exit.json").exists());
}

#[cfg(feature = "fixture-tests")]
mod serving {
    use super::*;

    fn start(sandbox: &Sandbox) -> (Running, Client) {
        let helper = Path::new(BINARY).with_file_name("rad-launcher-helper");
        assert!(helper.is_file(), "build the companion helper first");
        let mut child = Running(sandbox.command().spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(client) = Client::connect(&sandbox.runtime(), Endpoint::Editor) {
                return (child, client);
            }
            if let Some(status) = child.0.try_wait().unwrap() {
                panic!("runtime exited before ready: {status}");
            }
            assert!(Instant::now() < deadline, "runtime readiness timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(mut child: Running) -> Option<i32> {
        // Only the child created by this test is signalled.
        assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) }, 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                return status.code();
            }
            assert!(Instant::now() < deadline, "shutdown timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn view(client: &mut Client, operation: &str) -> Value {
        let response = client.request(operation, json!({})).unwrap();
        assert!(response.error.is_none(), "{operation}: {response:?}");
        serde_json::to_value(&response.data).unwrap()
    }

    /// `(code, subject)` of every event in a `runtime_log` answer, oldest first.
    fn event_codes(log: &Value) -> Vec<(String, Option<String>)> {
        log["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry["code"].as_str().unwrap().to_owned(),
                    entry["subject"].as_str().map(str::to_owned),
                )
            })
            .collect()
    }

    #[test]
    fn a_running_runtime_tells_the_editor_it_started_and_a_clean_stop_leaves_no_trace() {
        let sandbox = Sandbox::new("log");
        let (running, mut client) = start(&sandbox);
        let log = view(&mut client, "runtime_log");
        assert_eq!(event_codes(&log), [("started".into(), None)]);
        assert_eq!(log["dropped"], "0");
        // Reading the log is not an event, and the answer is the same the next time.
        assert_eq!(view(&mut client, "runtime_log"), log);
        drop(client);
        assert_eq!(stop(running), Some(0));
        let (running, mut client) = start(&sandbox);
        assert_eq!(
            event_codes(&view(&mut client, "runtime_log")),
            [("started".into(), None)],
            "the ring belongs to one process"
        );
        drop(client);
        assert_eq!(stop(running), Some(0));
    }

    #[test]
    fn a_second_instance_is_blocking_and_the_first_keeps_serving() {
        let sandbox = Sandbox::new("second");
        let (first, mut client) = start(&sandbox);
        let (code, stderr) = exit_code(&mut sandbox.command());
        assert_eq!(code, Some(75), "{stderr}");
        // The refused instance neither disturbed the first nor left a record.
        view(&mut client, "status");
        assert!(!sandbox.data().join("last-exit.json").exists());
        drop(client);
        assert_eq!(stop(first), Some(0));
    }

    #[test]
    fn an_unsafe_data_directory_keeps_the_runtime_in_recovery_and_writes_nothing_in_it() {
        let sandbox = Sandbox::new("data-unsafe");
        fs::create_dir(sandbox.data()).unwrap();
        fs::set_permissions(sandbox.data(), fs::Permissions::from_mode(0o755)).unwrap();
        let (running, mut client) = start(&sandbox);
        assert_eq!(
            view(&mut client, "recovery"),
            json!({
                "cause":"unsafe_folder",
                "actions":["details"],
                "detail":{"code":"unsafe_directory","counters_read":false}
            })
        );
        drop(client);
        assert_eq!(stop(running), Some(0));
        assert_eq!(fs::read_dir(sandbox.data()).unwrap().count(), 0);
    }

    #[test]
    fn a_newer_database_keeps_the_runtime_in_recovery_and_stays_untouched() {
        let sandbox = Sandbox::new("newer");
        let (first, client) = start(&sandbox);
        drop(client);
        assert_eq!(stop(first), Some(0));
        let database = sandbox.data().join("state.sqlite3");
        let mut bytes = fs::read(&database).unwrap();
        bytes[60..64].copy_from_slice(&3u32.to_be_bytes());
        fs::write(&database, &bytes).unwrap();
        let (running, mut client) = start(&sandbox);
        assert_eq!(view(&mut client, "recovery")["cause"], "newer_version");
        drop(client);
        assert_eq!(stop(running), Some(0));
        assert_eq!(fs::read(&database).unwrap(), bytes);
    }

    /// Holds SQLite's PENDING byte of `database` for as long as it lives, as another
    /// process with the file open for writing would: opening it then reports `Busy`.
    struct Locked(#[allow(dead_code)] fs::File); // the lock lives as long as the file is open
    impl Locked {
        fn new(database: &Path) -> Self {
            use std::os::fd::AsRawFd;
            let file = fs::OpenOptions::new().write(true).open(database).unwrap();
            // SAFETY: `lock` is fully initialised and outlives the call.
            let result = unsafe {
                let mut lock: libc::flock = std::mem::zeroed();
                lock.l_type = libc::F_WRLCK as _;
                lock.l_whence = libc::SEEK_SET as _;
                lock.l_start = 0x4000_0000;
                lock.l_len = 512;
                libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock)
            };
            assert_eq!(result, 0, "the lock must be taken");
            Self(file)
        }
    }

    #[test]
    fn an_unexpected_startup_failure_exits_1_and_the_next_start_reports_why_once() {
        let sandbox = Sandbox::new("startup");
        let (first, client) = start(&sandbox);
        drop(client);
        assert_eq!(stop(first), Some(0));
        let database = sandbox.data().join("state.sqlite3");
        // A clean start has nothing to report.
        assert!(!sandbox.data().join("last-exit.json").exists());
        // Not a case for recovery: the database is fine, but someone else holds it.
        let locked = Locked::new(&database);
        let (code, stderr) = exit_code(&mut sandbox.command());
        assert_eq!(code, Some(1), "{stderr}");
        assert!(stderr.contains("falló el arranque"), "{stderr}");
        let record: Value =
            serde_json::from_slice(&fs::read(sandbox.data().join("last-exit.json")).unwrap())
                .unwrap();
        assert_eq!(record["version"], 1);
        assert_eq!(record["code"], "startup");
        let at = record["at_unix_ms"].as_u64().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(now - at < 60_000, "{at} vs {now}");
        // Once the database is free the runtime starts and tells the Editor.
        drop(locked);
        let (running, mut client) = start(&sandbox);
        let overview = view(&mut client, "overview");
        assert_eq!(overview["previous_exit"]["code"], "startup");
        assert_eq!(
            overview["previous_exit"]["at_unix_ms"],
            at.to_string(),
            "{overview}"
        );
        let status = view(&mut client, "status");
        assert_eq!(status["previous_exit"], overview["previous_exit"]);
        // The Log says the same: why the last run ended comes right after this start.
        let log = event_codes(&view(&mut client, "runtime_log"));
        assert_eq!(
            log,
            [
                ("started".into(), None),
                ("previous_exit".into(), Some("startup".into()))
            ]
        );
        assert!(!sandbox.data().join("last-exit.json").exists());
        drop(client);
        assert_eq!(stop(running), Some(0));
        // The record was consumed: the next start exposes nothing.
        let (running, mut client) = start(&sandbox);
        assert!(view(&mut client, "overview").get("previous_exit").is_none());
        assert!(view(&mut client, "status").get("previous_exit").is_none());
        drop(client);
        assert_eq!(stop(running), Some(0));
    }
}
