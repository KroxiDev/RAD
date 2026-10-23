//! Isolated measurement instrument, not the product runtime or an official plugin.
//! Usage: runtime_profile NEW_ROOT {0|1|4} SECONDS; no hardware or GUI is attached.
//! The client runs in a thread of this process, so its cost is part of the sample.
use rad_client::connection::{Client, Endpoint};
use rad_runtime::server::Server;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    num::NonZeroU128,
    os::{fd::AsRawFd, unix::fs::DirBuilderExt, unix::net::UnixStream},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

const PREFIX: &str = "org.rad.profile.";
const HOT_SAMPLES: usize = 100;
const LOAD_SAMPLES: usize = 1000;

fn emit(value: Value) {
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &value).unwrap();
    writeln!(output).unwrap();
    output.flush().unwrap();
}

// This intentionally inert synthetic plugin has no polling or external integration.
// A 60-second visual lease keeps each demanded process alive over the measured phases.
fn plugin(id: &str) {
    assert!(id.starts_with(PREFIX));
    let mut input = std::io::stdin().lock();
    loop {
        let mut header = [0; 4];
        if input.read_exact(&mut header).is_err() {
            return;
        }
        let length = u32::from_be_bytes(header) as usize;
        assert!((1..=65536).contains(&length));
        let mut bytes = vec![0; length];
        input.read_exact(&mut bytes).unwrap();
        let request: Value = serde_json::from_slice(&bytes).unwrap();
        let message = match request["message"]["kind"].as_str() {
            Some("hello") => json!({
                "kind":"ready", "plugin_id":id,
                "plugin_version":{"major":1,"minor":0,"patch":0},
                "capabilities":request["message"]["capabilities"]
            }),
            Some("invoke") => json!({"kind":"result","outcome":{"status":"success"}}),
            Some("subscribe") => json!({
                "kind":"visual", "valid_for_ms":60000,
                "state":{"availability":"current","content":{"text":"profile","png_hex":null,"color":null}}
            }),
            Some("unsubscribe") => continue,
            _ => return,
        };
        let response = serde_json::to_vec(&json!({
            "protocol":1,"correlation":request["correlation"],"message":message
        }))
        .unwrap();
        let mut output = std::io::stdout().lock();
        if output
            .write_all(&(response.len() as u32).to_be_bytes())
            .and_then(|_| output.write_all(&response))
            .and_then(|_| output.flush())
            .is_err()
        {
            return;
        }
    }
}

fn private_dir(path: &Path) {
    std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
}

fn seed(root: &Path, executable: &Path, count: usize) {
    if count == 0 {
        return;
    }
    let plugins = root.join("store/plugins");
    private_dir(&plugins);
    let mut manager = rad_plugin_manager::Manager::create(
        &plugins,
        rad_plugin_contract::HOST_API,
        rad_plugin_host::TARGET,
    )
    .unwrap();
    let executable = std::fs::read(executable).unwrap();
    for index in 0..count {
        let manifest = serde_json::to_vec(&json!({
            "contract":1,"id":format!("{PREFIX}{index}"),
            "version":{"major":1,"minor":0,"patch":0},
            "host_min":1,"host_max":1,"targets":[rad_plugin_host::TARGET],
            "capabilities":["visual_freshness"],
            "actions":[{"id":"run","label":"Run","schema":{"version":1,"fields":[]},"credential_slots":[]}],
            "visuals":[{"id":"status","label":"Status","schema":{"version":1,"fields":[]},"credential_slots":[]}]
        }))
        .unwrap();
        let candidate = rad_plugin_package::Candidate::from_parts(
            &manifest,
            &executable,
            rad_plugin_contract::HOST_API,
            rad_plugin_host::TARGET,
        )
        .unwrap();
        let mut prepared = Some(
            manager
                .prepare(
                    rad_plugin_manager::Operation::Install,
                    Some(&rad_plugin_package::encode(&candidate)),
                )
                .unwrap(),
        );
        assert!(matches!(
            manager.commit(&mut prepared).unwrap(),
            rad_plugin_manager::CommitOutcome::Applied
        ));
    }
}

fn call(client: &mut Client, operation: &str, args: Value) -> Value {
    let response = client.request(operation, args).unwrap();
    assert!(
        response.error.is_none(),
        "{operation}: {:?}",
        response.error
    );
    serde_json::to_value(response.data).unwrap()
}

fn ready(client: &mut Client, count: usize) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = call(client, "plugin_status", json!({}));
        if status["state"] == "idle" && status["items"].as_array().unwrap().len() == count {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "plugin metadata not ready: {status}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

// The completion sample includes one or more real IPC result queries, not physical
// switch latency. No timer is inserted between result queries.
fn execute(client: &mut Client, button: &str) -> (u128, u128) {
    let started = Instant::now();
    let receipt = call(client, "run", json!({"id":button}));
    let admitted = started.elapsed().as_nanos();
    let deadline = started + Duration::from_secs(10);
    loop {
        let result = call(
            client,
            "result",
            json!({"execution_id":receipt["execution_id"]}),
        );
        if result["state"] == "succeeded" {
            return (admitted, started.elapsed().as_nanos());
        }
        assert!(
            result["state"] == "running" || result["state"] == "waiting",
            "execution did not succeed: {result}"
        );
        assert!(Instant::now() < deadline, "execution timeout");
    }
}

enum Transition {
    Begin(&'static str),
    End,
    Finished,
}

struct Control {
    transition: Transition,
    acknowledge: mpsc::SyncSender<()>,
}

struct Signals {
    commands: mpsc::Sender<Control>,
    socket: UnixStream,
}

impl Signals {
    fn send(&mut self, transition: Transition) {
        let (acknowledge, acknowledged) = mpsc::sync_channel(0);
        self.commands
            .send(Control {
                transition,
                acknowledge,
            })
            .unwrap();
        self.socket.write_all(&[1]).unwrap();
        acknowledged.recv_timeout(Duration::from_secs(10)).unwrap();
    }
}

fn scenario(root: &Path, count: usize, seconds: u64, signals: &mut Signals) {
    let mut client = Client::connect(&root.join("runtime"), Endpoint::Editor).unwrap();
    let status = ready(&mut client, count);
    assert!(
        status["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["host"] == "absent")
    );
    let folder = call(&mut client, "status", json!({}))["active_folder_id"].clone();
    let mut buttons = Vec::new();
    for index in 0..count.max(1) {
        call(
            &mut client,
            "create_button",
            json!({"folder_id":folder,"position":index,"name":format!("Profile {index}")}),
        );
        let button = format!("b{}", index + 1);
        let (kind, parameters) = if count == 0 {
            ("wait".into(), json!({"milliseconds":0}))
        } else {
            (
                format!("{PREFIX}{index}:run"),
                json!({"schema_version":1,"values":{}}),
            )
        };
        call(
            &mut client,
            "add_action",
            json!({"button_id":button,"type":kind,"parameters":parameters}),
        );
        // No visual exists yet: metadata is installed but the plugin starts cold.
        let (admission, completion) = execute(&mut client, &button);
        for (operation, ns) in [
            ("run_admission_cold", admission),
            ("run_complete_cold", completion),
        ] {
            emit(json!({"event":"sample","operation":operation,"ns":ns,"plugin_index":index}));
        }
        if count > 0 {
            call(
                &mut client,
                "edit",
                json!({"changes":[{"kind":"set","id":button,"field":"visual","value":{
                    "type":format!("{PREFIX}{index}#status"),
                    "parameters":{"schema_version":1,"values":{}}
                }}]}),
            );
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if call(&mut client, "plugin_visual", json!({"id":button}))["availability"]
                    == "current"
                {
                    break;
                }
                assert!(Instant::now() < deadline, "profile visual not current");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        buttons.push(button);
    }
    for index in 0..HOT_SAMPLES {
        let button = &buttons[index % buttons.len()];
        let (admission, completion) = execute(&mut client, button);
        for (operation, ns) in [
            ("run_admission_hot", admission),
            ("run_complete_hot", completion),
        ] {
            emit(json!({"event":"sample","operation":operation,"ns":ns,"iteration":index}));
        }
    }
    std::thread::sleep(Duration::from_secs(2));
    emit(
        json!({"event":"ready","plugins":count,"runtime_pid":std::process::id(),
        "client":"in-process thread; its allocations/CPU are included",
        "plugin":"inert echo fixture with a single 60s visual lease; not official plugin cost"}),
    );

    signals.send(Transition::Begin("idle"));
    // No client IPC or timer wakes the server during this interval. The instrument's
    // registered socket wakes it exactly once to mark the end of the phase.
    std::thread::sleep(Duration::from_secs(seconds));
    signals.send(Transition::End);

    let mut samples = Vec::with_capacity(LOAD_SAMPLES * 3);
    signals.send(Transition::Begin("load"));
    let started = Instant::now();
    let deadline = started + Duration::from_secs(seconds);
    let mut iterations = 0;
    while Instant::now() < deadline {
        let button = &buttons[iterations % buttons.len()];
        let query = Instant::now();
        call(&mut client, "status", json!({}));
        let query = query.elapsed().as_nanos();
        let (admission, completion) = execute(&mut client, button);
        if iterations < LOAD_SAMPLES {
            samples.push(("status_load", query));
            samples.push(("run_admission_load", admission));
            samples.push(("run_complete_load", completion));
        }
        iterations += 1;
    }
    let wall = started.elapsed().as_nanos();
    signals.send(Transition::End);
    // Buffer samples until the phase ends: stdout serialization does not dominate
    // the measured load. The bounded allocation is part of this instrument, and
    // only the first LOAD_SAMPLES iterations are retained as individual samples.
    for (operation, ns) in samples {
        emit(json!({"event":"sample","operation":operation,"ns":ns}));
    }
    emit(
        json!({"event":"load_summary","iterations":iterations,"wall_ns":wall,
            "sampled_iterations":iterations.min(LOAD_SAMPLES),"sample_policy":"first 1000 load iterations"}),
    );
}

#[derive(Default)]
struct Turns {
    count: u64,
    total: u128,
    maximum: u128,
}

impl Turns {
    fn record(&mut self, elapsed: u128) {
        self.count += 1;
        self.total += elapsed;
        self.maximum = self.maximum.max(elapsed);
    }
}

fn drain(socket: &mut UnixStream) {
    let mut bytes = [0; 64];
    loop {
        match socket.read(&mut bytes) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return,
            Err(error) => panic!("control wake: {error}"),
        }
    }
}

fn monotonic_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes one valid timespec; CLOCK_MONOTONIC is fixed.
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) },
        0
    );
    u64::try_from(time.tv_sec)
        .unwrap()
        .checked_mul(1_000_000_000)
        .unwrap()
        .checked_add(u64::try_from(time.tv_nsec).unwrap())
        .unwrap()
}

fn main() {
    let invocation = std::env::args().next().unwrap();
    let args: Vec<_> = std::env::args().skip(1).collect();
    if invocation.starts_with(PREFIX) {
        plugin(&invocation);
        return;
    }
    if args.first().is_some_and(|arg| arg == "--plugin") {
        assert_eq!(args.len(), 2, "--plugin ID");
        plugin(&args[1]);
        return;
    }
    assert_eq!(args.len(), 3, "runtime_profile NEW_ROOT {{0|1|4}} SECONDS");
    let root = PathBuf::from(&args[0]);
    let root = if root.is_absolute() {
        root
    } else {
        std::env::current_dir().unwrap().join(root)
    };
    let count: usize = args[1].parse().unwrap();
    let seconds: u64 = args[2].parse().unwrap();
    assert!([0, 1, 4].contains(&count));
    assert!((1..=20).contains(&seconds));
    private_dir(&root); // Never reuse or alter a pre-existing configuration.
    for directory in ["runtime", "home", "data", "system", "config", "cache"] {
        private_dir(&root.join(directory));
    }
    let executable = std::env::current_exe().unwrap();
    let bin = executable.parent().unwrap().parent().unwrap();
    let mut server = Server::new(
        &root.join("runtime"),
        &root.join("store"),
        NonZeroU128::new(0x4_7072_6f66).unwrap(),
    )
    .unwrap();
    seed(&root, &executable, count);
    let mut helper = Command::new(bin.join("rad-launcher-helper"));
    helper
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
    server
        .attach_launcher(rad_launcher::Launcher::spawn(helper).unwrap())
        .unwrap();
    let (mut wake, writer) = UnixStream::pair().unwrap();
    wake.set_nonblocking(true).unwrap();
    server
        .register_worker_io(
            &mut mio::unix::SourceFd(&wake.as_raw_fd()),
            mio::Interest::READABLE,
        )
        .unwrap();
    let (commands, received) = mpsc::channel();
    let mut signals = Signals {
        commands,
        socket: writer,
    };
    let client = std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scenario(&root, count, seconds, &mut signals);
        }));
        signals.send(Transition::Finished);
        result
    });
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut phase = None;
    let mut turns = Turns::default();
    let mut phase_started = Instant::now();
    let mut finishing = false;
    loop {
        drain(&mut wake);
        while let Ok(control) = received.try_recv() {
            match control.transition {
                Transition::Begin(name) => {
                    assert!(phase.is_none());
                    turns = Turns::default();
                    phase_started = Instant::now();
                    phase = Some(name);
                    emit(
                        json!({"event":"phase_begin","phase":name,"at_monotonic_ns":monotonic_ns()}),
                    );
                }
                Transition::End => {
                    let name = phase.take().expect("an active measurement phase");
                    emit(
                        json!({"event":"phase_end","phase":name,"at_monotonic_ns":monotonic_ns()}),
                    );
                    emit(
                        json!({"event":"turn_summary","phase":name,"count":turns.count,
                        "total_ns":turns.total,"max_ns":turns.maximum,
                        "wall_ns":phase_started.elapsed().as_nanos()}),
                    );
                }
                Transition::Finished => {
                    assert!(phase.is_none());
                    finishing = true;
                    server.request_shutdown();
                }
            }
            control.acknowledge.send(()).unwrap();
        }
        let before = Instant::now();
        server.turn().unwrap();
        if phase.is_some() {
            turns.record(before.elapsed().as_nanos());
        }
        if finishing && server.shutdown_complete() {
            break;
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("profile deadline");
        server.wait(remaining).unwrap();
    }
    let result = client.join().unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    emit(json!({"event":"complete","clean_shutdown":true}));
}
