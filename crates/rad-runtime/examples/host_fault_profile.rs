//! Instrument the real bounded Host with an echo peer and a peer that stops reading.
//! No Server, hardware, external integration or GUI is involved.
use rad_plugin_contract::{Manifest, Outcome, Parameters};
use rad_plugin_host::{Host, State};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::{fd::AsRawFd, unix::fs::DirBuilderExt},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

const ECHO: &str = "org.rad.fault.echo";
const BLOCKED: &str = "org.rad.fault.no-reader";

fn emit(value: Value) {
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &value).unwrap();
    writeln!(output).unwrap();
    output.flush().unwrap();
}

fn peer(id: &str) {
    let mut input = std::io::stdin().lock();
    loop {
        let mut header = [0; 4];
        if input.read_exact(&mut header).is_err() {
            return;
        }
        let size = u32::from_be_bytes(header) as usize;
        assert!((1..=65536).contains(&size));
        let mut payload = vec![0; size];
        input.read_exact(&mut payload).unwrap();
        let request: Value = serde_json::from_slice(&payload).unwrap();
        let hello = request["message"]["kind"] == "hello";
        let message = if hello {
            json!({"kind":"ready","plugin_id":id,"plugin_version":{"major":1,"minor":0,"patch":0},"capabilities":[]})
        } else {
            assert_eq!(request["message"]["kind"], "invoke");
            json!({"kind":"result","outcome":{"status":"success"}})
        };
        let response = serde_json::to_vec(
            &json!({"protocol":1,"correlation":request["correlation"],"message":message}),
        )
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
        if hello && id == BLOCKED {
            // Deliberately no more reads. Host owns this process and kills/reaps it.
            loop {
                std::thread::park();
            }
        }
    }
}

fn manifest(id: &str) -> Manifest {
    let fields: Vec<_> = (0..3)
        .map(|i| {
            json!({
                "id":format!("payload{i}"),"label":"Payload","required":false,
                "kind":{"type":"text","max_bytes":4096}
            })
        })
        .collect();
    serde_json::from_value(json!({
        "contract":1,"id":id,"version":{"major":1,"minor":0,"patch":0},
        "host_min":1,"host_max":1,"targets":[rad_plugin_host::TARGET],"capabilities":[],
        "actions":[{"id":"run","label":"Run","schema":{"version":1,"fields":fields},"credential_slots":[]}],
        "visuals":[]
    })).unwrap()
}

fn now(epoch: Instant) -> u64 {
    epoch.elapsed().as_millis().try_into().unwrap()
}

fn wait_io(host: &Host) {
    let mut fds: Vec<_> = host
        .io_sources()
        .into_iter()
        .map(|(_, fd, write)| libc::pollfd {
            fd: fd.as_raw_fd(),
            events: if write { libc::POLLOUT } else { libc::POLLIN },
            revents: 0,
        })
        .collect();
    if fds.is_empty() {
        std::thread::sleep(Duration::from_millis(1));
        return;
    }
    // All descriptors are borrowed from this Host and remain valid until its next mutation.
    let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 500) };
    if result < 0 {
        assert_eq!(
            std::io::Error::last_os_error().kind(),
            std::io::ErrorKind::Interrupted
        );
    }
}

fn ready(host: &mut Host, id: &str, epoch: Instant) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.state(id) != Some(State::Ready) {
        host.pump(now(epoch)).unwrap();
        assert!(Instant::now() < deadline, "private handshake timed out");
        if host.state(id) != Some(State::Ready) {
            wait_io(host);
        }
    }
}

fn healthy(host: &mut Host, generation: u64, epoch: Instant) -> u128 {
    let started = Instant::now();
    let params: Parameters =
        serde_json::from_value(json!({"schema_version":1,"values":{}})).unwrap();
    let key = host
        .invoke(ECHO, generation, "run", params, now(epoch) + 2000)
        .unwrap();
    let deadline = started + Duration::from_secs(2);
    loop {
        host.pump(now(epoch)).unwrap();
        while let Some((correlation, outcome)) = host.take_completion() {
            if correlation == key {
                assert_eq!(outcome, Outcome::Success);
                return started.elapsed().as_nanos();
            }
        }
        assert!(
            Instant::now() < deadline,
            "healthy peer blocked by unrelated no-reader"
        );
        wait_io(host);
    }
}

fn phase(host: &mut Host, generation: u64, epoch: Instant, name: &str, seconds: u64) {
    emit(json!({"event":"phase_begin","phase":name}));
    let started = Instant::now();
    let mut samples = Vec::new();
    while started.elapsed() < Duration::from_secs(seconds) {
        samples.push(healthy(host, generation, epoch));
        std::thread::sleep(Duration::from_millis(50));
    }
    emit(json!({"event":"phase_end","phase":name}));
    for ns in samples {
        emit(json!({"event":"sample","phase":name,"operation":"healthy_peer_roundtrip","ns":ns}));
    }
}

fn private_root(root: &Path) {
    assert!(root.is_absolute());
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    assert!(
        root.parent().unwrap().canonicalize().unwrap().starts_with(
            repository
                .join("target/performance")
                .canonicalize()
                .unwrap()
        )
    );
    std::fs::DirBuilder::new().mode(0o700).create(root).unwrap();
}

fn main() {
    let invocation = std::env::args().next().unwrap();
    if invocation == ECHO || invocation == BLOCKED {
        peer(&invocation);
        return;
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(args.len(), 2, "host_fault_profile NEW_ROOT SECONDS");
    let root = PathBuf::from(&args[0]);
    let seconds: u64 = args[1].parse().unwrap();
    assert!((1..=10).contains(&seconds));
    private_root(&root);
    let epoch = Instant::now();
    let executable = std::env::current_exe().unwrap();
    let mut host = Host::default();
    let generation = host
        .activate(Command::new(&executable), manifest(ECHO), now(epoch))
        .unwrap();
    ready(&mut host, ECHO, epoch);
    phase(&mut host, generation, epoch, "before", seconds);
    let blocked_generation = host
        .activate(Command::new(&executable), manifest(BLOCKED), now(epoch))
        .unwrap();
    ready(&mut host, BLOCKED, epoch);
    let params: Parameters = serde_json::from_value(json!({"schema_version":1,"values":{
        "payload0":"x".repeat(4096),"payload1":"y".repeat(4096),"payload2":"z".repeat(4096)
    }}))
    .unwrap();
    for _ in 0..rad_plugin_host::MAX_PLUGIN_JOBS {
        host.invoke(
            BLOCKED,
            blocked_generation,
            "run",
            params.clone(),
            now(epoch) + 30_000,
        )
        .unwrap();
    }
    host.pump(now(epoch)).unwrap();
    assert!(
        host.io_sources()
            .iter()
            .any(|(g, _, write)| *g == blocked_generation && *write),
        "fault must retain queued pipe output"
    );
    emit(
        json!({"event":"fault_state","scenario":"host_no_reader","retained_jobs":host.resources().1,"queued_write_interest":true}),
    );
    phase(&mut host, generation, epoch, "during", seconds);
    let started = Instant::now();
    host.retire(BLOCKED);
    let deadline = started + Duration::from_secs(5);
    while host.process_id(BLOCKED).is_some() {
        host.pump(now(epoch)).unwrap();
        while host.take_completion().is_some() {}
        assert!(Instant::now() < deadline, "no-reader not reaped");
        if host.process_id(BLOCKED).is_some() {
            wait_io(&host);
        }
    }
    emit(
        json!({"event":"sample","operation":"no_reader_retire_and_reap","ns":started.elapsed().as_nanos()}),
    );
    phase(&mut host, generation, epoch, "after", seconds);
    host.shutdown();
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.resources().0 != 0 {
        host.pump(now(epoch)).unwrap();
        while host.take_completion().is_some() {}
        assert!(Instant::now() < deadline, "echo peer not reaped");
        if host.resources().0 != 0 {
            wait_io(&host);
        }
    }
    emit(
        json!({"event":"complete","clean_shutdown":true,"scope":"real_host_component_not_server","probe_period_ms":50}),
    );
}
