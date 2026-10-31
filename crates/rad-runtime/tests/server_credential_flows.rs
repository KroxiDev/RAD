// Dynamic options and JPEG visuals through the real runtime, the real IPC and a real plugin
// process (`org.rad.test.options`, synthetic: no apps, audio, hardware, network or files).
use rad_client::connection::{Client, Endpoint};
use rad_launcher::Launcher;
use rad_runtime::server::Server;
use serde_json::{Value, json};
use std::{
    num::NonZeroU128,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const PLUGIN: &str = "org.rad.test.credentials.deferred";

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn binaries() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// The plugin store the runtime opens at start, with the test plugin already selected.
fn seed(store: &Path) {
    let plugins = store.join("plugins");
    let reinstall = plugins.exists();
    let mut manager = if reinstall {
        rad_plugin_manager::Manager::open(
            &plugins,
            rad_plugin_contract::HOST_API,
            rad_plugin_host::TARGET,
        )
        .unwrap()
    } else {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&plugins)
            .unwrap();
        rad_plugin_manager::Manager::create(
            &plugins,
            rad_plugin_contract::HOST_API,
            rad_plugin_host::TARGET,
        )
        .unwrap()
    };
    let manifest = serde_json::to_vec(&json!({"contract":1,"capabilities":["credentials","dynamic_options","visual_freshness"],"id":PLUGIN,"version":{"major":1,"minor":0,"patch":0},"host_min":1,"host_max":1,"targets":[rad_plugin_host::TARGET],"actions":[{"id":"authenticate","label":"Authenticate","schema":{"version":1,"fields":[{"id":"revision","label":"Revision","required":false,"kind":{"type":"dynamic_choice","depends_on":[]}}]},"credential_slots":["password"]}],"visuals":[{"id":"probe","label":"Probe","schema":{"version":1,"fields":[]},"credential_slots":["password"]}]})).unwrap();
    let executable = std::fs::read(binaries().join("rad-plugin-credentials-fixture")).unwrap();
    let candidate = rad_plugin_package::Candidate::from_parts(
        &manifest,
        &executable,
        rad_plugin_contract::HOST_API,
        rad_plugin_host::TARGET,
    )
    .unwrap();
    let bytes = rad_plugin_package::encode(&candidate);
    let mut prepared = Some(
        manager
            .prepare(
                if reinstall {
                    rad_plugin_manager::Operation::Reinstall(PLUGIN.into())
                } else {
                    rad_plugin_manager::Operation::Install
                },
                Some(&bytes),
            )
            .unwrap(),
    );
    assert!(matches!(
        manager.commit(&mut prepared).unwrap(),
        rad_plugin_manager::CommitOutcome::Applied
    ));
}

/// Tells the serving loop that the scenario is over, and wakes it with a connection.
struct Announce(std::sync::Arc<std::sync::atomic::AtomicBool>, PathBuf);
impl Drop for Announce {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
        let _ = std::os::unix::net::UnixStream::connect(self.1.join("rad").join("editor.sock"));
    }
}

/// What the serving loop last saw as unanswered client requests (the worlds take turns).
static OPEN_REQUESTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One launcher helper may live per process, so runtimes of different tests take turns.
static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Runs `scenario` on a client thread against a private runtime that owns the test plugin,
/// serving the runtime on this thread until the scenario ends.
fn world(name: &str, scenario: impl FnOnce(PathBuf) + Send + 'static) {
    let root = Directory(
        std::env::temp_dir().join(format!("rad-cred-flow-{}-{name}", std::process::id())),
    );
    serve(&root.0, name, scenario);
}
fn serve(root: &Path, name: &str, scenario: impl FnOnce(PathBuf) + Send + 'static) {
    let _turn = TURN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for sub in ["", "runtime", "home", "data", "system", "config", "cache"] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(root.join(sub))
            .unwrap();
    }
    let mut server = Server::new(
        &root.join("runtime"),
        &root.join("store"),
        NonZeroU128::new(0x4f50_0000 + name.len() as u128).unwrap(),
    )
    .unwrap();
    if !root.join("store/plugins").exists() {
        seed(&root.join("store"));
    }
    let helper = || {
        let mut command = std::process::Command::new(
            std::env::var("RAD_TEST_LAUNCHER_HELPER").expect("runner builds the private helper"),
        );
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
        command
    };
    // The previous runtime's helper is reaped by the time its server stopped; allow a beat.
    let end = Instant::now() + Duration::from_secs(5);
    let launcher = loop {
        match Launcher::spawn(helper()) {
            Ok(launcher) => break launcher,
            Err(error) => {
                assert!(
                    Instant::now() < end,
                    "launcher helper unavailable: {error:?}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    server.attach_launcher(launcher).unwrap();
    let runtime = root.join("runtime");
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let client = {
        let finished = finished.clone();
        std::thread::spawn(move || {
            let _announce = Announce(finished, runtime.clone());
            scenario(runtime);
        })
    };
    // Like the product loop: no periodic polling, only readiness and the server's own
    // deadlines (a plugin's options deadline included) wake it. The maximum is a safety net.
    let end = Instant::now() + Duration::from_secs(60);
    while !finished.load(std::sync::atomic::Ordering::Acquire) {
        server.turn().unwrap();
        OPEN_REQUESTS.store(
            server.pending_requests(),
            std::sync::atomic::Ordering::Release,
        );
        assert!(Instant::now() < end, "scenario watchdog");
        server.wait(Duration::from_secs(5)).unwrap();
    }
    let result = client.join();
    server.request_shutdown();
    let end = Instant::now() + Duration::from_secs(10);
    while !server.shutdown_complete() {
        server.turn().unwrap();
        assert!(Instant::now() < end, "runtime did not stop");
        server.wait(Duration::from_millis(10)).unwrap();
    }
    result.unwrap();
}

fn connect(runtime: &Path, endpoint: Endpoint) -> Client {
    Client::connect(runtime, endpoint).unwrap()
}

fn call(client: &mut Client, op: &str, args: Value) -> Value {
    let response = client.request(op, args).unwrap();
    assert!(response.error.is_none(), "{op}: {:?}", response.error);
    serde_json::to_value(response.data).unwrap()
}

/// Waits for selected metadata; credentials remain configurable while dormant.
fn ready(editor: &mut Client) {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        let status = call(editor, "plugin_status", json!({}));
        if status["state"] == "idle" && status["items"][0]["id"] == PLUGIN {
            return;
        }
        assert!(status["state"] != "failed", "plugin admin failed: {status}");
        assert!(Instant::now() < end, "plugin never became ready: {status}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn options_wait_for_credentials_ack_and_use_the_captured_revision() {
    world("options", |runtime| {
        let mut editor = connect(&runtime, Endpoint::Editor);
        ready(&mut editor);
        call(
            &mut editor,
            "credential_replace",
            json!({"plugin_id":PLUGIN,"slot":"password","secret":"correct"}),
        );
        let result = call(
            &mut editor,
            "options",
            json!({"type":format!("{PLUGIN}:authenticate"),"field":"revision","cursor":null,"context":{}}),
        );
        assert_eq!(result["availability"], "available", "{result}");
        assert!(
            result["items"][0]["value"]
                .as_str()
                .unwrap()
                .starts_with("rev"),
            "{result}"
        );
    });
}

#[test]
fn visual_resubscribes_with_new_revision_and_never_keeps_old_state_current() {
    world("visual", |runtime| {
        let mut editor = connect(&runtime, Endpoint::Editor);
        ready(&mut editor);
        call(
            &mut editor,
            "credential_replace",
            json!({"plugin_id":PLUGIN,"slot":"password","secret":"correct"}),
        );
        let folder = call(&mut editor, "status", json!({}))["active_folder_id"].clone();
        call(
            &mut editor,
            "create_button",
            json!({"folder_id":folder,"position":0,"name":"Credential visual"}),
        );
        call(
            &mut editor,
            "edit",
            json!({"changes":[{"kind":"set","id":"b1","field":"visual","value":{"type":format!("{PLUGIN}#probe"),"parameters":{"schema_version":1,"values":{}}}}]}),
        );
        let first = await_visual(&mut editor, |v| v["availability"] == "current");
        call(
            &mut editor,
            "credential_replace",
            json!({"plugin_id":PLUGIN,"slot":"password","secret":"wrong"}),
        );
        let next = call(&mut editor, "plugin_visual", json!({"id":"b1"}));
        assert_ne!(
            next["availability"], "current",
            "old authenticated state must stop being current after save"
        );
        call(
            &mut editor,
            "credential_replace",
            json!({"plugin_id":PLUGIN,"slot":"password","secret":"correct"}),
        );
        let last = await_visual(&mut editor, |v| {
            v["availability"] == "current" && v["text"] != first["text"]
        });
        assert!(last["text"].as_str().unwrap().starts_with("rev"));
    });
}
fn await_visual(editor: &mut Client, predicate: impl Fn(&Value) -> bool) -> Value {
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        let value = call(editor, "plugin_visual", json!({"id":"b1"}));
        if predicate(&value) {
            return value;
        }
        assert!(Instant::now() < end, "visual did not settle: {value}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

// Requests keep arriving while the plugin becomes Ready and completes: none is lost and
// nothing waits for traffic that never comes.

#[test]
fn leaving_while_waiting_for_credentials_ack_releases_the_transport_credit() {
    world("leaving", |runtime| {
        let mut editor = connect(&runtime, Endpoint::Editor);
        ready(&mut editor);
        call(
            &mut editor,
            "credential_replace",
            json!({"plugin_id":PLUGIN,"slot":"password","secret":"correct"}),
        );
        let mut leaving = connect(&runtime, Endpoint::Editor);
        let result = leaving.request_until("options", json!({"type":format!("{PLUGIN}:authenticate"),"field":"revision","cursor":null,"context":{}}), Instant::now() + Duration::from_millis(5));
        assert_eq!(result.unwrap_err().code, "deadline_exceeded");
        drop(leaving);
        let end = Instant::now() + Duration::from_secs(1);
        while OPEN_REQUESTS.load(std::sync::atomic::Ordering::Acquire) != 0 {
            assert!(
                Instant::now() < end,
                "abandoned ACK waiter leaked its transport credit"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        call(&mut editor, "status", json!({}));
    });
}

fn authenticate(editor: &mut Client) -> Value {
    let run = call(editor, "run", json!({"id":"a1"}));
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        let result = call(
            editor,
            "result",
            json!({"execution_id":run["execution_id"]}),
        );
        if result["state"] != "running" {
            return result;
        }
        assert!(Instant::now() < end, "fixture action stalled");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn credentials_survive_runtime_restart_and_key_loss_requires_reentry_not_reinstallation() {
    let root =
        Directory(std::env::temp_dir().join(format!("rad-cred-restart-{}", std::process::id())));
    serve(&root.0, "first", |runtime| {
        let mut editor = connect(&runtime, Endpoint::Editor);
        ready(&mut editor);
        call(
            &mut editor,
            "credential_replace",
            json!({"plugin_id":PLUGIN,"slot":"password","secret":"runtime-private-canary-F4"}),
        );
        call(
            &mut editor,
            "create_button",
            json!({"folder_id":"f1","position":0,"name":"Auth"}),
        );
        call(
            &mut editor,
            "add_action",
            json!({"button_id":"b1","type":format!("{PLUGIN}:authenticate"),"parameters":{"schema_version":1,"values":{}}}),
        );
        assert_eq!(authenticate(&mut editor)["state"], "failed");
        assert_private_surfaces(&mut editor, &runtime);

        call(
            &mut editor,
            "credential_replace",
            json!({"plugin_id":PLUGIN,"slot":"password","secret":"correct"}),
        );
        assert_eq!(authenticate(&mut editor)["state"], "succeeded");
        call(
            &mut editor,
            "edit",
            json!({"changes":[{"kind":"set","id":"b1","field":"name","value":"Renamed"}]}),
        );
        call(&mut editor, "undo", json!({}));
        assert_eq!(
            authenticate(&mut editor)["state"],
            "succeeded",
            "undo must not revert credentials"
        );
        call(&mut editor, "redo", json!({}));
        assert_eq!(
            authenticate(&mut editor)["state"],
            "succeeded",
            "redo must not revert credentials"
        );
    });
    let key_path = root.0.join("store/credentials/master.key");
    let original_key = std::fs::read(&key_path).unwrap();
    assert_eq!(original_key.len(), 40);
    assert_eq!(&original_key[..8], b"RADK\0\0\0\x01");
    assert_private_files(&root.0.join("store"), &original_key[8..]);

    seed(&root.0.join("store")); // Reinstall the same private package while runtime is stopped.
    serve(&root.0, "second", |runtime| {
        let mut editor = connect(&runtime, Endpoint::Editor);
        ready(&mut editor);
        assert_eq!(
            call(
                &mut editor,
                "credential_status",
                json!({"plugin_id":PLUGIN,"slot":"password"})
            )["state"],
            "present"
        );
        assert_eq!(authenticate(&mut editor)["state"], "succeeded");
    });
    assert_eq!(std::fs::read(&key_path).unwrap(), original_key);
    std::fs::remove_file(&key_path).unwrap(); // Own isolated fixture key only.
    serve(&root.0, "third", |runtime| {
        let mut editor = connect(&runtime, Endpoint::Editor);
        ready(&mut editor);
        assert_eq!(
            call(
                &mut editor,
                "credential_status",
                json!({"plugin_id":PLUGIN,"slot":"password"})
            )["state"],
            "unavailable"
        );
        assert_eq!(
            authenticate(&mut editor)["state"],
            "failed",
            "empty KeyMissing snapshot is admitted, plugin reports auth failure"
        );
        call(
            &mut editor,
            "credential_replace",
            json!({"plugin_id":PLUGIN,"slot":"password","secret":"correct"}),
        );
        assert_eq!(authenticate(&mut editor)["state"], "succeeded");
    });
    assert_ne!(std::fs::read(&key_path).unwrap(), original_key);
}

fn decode_base64(text: &str) -> Vec<u8> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = Vec::new();
    let mut bits = 0u32;
    let mut available = 0;
    for byte in text.bytes().take_while(|b| *b != b'=') {
        let value = alphabet
            .iter()
            .position(|b| *b == byte)
            .expect("base64 output") as u32;
        bits = (bits << 6) | value;
        available += 6;
        if available >= 8 {
            available -= 8;
            result.push((bits >> available) as u8);
        }
    }
    result
}
fn private_bytes(bytes: &[u8], key: &[u8]) {
    assert!(
        !bytes
            .windows(b"runtime-private-canary-F4".len())
            .any(|v| v == b"runtime-private-canary-F4"),
        "plaintext leaked"
    );
    assert!(
        !bytes.windows(key.len()).any(|v| v == key),
        "master key leaked"
    );
}
fn assert_private_files(path: &Path, key: &[u8]) {
    for entry in std::fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "credentials" {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            assert_private_files(&entry.path(), key);
        } else {
            private_bytes(&std::fs::read(entry.path()).unwrap(), key);
        }
    }
}
fn assert_private_surfaces(editor: &mut Client, runtime: &Path) {
    let key = std::fs::read(
        runtime
            .parent()
            .unwrap()
            .join("store/credentials/master.key"),
    )
    .unwrap();
    for (op, args) in [
        ("status", json!({})),
        ("get", json!({"id":"a1"})),
        (
            "schema",
            json!({"type":format!("{PLUGIN}:authenticate"),"cursor":null}),
        ),
        (
            "credential_status",
            json!({"plugin_id":PLUGIN,"slot":"password"}),
        ),
        ("plugin_configuration", json!({"id":"a1","cursor":null})),
    ] {
        let response = editor.request(op, args).unwrap();
        assert!(
            response.error.is_none(),
            "query {op} rejected: {:?}",
            response.error
        );
        private_bytes(format!("{response:?}").as_bytes(), &key[8..]);
        private_bytes(&serde_json::to_vec(&response).unwrap(), &key[8..]);
    }
    let token = call(
        editor,
        "transfer_begin",
        json!({"kind":"export","size":null}),
    )["transfer_id"]
        .clone();
    let mut package = Vec::new();
    loop {
        let part = call(
            editor,
            "transfer_read",
            json!({"transfer_id":token,"offset":package.len().to_string()}),
        );
        package.extend(decode_base64(part["base64"].as_str().unwrap()));
        if part["eof"] == true {
            break;
        }
    }
    private_bytes(&package, &key[8..]);
    assert_eq!(&package[..8], b"RADPKG\r\n");
    assert_eq!(u16::from_be_bytes(package[8..10].try_into().unwrap()), 2);
    let document_len = u32::from_be_bytes(package[20..24].try_into().unwrap()) as usize;
    let resources = u16::from_be_bytes(package[24..26].try_into().unwrap());
    assert_eq!(resources, 0, "no credential archive resources allowed");
    let metadata_len = u16::from_be_bytes(package[26..28].try_into().unwrap()) as usize;
    assert_eq!(
        metadata_len, 48,
        "identity metadata has a fixed, non-secret layout"
    );
    assert_eq!(
        package.len(),
        92 + document_len + metadata_len,
        "package contains only configuration, identity metadata and digest, no vault/key"
    );
    let config: Value = serde_json::from_slice(&package[60..60 + document_len]).unwrap();
    assert!(!config.to_string().contains("master.key"));
    fn no_secret_fields(value: &Value) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    assert!(!["credentials", "master_key", "secret"].contains(&key.as_str()));
                    no_secret_fields(value);
                }
            }
            Value::Array(items) => {
                for value in items {
                    no_secret_fields(value);
                }
            }
            _ => {}
        }
    }
    no_secret_fields(&config);
}

#[test]
fn options_keep_old_snapshot_during_save_and_share_the_existing_two_query_limit() {
    world("options-capture", |runtime| {
        let mut editor = connect(&runtime, Endpoint::Editor);
        ready(&mut editor);
        call(
            &mut editor,
            "credential_replace",
            json!({"plugin_id":PLUGIN,"slot":"password","secret":"wrong"}),
        );
        let mut first = connect(&runtime, Endpoint::Editor);
        let mut second = connect(&runtime, Endpoint::Editor);
        let question = || json!({"type":format!("{PLUGIN}:authenticate"),"field":"revision","cursor":null,"context":{}});
        let q1 = question();
        let q2 = question();
        let one = std::thread::spawn(move || call(&mut first, "options", q1));
        let two = std::thread::spawn(move || call(&mut second, "options", q2));
        let end = Instant::now() + Duration::from_secs(2);
        while OPEN_REQUESTS.load(std::sync::atomic::Ordering::Acquire) != 2 {
            assert!(
                !one.is_finished() && !two.is_finished(),
                "fixture ACK arrived before both admissions were observed"
            );
            assert!(Instant::now() < end);
            std::thread::yield_now();
        }
        let rejected = editor.request("options", question()).unwrap();
        assert_eq!(rejected.error.unwrap().code, "resource_exhausted");
        call(
            &mut editor,
            "credential_replace",
            json!({"plugin_id":PLUGIN,"slot":"password","secret":"correct"}),
        );
        let old_one = one.join().unwrap();
        let old_two = two.join().unwrap();
        assert_eq!(old_one["items"], old_two["items"]);
        let current = call(&mut editor, "options", question());
        assert_ne!(
            old_one["items"], current["items"],
            "pending options must retain the admission revision, not use the replacement"
        );
    });
}
