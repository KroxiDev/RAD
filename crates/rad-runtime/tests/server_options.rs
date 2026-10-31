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

const PLUGIN: &str = "org.rad.test.options";
const REGISTERED_ORIGIN: &str = "https://github.com/owner/repo";

// Measurement-only instrumentation: absent from production and silent unless
// explicitly requested. Existing assertions remain the acceptance contract.
#[cfg(test)]
mod profile {
    use super::*;

    use std::{collections::BTreeSet, io::Write, sync::OnceLock};

    fn enabled() -> bool {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED
            .get_or_init(|| std::env::var_os("RAD_PROFILE_INTEGRATED").is_some_and(|v| v == "1"))
    }
    pub fn active(scenario: &str) -> bool {
        enabled()
            && matches!(
                scenario,
                "slow" | "crash" | "invalid" | "leaves" | "capacity"
            )
    }
    pub fn emit(scenario: &str, kind: &str, mut value: Value) {
        if !active(scenario) {
            return;
        }
        value["suite"] = json!("integrated_faults");
        value["scenario"] = json!(scenario);
        value["kind"] = json!(kind);
        let mut stdout = std::io::stdout().lock();
        // libtest may have left its "test name ... " prefix on this line.
        if kind == "metadata" {
            writeln!(stdout).unwrap();
        }
        writeln!(stdout, "{value}").unwrap();
        stdout.flush().unwrap();
    }
    pub fn sample(scenario: &str, operation: &str, elapsed: Duration) {
        emit(
            scenario,
            "sample",
            json!({"operation":operation,"ns":elapsed.as_nanos()}),
        );
    }
    pub fn timed<T>(scenario: &str, operation: &str, work: impl FnOnce() -> T) -> T {
        if !active(scenario) {
            return work();
        }
        let started = Instant::now();
        let result = work();
        sample(scenario, operation, started.elapsed());
        result
    }
    pub fn children() -> BTreeSet<u32> {
        std::fs::read_dir("/proc/self/task")
            .unwrap()
            .map(|thread| thread.unwrap().path().join("children"))
            .flat_map(|path| {
                // An owned thread may finish between readdir and open. Other
                // errors must not masquerade as proof that a child was reaped.
                let text = match std::fs::read_to_string(path) {
                    Ok(text) => text,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                    Err(error) => panic!("cannot inspect own children: {error}"),
                };
                text.split_whitespace()
                    .map(|pid| pid.parse().unwrap())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
    #[derive(Default)]
    pub struct Turns {
        count: usize,
        total: u128,
        max: u128,
    }
    impl Turns {
        pub fn turn(&mut self, server: &mut Server, scenario: &str) {
            let started = active(scenario).then(Instant::now);
            server.turn().unwrap();
            if let Some(started) = started {
                let ns = started.elapsed().as_nanos();
                self.count += 1;
                self.total += ns;
                self.max = self.max.max(ns);
            }
        }
        pub fn emit(&self, scenario: &str, phase: &str) {
            emit(
                scenario,
                "turn_summary",
                json!({"phase":phase,
                "count":self.count,"total_ns":self.total,"max_ns":self.max}),
            );
        }
    }
}

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
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&plugins)
        .unwrap();
    let mut manager =
        rad_plugin_manager::Manager::create(&plugins, 2, rad_plugin_host::TARGET).unwrap();
    let manifest = include_bytes!("../../rad-plugin-host/test-plugins/options-manifest.json");
    let executable = std::fs::read(binaries().join("rad-plugin-options-fixture")).unwrap();
    let candidate = rad_plugin_package::Candidate::from_parts(
        manifest,
        &executable,
        2,
        rad_plugin_host::TARGET,
    )
    .unwrap();
    let bytes = rad_plugin_package::encode(&candidate);
    let origin = rad_plugin_manager::GithubSource::parse(REGISTERED_ORIGIN).unwrap();
    let mut prepared = Some(
        manager
            .prepare_from_source(
                rad_plugin_manager::Operation::Install,
                Some(&bytes),
                Some(&origin),
                None,
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
    let _turn = TURN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let children_before = profile::active(name).then(profile::children);
    profile::emit(
        name,
        "metadata",
        json!({
        "scope":"server_client_plugin_fixture",
        "path":["rad_client","Unix socket Transport","Server::turn","Authority","Host","options fixture process"],
        "clock":"Instant wall monotonic","pid":std::process::id(),
        "limits":["owned synthetic plugin and private helper, no hardware or real backends",
            "client-observed intervals include scheduling and test polling",
            "serving turn aggregate includes setup and healthy traffic",
            "shutdown_complete checks helper/plugin reaping; direct own children cross-check",
            "no integrated Store-write uncertainty, downloader or converter fault measured"]}),
    );
    let root =
        Directory(std::env::temp_dir().join(format!("rad-opt-{}-{name}", std::process::id())));
    for sub in ["", "runtime", "home", "data", "system", "config", "cache"] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(root.0.join(sub))
            .unwrap();
    }
    let mut server = Server::new(
        &root.0.join("runtime"),
        &root.0.join("store"),
        NonZeroU128::new(0x4f50_0000 + name.len() as u128).unwrap(),
    )
    .unwrap();
    seed(&root.0.join("store"));
    let helper = || {
        let mut command = std::process::Command::new(
            std::env::var("RAD_TEST_LAUNCHER_HELPER").expect("runner builds the private helper"),
        );
        command
            .env_clear()
            .env("HOME", root.0.join("home"))
            .env("XDG_DATA_HOME", root.0.join("data"))
            .env("XDG_DATA_DIRS", root.0.join("system"))
            .env("XDG_CONFIG_HOME", root.0.join("config"))
            .env("XDG_CACHE_HOME", root.0.join("cache"))
            .env("XDG_RUNTIME_DIR", root.0.join("runtime"))
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
    let runtime = root.0.join("runtime");
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
    let mut serving_turns = profile::Turns::default();
    while !finished.load(std::sync::atomic::Ordering::Acquire) {
        serving_turns.turn(&mut server, name);
        OPEN_REQUESTS.store(
            server.pending_requests(),
            std::sync::atomic::Ordering::Release,
        );
        assert!(Instant::now() < end, "scenario watchdog");
        server.wait(Duration::from_secs(5)).unwrap();
    }
    let result = client.join();
    serving_turns.emit(name, "serving");
    let stop_started = Instant::now();
    server.request_shutdown();
    let end = Instant::now() + Duration::from_secs(10);
    let mut shutdown_turns = profile::Turns::default();
    while !server.shutdown_complete() {
        shutdown_turns.turn(&mut server, name);
        assert!(Instant::now() < end, "runtime did not stop");
        server.wait(Duration::from_millis(10)).unwrap();
    }
    let shutdown_ns = stop_started.elapsed().as_nanos();
    shutdown_turns.emit(name, "shutdown");
    if let Some(before) = children_before {
        let after = profile::children();
        assert_eq!(
            after, before,
            "the integrated fixture must reap all owned children"
        );
        profile::emit(
            name,
            "complete",
            json!({"client_ok":result.is_ok(),
            "shutdown_complete":server.shutdown_complete(),"shutdown_ns":shutdown_ns,
            "children_reaped":true,"children_before":before,"children_after":after}),
        );
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

fn code(client: &mut Client, op: &str, args: Value) -> String {
    let response = client.request(op, args).unwrap();
    response
        .error
        .unwrap_or_else(|| panic!("{op} was accepted: {:?}", response.data))
        .code
        .to_string()
}

/// Waits for installed metadata; on-demand processes need not be running.
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

fn values(scenario: &str) -> Value {
    json!({"schema_version":1,"values":{"scenario":scenario,"group":"a","member":"a2"}})
}

/// Button `b1` in the active folder holding the `pick` action `a1`.
fn pick_button(editor: &mut Client, parameters: Value) {
    let folder = call(editor, "status", json!({}))["active_folder_id"].clone();
    call(
        editor,
        "create_button",
        json!({"folder_id":folder,"position":0,"name":"Pick"}),
    );
    call(
        editor,
        "add_action",
        json!({"button_id":"b1","type":format!("{PLUGIN}:pick"),"parameters":parameters}),
    );
}

fn await_visual(editor: &mut Client, id: &str, ready: impl Fn(&Value) -> bool) -> Value {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let visual = call(editor, "plugin_visual", json!({"id":id}));
        if ready(&visual) {
            return visual;
        }
        assert!(Instant::now() < end, "visual never settled: {visual}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn ask(client: &mut Client, args: Value) -> Value {
    call(client, "options", args)
}

fn listed(answer: &Value) -> Vec<(String, String)> {
    answer["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["value"].as_str().unwrap().to_owned(),
                i["label"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
    values
        .iter()
        .map(|(v, l)| ((*v).into(), (*l).into()))
        .collect()
}

/// Everything the saved configuration of `b1`/`a1` consists of, as any client can read it,
/// without the revision counter (an execution moves it without touching the configuration).
fn snapshot(client: &mut Client) -> Value {
    let status = call(client, "status", json!({}));
    json!({
        "history": status["history_operations"],
        "permissions": status["global_permissions"],
        "button": call(client, "get", json!({"id":"b1"}))["fields"],
        "action": call(client, "get", json!({"id":"a1"}))["fields"],
        "parameters": call(client, "plugin_configuration", json!({"id":"a1","cursor":null})),
    })
}

fn revision(client: &mut Client) -> Value {
    call(client, "status", json!({}))["view_revision"].clone()
}

fn finished(client: &mut Client, execution: &Value) -> Value {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let result = call(client, "result", json!({"execution_id":execution}));
        if result["state"] != "running" {
            return result;
        }
        assert!(Instant::now() < end, "execution never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn slow_question() -> Value {
    json!({"type":format!("{PLUGIN}:pick"),"field":"group","cursor":null,
           "context":{"scenario":"slow"}})
}

fn normal_question() -> Value {
    json!({"type":format!("{PLUGIN}:pick"),"field":"group","cursor":null,
           "context":{"scenario":"normal"}})
}

/// Asks on its own connection and thread; reports the code (or "ok") and how long it took.
fn ask_in_background(
    runtime: &Path,
    question: Value,
) -> std::thread::JoinHandle<(String, Duration)> {
    let mut client = connect(runtime, Endpoint::Editor);
    std::thread::spawn(move || {
        let started = Instant::now();
        let response = client.request("options", question).unwrap();
        let outcome = response
            .error
            .map_or("ok".to_owned(), |e| e.code.to_string());
        (outcome, started.elapsed())
    })
}

#[test]
fn a_slow_plugin_times_out_near_two_seconds_while_the_runtime_keeps_serving() {
    world("slow", |runtime| {
        let mut editor = connect(&runtime, Endpoint::Editor);
        ready(&mut editor);
        pick_button(&mut editor, values("normal"));
        let waiting = ask_in_background(&runtime, slow_question());
        std::thread::sleep(Duration::from_millis(300));
        // Pending, yet other requests, executions and even other questions are answered.
        let served = Instant::now();
        profile::timed("slow", "healthy_status", || {
            call(&mut editor, "status", json!({}))
        });
        let run = call(&mut editor, "run", json!({"id":"a1"}));
        assert_eq!(
            finished(&mut editor, &run["execution_id"])["state"],
            "succeeded"
        );
        assert_eq!(
            ask(&mut editor, normal_question())["availability"],
            "available"
        );
        assert!(
            served.elapsed() < Duration::from_millis(1000),
            "{:?}",
            served.elapsed()
        );
        assert!(
            !waiting.is_finished(),
            "the slow question was not waited for"
        );
        profile::sample("slow", "healthy_status_run_and_query", served.elapsed());
        let (outcome, elapsed) = waiting.join().unwrap();
        assert_eq!(outcome, "options_timeout");
        profile::sample("slow", "options_request_to_timeout_observed", elapsed);
        assert!(
            (Duration::from_millis(1900)..Duration::from_millis(3000)).contains(&elapsed),
            "{elapsed:?}"
        );
        // The plugin is fine and the saved action untouched.
        assert_eq!(
            ask(&mut editor, normal_question())["availability"],
            "available"
        );
    });
}

#[test]
fn the_late_reply_of_an_expired_question_never_answers_the_next_one() {
    world("late", |runtime| {
        let mut editor = connect(&runtime, Endpoint::Editor);
        ready(&mut editor);
        let first = ask_in_background(&runtime, slow_question());
        assert_eq!(first.join().unwrap().0, "options_timeout");
        // The first reply is still due (at 3 s) and arrives while this question waits. A
        // reply taken for this one would end it early, as an available list.
        let second = ask_in_background(&runtime, slow_question());
        let (outcome, elapsed) = second.join().unwrap();
        assert_eq!(outcome, "options_timeout");
        assert!(elapsed >= Duration::from_millis(1900), "{elapsed:?}");
    });
}

fn counter(execution: &Value) -> u64 {
    execution
        .as_str()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn asking_for_options_needs_no_execute_and_changes_nothing_anywhere() {
    world("effects", |runtime| {
        let mut editor = connect(&runtime, Endpoint::Editor);
        ready(&mut editor);
        pick_button(&mut editor, values("normal"));
        call(
            &mut editor,
            "edit",
            json!({"changes":[{"kind":"set","id":"b1","field":"visual","value":{
                "type":format!("{PLUGIN}#probe"),
                "parameters":{"schema_version":1,"values":{"scenario":"normal","group":"a"}}}}]}),
        );
        await_visual(&mut editor, "b1", |v| v["availability"] == "current");
        // A guard button to count executions: queries must not consume an execution id.
        let folder = call(&mut editor, "status", json!({}))["active_folder_id"].clone();
        call(
            &mut editor,
            "create_button",
            json!({"folder_id":folder,"position":1,"name":"Guard"}),
        );
        call(
            &mut editor,
            "add_action",
            json!({"button_id":"b2","type":"wait","parameters":{"milliseconds":0}}),
        );
        call(
            &mut editor,
            "set_permissions",
            json!({"target":"b1","value":{"own":{"consult":true,"modify":true,"execute":false}}}),
        );
        let mut automation = connect(&runtime, Endpoint::Automation);
        assert_eq!(
            code(&mut automation, "run", json!({"id":"a1"})),
            "permission_denied",
            "Execute is really denied"
        );
        let first = call(&mut editor, "run", json!({"id":"b2"}));
        finished(&mut editor, &first["execution_id"]);
        let (before, published) = (snapshot(&mut editor), revision(&mut editor));

        for client in [&mut editor, &mut automation] {
            for question in [
                json!({"id":"a1","field":"group","cursor":null}),
                json!({"id":"a1","field":"member","cursor":null,"context":{"group":"b"}}),
                json!({"id":"b1","field":"group","cursor":null}),
                normal_question(),
            ] {
                assert_eq!(ask(client, question)["availability"], "available");
            }
        }
        std::thread::sleep(Duration::from_millis(600));

        assert_eq!(
            snapshot(&mut editor),
            before,
            "configuration, history, permissions"
        );
        assert_eq!(revision(&mut editor), published);
        let next = call(&mut editor, "run", json!({"id":"b2"}));
        assert_eq!(
            counter(&next["execution_id"]),
            counter(&first["execution_id"]) + 1
        );
        // The plugin was only asked, never told to act.
        let visual = await_visual(&mut editor, "b1", |v| v["availability"] == "current");
        assert_eq!(visual["text"], "invocations:0");
    });
}

#[test]
fn plugin_settings_are_snapshotted_for_options_and_replace_visual_subscriptions() {
    world("settings-snapshots", |runtime| {
        let mut editor = connect(&runtime, Endpoint::Editor);
        ready(&mut editor);
        let set = |editor: &mut Client, port| {
            call(
                editor,
                "set_plugin_settings",
                json!({"plugin_id":PLUGIN,"parameters":{"schema_version":1,"values":{"port":port}}}),
            );
        };
        set(&mut editor, 5555);
        let mut query_client = connect(&runtime, Endpoint::Editor);
        let query = std::thread::spawn(move || ask(&mut query_client, normal_question()));
        let until = Instant::now() + Duration::from_secs(3);
        while OPEN_REQUESTS.load(std::sync::atomic::Ordering::Acquire) == 0 {
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        set(&mut editor, 6666);
        assert_eq!(
            listed(&query.join().unwrap()),
            pairs(&[("port5555", "Port 5555")])
        );
        assert_eq!(
            listed(&ask(&mut editor, normal_question())),
            pairs(&[("port6666", "Port 6666")])
        );
        pick_button(&mut editor, values("normal"));
        call(
            &mut editor,
            "edit",
            json!({"changes":[{"kind":"set","id":"b1","field":"visual","value":{"type":format!("{PLUGIN}#probe"),"parameters":{"schema_version":1,"values":{"scenario":"normal","group":"a"}}}}]}),
        );
        await_visual(&mut editor, "b1", |v| {
            v["availability"] == "current" && v["text"] == "port6666"
        });
        set(&mut editor, 7777);
        await_visual(&mut editor, "b1", |v| {
            v["availability"] == "current" && v["text"] == "port7777"
        });
    });
}
