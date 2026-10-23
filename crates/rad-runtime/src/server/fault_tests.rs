//! Critical whole-Server fault boundaries. All external endpoints are owned fixtures.
use super::*;
use rad_client::connection::{Client, Endpoint};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        net::{UnixListener, UnixStream},
        process::CommandExt,
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        MutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
};

const ECHO: &str = "org.rad.fault.echo";
const BLOCKED: &str = "org.rad.fault.no-reader";
static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);
fn profiled() -> bool {
    std::env::var_os("RAD_PROFILE_INTEGRATED_FAULTS").is_some_and(|v| v == "1")
}
fn emit(name: &str, kind: &str, mut value: Value) {
    if profiled() {
        value["suite"] = json!("remaining_integrated_faults");
        value["scenario"] = json!(name);
        value["kind"] = json!(kind);
        let mut out = std::io::stdout().lock();
        if kind == "metadata" {
            writeln!(out).unwrap();
        }
        writeln!(out, "{value}").unwrap();
        out.flush().unwrap();
    }
}
fn sample(name: &str, operation: &str, started: Instant) {
    emit(
        name,
        "sample",
        json!({"operation":operation,"ns":started.elapsed().as_nanos()}),
    );
}
fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn bins() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
struct Root {
    path: PathBuf,
    keep: bool,
}
impl Drop for Root {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
struct World {
    server: Option<Server>,
    root: Root,
    name: &'static str,
    _serial: MutexGuard<'static, ()>,
    stopped: bool,
}
impl World {
    fn new(name: &'static str) -> Self {
        let serial = FIXTURE_RUNTIME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let keep = profiled();
        let base = if keep {
            let base = PathBuf::from(
                std::env::var_os("RAD_PROFILE_FAULT_ROOT")
                    .expect("explicit retained run root required"),
            );
            let base = base.canonicalize().unwrap();
            assert!(base.starts_with(repo().join("target/performance").canonicalize().unwrap()));
            assert_eq!(fs::metadata(&base).unwrap().permissions().mode() & 0o077, 0);
            base
        } else {
            std::env::temp_dir()
        };
        // sockaddr_un is bounded; scenario names belong in metadata, not the socket path.
        let path = base.join(format!(
            "fi-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(
            path.join("runtime/rad/editor.sock").as_os_str().len() < 108,
            "fixture root makes the Unix socket path too long"
        );
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        for part in [
            "runtime", "home", "data", "system", "config", "cache", "tmp", "inputs",
        ] {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path.join(part))
                .unwrap();
        }
        let root = Root { path, keep };
        let server = Server::new(
            &root.path.join("runtime"),
            &root.path.join("store"),
            NonZeroU128::new(0xfa017).unwrap(),
        )
        .unwrap();
        emit(
            name,
            "metadata",
            json!({"scope":"real_Server_Transport_typed_Client_controlled_dependency",
            "clock":"Instant_wall_monotonic", "synthetic":true,
            "turn_scope":"World.run client workflow includes configuration and observation; excludes construction, package seeding, attach and shutdown",
            "limits":["controlled fixtures, no hardware or personal services", "observed intervals include scheduling and stated client observation polls",
                "Store return loss is injected after real SQLite commit, not a physical filesystem fault", "not contact, optical latency or exact wakeup counts"]}),
        );
        Self {
            server: Some(server),
            root,
            name,
            _serial: serial,
            stopped: false,
        }
    }
    fn input(&self, name: &str, source: &Path) -> PathBuf {
        let bytes = fs::read(source).unwrap();
        assert!(bytes.starts_with(b"\x7fELF"));
        let path = self.root.path.join("inputs").join(name);
        if !path.exists() {
            fs::write(&path, &bytes).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert_eq!(fs::read(&path).unwrap(), bytes);
        if profiled() {
            let hash = Command::new("/usr/bin/sha256sum")
                .arg(&path)
                .output()
                .unwrap();
            assert!(hash.status.success());
            let hash = String::from_utf8(hash.stdout).unwrap();
            emit(
                self.name,
                "input",
                json!({"name":name,"sha256":hash.split_whitespace().next().unwrap(),"retained_before_spawn":true}),
            );
        }
        path
    }
    fn attach(
        &mut self,
        download: impl Fn(
            &str,
            &rad_plugin_manager::Cancellation,
            &rad_plugin_manager::Progress,
        ) -> Result<Vec<u8>, rad_plugin_manager::Error>
        + Send
        + 'static,
    ) {
        let source = PathBuf::from(
            std::env::var_os("RAD_TEST_LAUNCHER_HELPER").expect("build helper before fixtures"),
        );
        let helper = self.input("rad-launcher-helper", &source);
        let mut command = Command::new(helper);
        for (key, part) in [
            ("HOME", "home"),
            ("XDG_RUNTIME_DIR", "runtime"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_DATA_DIRS", "system"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_CONFIG_DIRS", "system"),
            ("XDG_CACHE_HOME", "cache"),
            ("TMPDIR", "tmp"),
        ] {
            command.env(key, self.root.path.join(part));
        }
        command.env("PATH", "/nonexistent-rad-private-bin").env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent-rad-private-bus",
        );
        // Explicit values survive env_clear; never inherit a desktop service endpoint.
        let values: Vec<_> = command
            .get_envs()
            .map(|(k, v)| (k.to_owned(), v.unwrap().to_owned()))
            .collect();
        command.env_clear().envs(values);
        let launcher = rad_launcher::Launcher::spawn(command).unwrap();
        self.server
            .as_mut()
            .unwrap()
            .attach_launcher_with_download(launcher, download)
            .unwrap();
    }
    fn run<T: Send + 'static>(&mut self, work: impl FnOnce(PathBuf) -> T + Send + 'static) -> T {
        let runtime = self.root.path.join("runtime");
        let ended = Arc::new(AtomicBool::new(false));
        struct Announce(Arc<AtomicBool>, PathBuf);
        impl Drop for Announce {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
                let _ = UnixStream::connect(self.1.join("rad/editor.sock"));
            }
        }
        let ended_client = ended.clone();
        let client = thread::spawn(move || {
            let _done = Announce(ended_client, runtime.clone());
            work(runtime)
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut count = 0usize;
        let mut total = 0u128;
        let mut max = 0u128;
        let server = self.server.as_mut().unwrap();
        while !ended.load(Ordering::Acquire) {
            let start = Instant::now();
            server.turn().unwrap();
            let ns = start.elapsed().as_nanos();
            count += 1;
            total += ns;
            max = max.max(ns);
            assert!(Instant::now() < deadline, "owned client scenario watchdog");
            if !ended.load(Ordering::Acquire) {
                server.wait(Duration::from_secs(1)).unwrap();
            }
        }
        emit(
            self.name,
            "turn_summary",
            json!({"count":count,"total_ns":total,"max_ns":max}),
        );
        client.join().unwrap()
    }
    fn stop(&mut self) -> bool {
        if self.stopped {
            return true;
        }
        self.stopped = true;
        let started = Instant::now();
        let server = self.server.as_mut().unwrap();
        server.request_shutdown();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !server.shutdown_complete() && Instant::now() < deadline {
            if server.turn().is_err() || server.wait(Duration::from_millis(100)).is_err() {
                break;
            }
        }
        let clean = server.shutdown_complete();
        self.server.take();
        sample(
            self.name,
            "request_shutdown_to_complete_and_server_drop",
            started,
        );
        clean
    }
}
impl Drop for World {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
fn connect(runtime: &Path) -> Client {
    Client::connect(runtime, Endpoint::Editor).unwrap()
}
fn call(client: &mut Client, operation: &str, arguments: Value) -> Value {
    let response = client.request(operation, arguments).unwrap();
    assert!(response.error.is_none(), "{operation}: {response:?}");
    serde_json::to_value(response.data).unwrap()
}
fn await_value(
    client: &mut Client,
    operation: &str,
    arguments: Value,
    accepts: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let value = call(client, operation, arguments.clone());
        if accepts(&value) {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "observation deadline: {operation}: {value}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
fn healthy(client: &mut Client, name: &str) {
    let started = Instant::now();
    call(client, "status", json!({}));
    sample(name, "healthy_status", started);
}

#[test]
fn uncertain_real_store_return_is_unknown_over_ipc_and_reconciles_without_replay() {
    use crate::storage::fixture::{Control, Fault};
    let control = Control::install(Fault::UncertainReturn);
    let mut world = World::new("store_return_uncertain");
    let mut control = world.run(move |runtime| {
        let mut editor = connect(&runtime);
        let folder = call(&mut editor, "status", json!({}))["active_folder_id"].clone();
        let path = runtime.clone();
        let pending = thread::spawn(move || {
            let mut client = connect(&path);
            client.request("create_button", json!({"folder_id":folder,"position":0,"name":"once"})).unwrap()
        });
        let observed = control.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(observed, rad_store::CommitOutcome::Uncertain { .. }), "the real Store must classify the lost return, not a synthetic Authority callback: {observed:?}");
        healthy(&mut editor, "store_return_uncertain");
        let started = Instant::now();
        let mut control = control; control.release();
        let response = pending.join().unwrap();
        let error = response.error.expect("uncertain request cannot claim applied");
        assert_eq!(error.code, "storage_uncertain");
        assert_eq!(error.outcome, Some(rad_api::Outcome::Unknown));
        sample("store_return_uncertain", "release_to_unknown_response", started);
        let reconciliation = Instant::now();
        await_value(&mut editor, "status", json!({}), |v| v["storage_state"] == "ready");
        await_value(&mut editor, "get", json!({"id":"b1"}), |v| v["fields"]["name"] == "once");
        sample("store_return_uncertain", "unknown_to_reconciled_ready_and_one_change_observed", reconciliation);
        healthy(&mut editor, "store_return_uncertain");
        assert_eq!(control.count(), 1, "reconciliation must not replay the original commit");
        control
    });
    control.release();
    assert!(world.stop());
    control.resources_dropped();
    let store = rad_store::Store::open(&world.root.path.join("store/state.sqlite3")).unwrap();
    assert_eq!(store.load().unwrap().configuration.data().buttons.len(), 1);
    emit(
        world.name,
        "complete",
        json!({"healthy":true,"unknown":true,"real_sqlite":true,"replayed":false,"shutdown_complete":true,"storage_resources_dropped":true}),
    );
}

#[test]
fn lost_real_store_worker_returns_unknown_and_restart_reads_one_change_without_replay() {
    use crate::storage::fixture::{Control, Fault};
    let control = Control::install(Fault::LostAfterCommit);
    let mut world = World::new("store_worker_lost");
    let control = world.run(move |runtime| {
        let mut editor = connect(&runtime);
        let folder = call(&mut editor, "status", json!({}))["active_folder_id"].clone();
        let path = runtime.clone();
        let pending = thread::spawn(move || {
            connect(&path)
                .request(
                    "create_button",
                    json!({"folder_id":folder,"position":0,"name":"once"}),
                )
                .unwrap()
        });
        assert!(matches!(
            control
                .entered
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
            rad_store::CommitOutcome::Applied {
                revision: 2,
                changed: true
            }
        ));
        healthy(&mut editor, "store_worker_lost");
        let started = Instant::now();
        let mut control = control;
        control.release();
        let error = pending
            .join()
            .unwrap()
            .error
            .expect("lost worker cannot promise that its durable change was not applied");
        assert_eq!(error.code, "storage_uncertain");
        assert_eq!(error.outcome, Some(rad_api::Outcome::Unknown));
        sample("store_worker_lost", "release_to_unknown_response", started);
        assert_eq!(
            call(&mut editor, "status", json!({}))["storage_state"],
            "recovery_required"
        );
        healthy(&mut editor, "store_worker_lost");
        assert_eq!(control.count(), 1);
        control
    });
    assert!(world.stop());
    control.resources_dropped();
    world.server = Some(
        Server::new(
            &world.root.path.join("runtime"),
            &world.root.path.join("store"),
            NonZeroU128::new(0xfa018).unwrap(),
        )
        .unwrap(),
    );
    world.stopped = false;
    world.run(|runtime| {
        let mut editor = connect(&runtime);
        assert_eq!(
            call(&mut editor, "get", json!({"id":"b1"}))["fields"]["name"],
            "once"
        );
        assert_eq!(
            call(&mut editor, "status", json!({}))["storage_state"],
            "ready"
        );
    });
    assert!(world.stop());
    let loaded = rad_store::Store::open(&world.root.path.join("store/state.sqlite3"))
        .unwrap()
        .load()
        .unwrap();
    assert_eq!(loaded.configuration.data().buttons.len(), 1);
    emit(
        world.name,
        "complete",
        json!({"unknown":true,"real_sqlite":true,"controlled_worker_exit":true,"replayed":false,"restart_reads_one_change":true,"shutdown_complete":true,"storage_resources_dropped":true}),
    );
}

fn peer_manifest(id: &str) -> Vec<u8> {
    let fields: Vec<_> = (0..3).map(|i| json!({"id":format!("payload{i}"),"label":"Payload","required":false,"kind":{"type":"text","max_bytes":4096}})).collect();
    serde_json::to_vec(&json!({"contract":1,"id":id,"version":{"major":1,"minor":0,"patch":0},
        "host_min":1,"host_max":1,"targets":[rad_plugin_host::TARGET],"capabilities":[],
        "actions":[{"id":"run","label":"Run","schema":{"version":1,"fields":fields},"credential_slots":[]}],"visuals":[]})).unwrap()
}
fn package(world: &World, name: &str, manifest: &[u8], binary: &Path) -> Vec<u8> {
    let input = world.input(name, binary);
    let elf = fs::read(input).unwrap();
    let candidate = rad_plugin_package::Candidate::from_parts(
        manifest,
        &elf,
        rad_plugin_contract::HOST_API,
        rad_plugin_host::TARGET,
    )
    .unwrap();
    let bytes = rad_plugin_package::encode(&candidate);
    let path = world.root.path.join("inputs").join(format!("{name}.radp"));
    fs::write(&path, &bytes).unwrap();
    if profiled() {
        let out = Command::new("/usr/bin/sha256sum")
            .arg(&path)
            .output()
            .unwrap();
        assert!(out.status.success());
        let hash = String::from_utf8(out.stdout).unwrap();
        emit(
            world.name,
            "package",
            json!({"name":name,"sha256":hash.split_whitespace().next().unwrap(),"retained_before_install":true}),
        );
    }
    bytes
}
fn seed(world: &World, packages: &[&[u8]]) {
    let root = world.root.path.join("store/plugins");
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let mut manager = rad_plugin_manager::Manager::create(
        &root,
        rad_plugin_contract::HOST_API,
        rad_plugin_host::TARGET,
    )
    .unwrap();
    for package in packages {
        let mut prepared = Some(
            manager
                .prepare(rad_plugin_manager::Operation::Install, Some(package))
                .unwrap(),
        );
        assert!(matches!(
            manager.commit(&mut prepared).unwrap(),
            rad_plugin_manager::CommitOutcome::Applied
        ));
    }
    // Byte-compare installed ELF files before the Host may execute them. The full
    // private store is retained in profiled runs, not erased by a Scratch guard.
    fn installed(path: &Path, output: &mut Vec<Vec<u8>>) {
        for item in fs::read_dir(path).unwrap() {
            let item = item.unwrap();
            let metadata = fs::symlink_metadata(item.path()).unwrap();
            if metadata.is_dir() {
                installed(&item.path(), output);
            } else if metadata.is_file() {
                let bytes = fs::read(item.path()).unwrap();
                if bytes.starts_with(b"\x7fELF") {
                    output.push(bytes);
                }
            }
        }
    }
    let mut actual = Vec::new();
    installed(&root, &mut actual);
    for bytes in packages {
        let candidate = rad_plugin_package::Candidate::parse(
            bytes,
            rad_plugin_contract::HOST_API,
            rad_plugin_host::TARGET,
        )
        .unwrap();
        assert!(
            actual.iter().any(|elf| elf == candidate.executable()),
            "installed executable must match the retained package before first execution"
        );
    }
}
fn ready(client: &mut Client) {
    await_value(client, "plugin_status", json!({}), |v| {
        v["state"] == "idle" && !v["items"].as_array().unwrap().is_empty()
    });
}
fn button(
    client: &mut Client,
    position: usize,
    name: &str,
    kind: &str,
    parameters: Value,
) -> String {
    let folder = call(client, "status", json!({}))["active_folder_id"].clone();
    call(
        client,
        "create_button",
        json!({"folder_id":folder,"position":position,"name":name}),
    );
    let folder = call(client, "get", json!({"id":folder}));
    let id = folder["fields"]["slots"][position]["id"]
        .as_str()
        .expect("created visible button")
        .to_owned();
    call(
        client,
        "add_action",
        json!({"button_id":id,"type":kind,"parameters":parameters}),
    );
    id
}
fn run_ok(client: &mut Client, id: &str) {
    let run = call(client, "run", json!({"id":id}));
    let result = await_value(
        client,
        "result",
        json!({"execution_id":run["execution_id"]}),
        |v| v["state"] != "running",
    );
    assert_eq!(result["state"], "succeeded", "{result}");
}

#[test]
fn held_downloader_keeps_one_credit_while_cold_plugin_and_core_stay_healthy() {
    let mut world = World::new("downloader_held");
    let echo = package(
        &world,
        "echo",
        &peer_manifest(ECHO),
        &bins().join("examples/host_fault_profile"),
    );
    let other = package(
        &world,
        "download",
        &peer_manifest(BLOCKED),
        &bins().join("examples/host_fault_profile"),
    );
    seed(&world, &[&echo]);
    let (entered, observed) = mpsc::sync_channel(1);
    let (release, permit) = mpsc::sync_channel(1);
    let calls = Arc::new(AtomicUsize::new(0));
    let called = calls.clone();
    world.attach(move |_, _, _| {
        called.fetch_add(1, Ordering::AcqRel);
        let _ = entered.try_send(());
        permit
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| rad_plugin_manager::Error::Io)?;
        Ok(other.clone())
    });
    world.run(move |runtime| {
        let mut editor = connect(&runtime); ready(&mut editor);
        let healthy_button = button(&mut editor, 1, "Healthy", "wait", json!({"milliseconds":0}));
        assert_eq!(call(&mut editor, "plugin_status", json!({}))["items"][0]["host"], "absent", "first execution must really be cold");
        call(&mut editor, "plugin_admin", json!({"operation":"install","plugin_id":null,"source":"https://github.com/owner/fixture/releases/tag/held","trusted":true}));
        observed.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(call(&mut editor, "plugin_status", json!({}))["state"], "preparing");
        let refused = editor.request("plugin_admin", json!({"operation":"install","plugin_id":null,"source":"https://github.com/owner/fixture/releases/tag/held","trusted":true})).unwrap();
        assert_eq!(refused.error.unwrap().code, "resource_exhausted");
        let started = Instant::now();
        let echo_button = button(&mut editor, 0, "Echo", &format!("{ECHO}:run"), json!({"schema_version":1,"values":{}}));
        run_ok(&mut editor, &echo_button); sample("downloader_held", "configure_first_demand_and_other_plugin_result", started);
        healthy(&mut editor, "downloader_held"); run_ok(&mut editor, &healthy_button);
        assert_eq!(calls.load(Ordering::Acquire), 1, "refusal must not create a hidden downloader/replay");
        let started = Instant::now(); release.send(()).unwrap();
        await_value(&mut editor, "plugin_status", json!({}), |v| v["state"] == "idle" && v["items"].as_array().unwrap().len() == 2);
        sample("downloader_held", "release_to_two_installed_observed", started);
        assert_eq!(calls.load(Ordering::Acquire), 1); run_ok(&mut editor, &healthy_button);
    });
    assert!(world.stop());
    emit(
        world.name,
        "complete",
        json!({"healthy":true,"cold_other_plugin":true,"single_credit":true,"download_calls":1,"replayed":false,"shutdown_complete":true,"host_reaped":true}),
    );
}

#[test]
fn non_reading_plugin_retains_real_pipe_output_but_not_unrelated_core_execution() {
    let mut world = World::new("host_non_reader");
    let bytes = package(
        &world,
        "non-reader",
        &peer_manifest(BLOCKED),
        &bins().join("examples/host_fault_profile"),
    );
    seed(&world, &[&bytes]);
    world.attach(|_, _, _| panic!("no network in private fixture"));
    world.run(|runtime| {
        let mut editor = connect(&runtime); ready(&mut editor);
        // The canonical document stores opaque parameters as hex, whose scalar
        // budget is 8192 bytes. Include the JSON wrapper before doubling its size.
        // Sixteen frames exceed a normal x86_64 pipe; write interest proves retention.
        let parameters = json!({"schema_version":1,"values":{"payload0":"x".repeat(4000)}});
        assert!(serde_json::to_vec(&parameters).unwrap().len() * 2 < 8190);
        let blocked = button(&mut editor, 0, "Blocked", &format!("{BLOCKED}:run"), parameters.clone());
        for index in 1..rad_plugin_host::MAX_PLUGIN_JOBS {
            let response = editor.request("add_action", json!({"button_id":blocked,"type":format!("{BLOCKED}:run"),"parameters":parameters})).unwrap();
            assert!(response.error.is_none(), "additional action {index}, params={} bytes: {response:?}", serde_json::to_vec(&parameters).unwrap().len());
        }
        call(&mut editor, "edit", json!({"changes":[{"kind":"set","id":blocked,"field":"mode","value":"parallel"}]}));
        let healthy_button = button(&mut editor, 1, "Healthy", "wait", json!({"milliseconds":0}));
        let execution = call(&mut editor, "run", json!({"id":blocked}))["execution_id"].clone();
        let started = Instant::now(); healthy(&mut editor, "host_non_reader"); run_ok(&mut editor, &healthy_button); sample("host_non_reader", "healthy_core_while_pipe_retained", started);
        assert_eq!(call(&mut editor, "result", json!({"execution_id":execution}))["state"], "running");
        drop(editor);
        let mut again = connect(&runtime);
        assert_eq!(call(&mut again, "result", json!({"execution_id":execution}))["state"], "running", "reconnect must observe, never replay the blocked admission");
        run_ok(&mut again, &healthy_button);
    });
    let host = world
        .server
        .as_ref()
        .unwrap()
        .native
        .as_ref()
        .unwrap()
        .plugin_host();
    assert_eq!(host.state(BLOCKED), Some(rad_plugin_host::State::Ready));
    let generation = host
        .bindings()
        .find(|(manifest, _, _)| manifest.id == BLOCKED)
        .unwrap()
        .1;
    assert!(
        host.io_sources()
            .iter()
            .any(|(g, _, write)| *g == generation && *write),
        "must retain actual pipe output, not just simulate a pending result"
    );
    assert_eq!(
        host.resources().1,
        rad_plugin_host::MAX_PLUGIN_JOBS,
        "client reconnect may not add/replay jobs"
    );
    let started = Instant::now();
    assert!(world.stop());
    sample(
        world.name,
        "shutdown_with_real_queued_pipe_and_reap",
        started,
    );
    emit(
        world.name,
        "complete",
        json!({"healthy":true,"actual_queued_pipe":true,"pending_jobs_before_stop":rad_plugin_host::MAX_PLUGIN_JOBS,"replayed":false,"shutdown_complete":true,"host_reaped":true}),
    );
}

/// Only owned, deterministic D-Bus/fake-player processes. No shell or desktop bus.
struct FixtureChild {
    child: Child,
    lines: mpsc::Receiver<String>,
    output: Arc<std::sync::Mutex<(Vec<String>, bool)>>,
    reader: Option<thread::JoinHandle<()>>,
    reaped: bool,
}
impl FixtureChild {
    fn spawn(
        world: &World,
        executable: &Path,
        arguments: &[String],
        bus: Option<&str>,
        player: bool,
    ) -> Self {
        let mut command = Command::new(executable);
        command
            .args(arguments)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", world.root.path.join("home"))
            .env("XDG_RUNTIME_DIR", world.root.path.join("runtime"))
            .env("XDG_DATA_HOME", world.root.path.join("data"))
            .env("XDG_CONFIG_HOME", world.root.path.join("config"))
            .env("XDG_CACHE_HOME", world.root.path.join("cache"))
            .env("XDG_DATA_DIRS", world.root.path.join("system"))
            .env("XDG_CONFIG_DIRS", world.root.path.join("system"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        if let Some(bus) = bus {
            command.env("DBUS_SESSION_BUS_ADDRESS", bus);
        }
        if player {
            command.env("FAKE_PLAYER_PRIVATE_BUS", "1");
        }
        let parent = std::process::id();
        // SAFETY: the post-fork closure only uses async-signal-safe libc calls.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() as u32 != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::sync_channel(512);
        let output = Arc::new(std::sync::Mutex::new((Vec::new(), false)));
        let captured = output.clone();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                let mut output = captured.lock().unwrap();
                if output.0.len() < 512 {
                    output.0.push(line.clone());
                } else {
                    output.1 = true;
                }
                if sender.try_send(line).is_err() {
                    output.1 = true;
                }
            }
        });
        Self {
            child,
            lines,
            output,
            reader: Some(reader),
            reaped: false,
        }
    }
    fn line(&self) -> String {
        self.lines
            .recv_timeout(Duration::from_secs(5))
            .expect("owned fixture startup/response deadline")
    }
    fn command(&mut self, command: &str) {
        let input = self.child.stdin.as_mut().unwrap();
        writeln!(input, "{command}\nping barrier").unwrap();
        input.flush().unwrap();
        while self.line() != "pong barrier" {}
    }
    fn stop(&mut self, eof: bool) -> bool {
        if self.reaped {
            return true;
        }
        self.child.stdin.take();
        if !eof && self.child.try_wait().unwrap().is_none() {
            // SAFETY: unreaped Child still owns this live PID; no unrelated process is selected.
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGTERM);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let graceful = self.child.try_wait().unwrap().is_some();
        if !graceful {
            // SAFETY: this Child was launched as the leader of its own process group.
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
        }
        self.child.wait().unwrap();
        self.reaped = true;
        self.reader.take().unwrap().join().unwrap();
        graceful
    }
    fn no_methods(&self) {
        let output = self.output.lock().unwrap();
        assert!(!output.1, "fixture output budget/observation lost");
        assert!(
            !output.0.iter().any(|line| line.starts_with("call ")),
            "visuals/core actions must not execute or replay MPRIS methods"
        );
    }
}
impl Drop for FixtureChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.stop(false);
        }
    }
}

#[test]
fn retained_official_converter_does_not_hold_text_visual_core_or_server_and_reaps() {
    let mut world = World::new("official_converter_held");
    let plugin = package(
        &world,
        "held-converter-fixture",
        &fs::read(repo().join("plugins/media/manifest.json")).unwrap(),
        &repo().join("target/plugins/release/examples/held-converter-fixture"),
    );
    seed(&world, &[&plugin]);
    let daemon = world.input("dbus-daemon", Path::new("/usr/bin/dbus-daemon"));
    let player = world.input(
        "fake-player",
        &repo().join("target/plugins/release/examples/fake_player"),
    );
    let config = world.root.path.join("bus.conf");
    let address = format!(
        "unix:path={}",
        world.root.path.join("runtime/private-bus").display()
    );
    fs::write(&config, format!("<busconfig><type>session</type><listen>{address}</listen><auth>EXTERNAL</auth><policy context=\"default\"><allow send_destination=\"*\"/><allow receive_sender=\"*\"/><allow own=\"*\"/></policy></busconfig>")).unwrap();
    let mut bus = FixtureChild::spawn(
        &world,
        &daemon,
        &[
            format!("--config-file={}", config.display()),
            "--nofork".into(),
            "--print-address".into(),
        ],
        None,
        false,
    );
    let address = bus.line();
    assert!(address.starts_with(&format!(
        "unix:path={},",
        world.root.path.join("runtime/private-bus").display()
    )));
    let mut cover = FixtureChild::spawn(
        &world,
        &player,
        &["org.mpris.MediaPlayer2.cover".into()],
        Some(&address),
        true,
    );
    let mut text = FixtureChild::spawn(
        &world,
        &player,
        &["org.mpris.MediaPlayer2.text".into()],
        Some(&address),
        true,
    );
    assert_eq!(cover.line(), "ready");
    assert_eq!(text.line(), "ready");
    cover.command("status Playing");
    text.command("status Playing");
    let png = world.root.path.join("inputs/cover.png");
    fs::copy(repo().join("packaging/rad.png"), &png).unwrap();
    cover.command(&format!("art file://{}", png.display()));
    let listener = UnixListener::bind(world.root.path.join("runtime/cover.fixture")).unwrap();
    listener.set_nonblocking(true).unwrap();
    world.attach(|_, _, _| panic!("no network in private converter fixture"));
    let mut text = world.run(move |runtime| {
        let configured = thread::spawn(move || {
            let end = Instant::now() + Duration::from_secs(5);
            let mut gate = loop { match listener.accept() {
                Ok((gate,_)) => break gate,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => { assert!(Instant::now() < end); thread::sleep(Duration::from_millis(10)); }
                Err(error) => panic!("{error}"),
            } };
            gate.set_read_timeout(Some(Duration::from_secs(5))).unwrap(); gate.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            gate.write_all(&(address.len() as u32).to_be_bytes()).unwrap(); gate.write_all(address.as_bytes()).unwrap();
            gate
        });
        let mut editor = connect(&runtime); ready(&mut editor);
        let text_button = button(&mut editor, 0, "Text", "wait", json!({"milliseconds":0}));
        call(&mut editor, "edit", json!({"changes":[{"kind":"set","id":text_button,"field":"visual","value":{"type":"org.rad.media#now_playing","parameters":{"schema_version":1,"values":{"player":"text"}}}}]}));
        await_value(&mut editor, "plugin_visual", json!({"id":text_button}), |v| v["availability"] == "current" && v["text"] == "Playing");
        let mut gate = configured.join().unwrap();
        let cover_button = button(&mut editor, 1, "Cover", "wait", json!({"milliseconds":0}));
        call(&mut editor, "edit", json!({"changes":[{"kind":"set","id":cover_button,"field":"visual","value":{"type":"org.rad.media#now_playing","parameters":{"schema_version":1,"values":{"player":"cover"}}}}]}));
        let mut entered = [0]; gate.read_exact(&mut entered).expect("the official converter must accept and retain real cover bytes"); assert_eq!(entered, [b'E']);
        // A changed value proves progress of the unrelated official reader, not
        // merely a cached visual renewed by the SDK heartbeat.
        text.command("status Paused");
        await_value(&mut editor, "plugin_visual", json!({"id":text_button}), |v| v["availability"] == "current" && v["text"] == "Paused");
        for _ in 0..3 {
            let started = Instant::now();
            assert_eq!(call(&mut editor, "plugin_visual", json!({"id":text_button}))["text"], "Paused");
            healthy(&mut editor, "official_converter_held"); run_ok(&mut editor, &text_button);
            sample("official_converter_held", "text_and_core_while_converter_owned", started);
        }
        assert_eq!(call(&mut editor, "plugin_visual", json!({"id":cover_button}))["image"], false);
        let started = Instant::now(); gate.write_all(b"R").unwrap();
        let image = await_value(&mut editor, "plugin_visual", json!({"id":cover_button}), |v| v["availability"] == "current" && v["image"] == true);
        let normalized = call(&mut editor, "plugin_visual_read", json!({"id":cover_button,"revision":image["revision"],"offset":"0"}));
        assert!(normalized["hex"].as_str().unwrap().starts_with("89504e470d0a1a0a"));
        sample("official_converter_held", "release_to_official_jpeg_and_runtime_png_observed", started);
        run_ok(&mut editor, &text_button);
        text
    });
    assert!(world.stop());
    assert!(cover.stop(true));
    assert!(text.stop(true));
    assert!(bus.stop(false));
    cover.no_methods();
    text.no_methods();
    emit(
        world.name,
        "complete",
        json!({"official_media_plugin":true,"official_converter":true,"private_mpris":true,"healthy":true,"jpeg_normalized_to_png":true,
        "mpris_methods":0,"replayed":false,"shutdown_complete":true,"host_reaped":true,"private_bus_players_reaped":true}),
    );
}
