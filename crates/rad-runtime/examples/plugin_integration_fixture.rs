// Private headless instrument, not a product administrative CLI.
// Only --live downloads the explicitly authorized public synthetic fixture.
use rad_client::connection::{Client, Endpoint};
use rad_runtime::server::Server;
use serde_json::{Value, json};
use std::{
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
const ID: &str = "org.rad.fixture";
fn call(c: &mut Client, op: &str, args: Value) -> Value {
    let r = c
        .request(op, args)
        .unwrap_or_else(|e| panic!("private IPC {op}: {e:?}"));
    assert!(
        r.error.is_none(),
        "request {op} failed: {:?}",
        r.error.as_ref().map(|e| &e.code)
    );
    r.data
}
fn ready(c: &mut Client) -> Value {
    let end = Instant::now() + Duration::from_secs(90);
    loop {
        let d = call(c, "plugin_status", json!({}));
        assert!(d["state"] != "uncertain", "durable outcome uncertain");
        if d["state"] == "idle" {
            return d;
        }
        assert!(d["state"] != "failed", "admin failed: {}", d["error"]);
        assert!(Instant::now() < end, "admin watchdog");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn admin(c: &mut Client, op: &str, version: Option<u32>) -> Value {
    let source = version
        .map(|v| format!("https://github.com/KroxiDev/rad-plugin-fixture/releases/tag/v{v}.0.0"));
    call(
        c,
        "plugin_admin",
        json!({"operation":op,"plugin_id":if op=="install"{None}else{Some(ID)},"source":source,"trusted":true}),
    );
    let d = ready(c);
    println!("admin,{op},confirmed");
    d
}
fn wait_execution(c: &mut Client, id: &Value) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let d = call(c, "result", json!({"execution_id":id}));
        if d["state"] == "succeeded" {
            return;
        }
        assert_eq!(d["state"], "running", "fixture execution");
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_micros(100));
    }
}
fn seed(root: &Path, bin: &Path) {
    let dir = root.join("store/plugins");
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    let mut m = rad_plugin_manager::Manager::create(&dir, 1, rad_plugin_host::TARGET).unwrap();
    let manifest = include_bytes!("../../rad-plugin-host/fixture/manifest-v1.json");
    let executable = std::fs::read(bin.join("rad-plugin-fixture-v1")).unwrap();
    let candidate = rad_plugin_package::Candidate::from_parts(
        manifest,
        &executable,
        1,
        rad_plugin_host::TARGET,
    )
    .unwrap();
    let bytes = rad_plugin_package::encode(&candidate);
    let mut p = Some(
        m.prepare(rad_plugin_manager::Operation::Install, Some(&bytes))
            .unwrap(),
    );
    assert!(matches!(
        m.commit(&mut p).unwrap(),
        rad_plugin_manager::CommitOutcome::Applied
    ));
}
// Enumerate only this instrument's threads and their direct children, never system PIDs.
fn own_children() -> Vec<u32> {
    let mut ids = vec![];
    for t in std::fs::read_dir("/proc/self/task").unwrap() {
        let path = t.unwrap().path().join("children");
        if let Ok(s) = std::fs::read_to_string(path) {
            ids.extend(s.split_whitespace().map(|s| s.parse::<u32>().unwrap()));
        }
    }
    ids.sort();
    ids.dedup();
    ids
}
fn sample(pid: u32) -> (u64, u64, u64) {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("own plugin alive");
    let fields: Vec<_> = s.rsplit_once(") ").unwrap().1.split_whitespace().collect();
    let ticks = fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap();
    let start = fields[19].parse().unwrap();
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let rss = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    (ticks, start, rss)
}
fn percentile(values: &[u128], p: usize) -> u128 {
    let mut v = values.to_vec();
    v.sort_unstable();
    v[(v.len() * p).div_ceil(100).saturating_sub(1)]
}
fn scenario(root: PathBuf, live: bool, measure: bool, idle: bool, baseline: Vec<u32>) {
    let mut c = Client::connect(&root.join("runtime"), Endpoint::Editor).unwrap();
    ready(&mut c);
    if live {
        admin(&mut c, "install", Some(1));
    }
    let d = ready(&mut c);
    assert_eq!(d["items"][0]["host"], "absent");
    assert!(
        own_children().into_iter().all(|p| baseline.contains(&p)),
        "unused installed metadata must not spawn a process"
    );
    let folder = call(&mut c, "status", json!({}))["active_folder_id"].clone();
    call(
        &mut c,
        "create_button",
        json!({"folder_id":folder,"position":0,"name":"Fixture"}),
    );
    call(
        &mut c,
        "add_action",
        json!({"button_id":"b1","type":"org.rad.fixture:run","parameters":{"schema_version":1,"values":{"mode":"echo"}}}),
    );
    call(
        &mut c,
        "edit",
        json!({"changes":[{"kind":"set","id":"b1","field":"visual","value":{"type":"org.rad.fixture#status","parameters":{"schema_version":1,"values":{"mode":"echo"}}}}]}),
    );
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        let v = call(&mut c, "plugin_visual", json!({"id":"b1"}));
        if v["availability"] == "current" {
            assert_eq!(v["text"], "v1");
            break;
        }
        assert!(Instant::now() < end, "visual current");
        std::thread::sleep(Duration::from_millis(1));
    }
    let children: Vec<_> = own_children()
        .into_iter()
        .filter(|p| !baseline.contains(p))
        .collect();
    assert_eq!(children.len(), 1, "exactly one new plugin process");
    let plugin = children[0];
    println!("owned_plugin_pid,{plugin}");
    assert!(
        !call(&mut c, "plugin_catalog", json!({"cursor":null}))["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        call(
            &mut c,
            "schema",
            json!({"type":"org.rad.fixture:run","cursor":null})
        )["schema_version"],
        1
    );
    let mut automation = Client::connect(&root.join("runtime"), Endpoint::Automation).unwrap();
    let denied = automation
        .request(
            "plugin_admin",
            json!({"operation":"uninstall","plugin_id":ID}),
        )
        .unwrap();
    assert_eq!(denied.error.unwrap().code, "permission_denied");
    if measure {
        // No IPC during idle. The instrument and runtime share a process; plugin PID is separate.
        let clk = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        assert!(clk > 0);
        println!("clk_tck,{clk}");
        if idle {
            for window in 0..3 {
                let epoch = Instant::now();
                let (_, identity, _) = sample(plugin);
                for n in 0..=60 {
                    let target = epoch + Duration::from_secs(n);
                    if let Some(wait) = target.checked_duration_since(Instant::now()) {
                        std::thread::sleep(wait);
                    }
                    let (ticks, start, rss) = sample(plugin);
                    assert_eq!(start, identity, "PID reuse");
                    println!(
                        "idle,{window},{n},{},{ticks},{rss}",
                        epoch.elapsed().as_nanos()
                    );
                }
            }
        }
        // Transport deliberately expires idle clients; reconnect, never replay.
        c = Client::connect(&root.join("runtime"), Endpoint::Editor).unwrap();
        let mut admission = vec![];
        let mut complete = vec![];
        let mut status = vec![];
        for i in 0..1000 {
            let t = Instant::now();
            let d = call(&mut c, "run", json!({"id":"b1"}));
            admission.push(t.elapsed().as_nanos());
            wait_execution(&mut c, &d["execution_id"]);
            complete.push(t.elapsed().as_nanos());
            let t = Instant::now();
            call(&mut c, "status", json!({}));
            status.push(t.elapsed().as_nanos());
            println!("latency,{i},{},{},{}", admission[i], complete[i], status[i]);
        }
        for (name, v) in [
            ("admission", admission),
            ("echo_complete", complete),
            ("status_nonpersist", status),
        ] {
            println!(
                "nearest_rank_ns,{name},p50={},p95={},p99={}",
                percentile(&v, 50),
                percentile(&v, 95),
                percentile(&v, 99)
            );
        }
    }
    if live {
        // Wait precedes the plugin action in a separate button: reservation covers future dispatch.
        call(
            &mut c,
            "create_button",
            json!({"folder_id":folder,"position":1,"name":"Wait guard"}),
        );
        call(
            &mut c,
            "add_action",
            json!({"button_id":"b2","type":"wait","parameters":{"milliseconds":500}}),
        );
        call(
            &mut c,
            "add_action",
            json!({"button_id":"b2","type":"org.rad.fixture:run","parameters":{"schema_version":1,"values":{"mode":"echo"}}}),
        );
        let d = call(&mut c, "run", json!({"id":"b2"}));
        let denied=c.request("plugin_admin",json!({"operation":"update","plugin_id":ID,"source":"https://github.com/KroxiDev/rad-plugin-fixture/releases/tag/v2.0.0","trusted":true})).unwrap();
        assert_eq!(denied.error.unwrap().code, "plugin_in_use");
        wait_execution(&mut c, &d["execution_id"]);
        for (op, version, major) in [("update", Some(2), 2), ("rollback", None, 1)] {
            let d = admin(&mut c, op, version);
            assert_eq!(d["items"][0]["selected"]["major"], major);
        }
    }
    // Give an ordinary Wait and a Regresar their own plugin visual, then remove
    // only the plugin. Appearance failure must not disable either native action.
    let parent = call(&mut c, "status", json!({}))["active_folder_id"].clone();
    call(
        &mut c,
        "create_folder",
        json!({"parent_id":parent,"position":3,"name":"Native safety"}),
    );
    let entry = call(&mut c, "get", json!({"id":parent}))["fields"]["slots"][3]["id"].clone();
    let nav = call(&mut c, "run", json!({"id":entry}));
    wait_execution(&mut c, &nav["execution_id"]);
    let child = call(&mut c, "status", json!({}))["active_folder_id"].clone();
    let back = call(&mut c, "get", json!({"id":child}))["fields"]["slots"][0]["id"].clone();
    call(
        &mut c,
        "create_button",
        json!({"folder_id":child,"position":1,"name":"Healthy Wait"}),
    );
    let healthy = call(&mut c, "get", json!({"id":child}))["fields"]["slots"][1]["id"].clone();
    call(
        &mut c,
        "add_action",
        json!({"button_id":healthy,"type":"wait","parameters":{"milliseconds":0}}),
    );
    for id in [&healthy, &back] {
        call(
            &mut c,
            "edit",
            json!({"changes":[{"kind":"set","id":id,"field":"visual","value":{"type":"org.rad.fixture#status","parameters":{"schema_version":1,"values":{"mode":"echo"}}}}]}),
        );
    }
    let before = call(
        &mut c,
        "plugin_configuration",
        json!({"id":"a1","cursor":null}),
    );
    admin(&mut c, "uninstall", None);
    assert_eq!(
        call(
            &mut c,
            "plugin_configuration",
            json!({"id":"a1","cursor":null})
        ),
        before
    );
    assert_eq!(
        call(&mut c, "plugin_visual", json!({"id":healthy}))["availability"],
        "unavailable"
    );
    let d = call(&mut c, "run", json!({"id":healthy}));
    wait_execution(&mut c, &d["execution_id"]);
    let d = call(&mut c, "run", json!({"id":back}));
    wait_execution(&mut c, &d["execution_id"]);
    assert_eq!(
        call(&mut c, "status", json!({}))["active_folder_id"],
        parent
    );
    if live {
        let d = admin(&mut c, "reinstall", Some(1));
        assert_eq!(d["items"][0]["host"], "ready");
    }
    println!("fixture,passed,live={live},measure={measure}");
}
fn pipe_latencies(bin: &Path) {
    let mut host = rad_plugin_host::Host::default();
    let manifest = serde_json::from_slice(include_bytes!(
        "../../rad-plugin-host/fixture/manifest-v1.json"
    ))
    .unwrap();
    host.activate(
        std::process::Command::new(bin.join("rad-plugin-fixture-v1")),
        manifest,
        0,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while host.state(ID) != Some(rad_plugin_host::State::Ready) {
        host.pump(0).unwrap();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_micros(100));
    }
    let generation = host.bindings().next().unwrap().1;
    let mut dispatch = vec![];
    let mut response = vec![];
    for i in 0..1000 {
        let params =
            serde_json::from_value(json!({"schema_version":1,"values":{"mode":"echo"}})).unwrap();
        let t = Instant::now();
        let key = host.invoke(ID, generation, "run", params, 10000).unwrap();
        dispatch.push(t.elapsed().as_nanos());
        loop {
            host.pump(0).unwrap();
            if let Some((received, outcome)) = host.take_completion() {
                assert_eq!(key, received);
                assert_eq!(outcome, rad_plugin_contract::Outcome::Success);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_micros(100));
        }
        response.push(t.elapsed().as_nanos());
        println!("pipe_latency,{i},{},{}", dispatch[i], response[i]);
    }
    for (name, v) in [("pipe_dispatch", dispatch), ("pipe_response", response)] {
        println!(
            "nearest_rank_ns,{name},p50={},p95={},p99={}",
            percentile(&v, 50),
            percentile(&v, 95),
            percentile(&v, 99)
        );
    }
    host.shutdown();
    while host.resources().0 > 0 {
        host.pump(0).unwrap();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert!(
        args.iter()
            .all(|a| a == "--live" || a == "--measure" || a == "--latency-only")
    );
    let live = args.iter().any(|a| a == "--live");
    let idle = args.iter().any(|a| a == "--measure");
    let measure = idle || args.iter().any(|a| a == "--latency-only");
    let root = std::env::temp_dir().join(format!("rad-plugin-integration-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .expect("exclusive private fixture root");
    for sub in ["runtime", "home", "data", "system", "config", "cache"] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join(sub))
            .unwrap();
    }
    println!("fixture_root,{}", root.display());
    let exe = std::env::current_exe().unwrap();
    let bin = exe.parent().unwrap().parent().unwrap().to_path_buf();
    let mut server = Server::new(
        &root.join("runtime"),
        &root.join("store"),
        std::num::NonZeroU128::new(871).unwrap(),
    )
    .unwrap();
    if !live {
        seed(&root, &bin);
    }
    let mut cmd = std::process::Command::new(bin.join("rad-launcher-helper"));
    cmd.env_clear()
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
    let launcher = rad_launcher::Launcher::spawn(cmd).unwrap();
    let baseline = own_children();
    server.attach_launcher(launcher).unwrap();
    let client = std::thread::spawn(move || scenario(root, live, measure, idle, baseline));
    let end = Instant::now() + Duration::from_secs(600);
    while !client.is_finished() {
        server.turn().unwrap();
        assert!(Instant::now() < end);
        server.wait(Duration::from_millis(10)).unwrap();
    }
    let result = client.join();
    server.request_shutdown();
    let end = Instant::now() + Duration::from_secs(5);
    while !server.shutdown_complete() {
        server.turn().unwrap();
        assert!(Instant::now() < end, "residual retained");
        server.wait(Duration::from_millis(10)).unwrap();
    }
    result.unwrap();
    if measure {
        pipe_latencies(&bin);
    }
}
