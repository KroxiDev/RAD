//! A plugin must not outlive the runtime that launched it, even when the runtime dies
//! without running any cleanup (ADR 0013).
use rad_plugin_contract::*;
use rad_plugin_host::Host;
use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const PARENT: &str = "RAD_HOST_PARENT_DEATH_PARENT";
const PID_PREFIX: &str = "PLUGIN_PID=";

fn manifest() -> Manifest {
    Manifest {
        resources: vec![],
        capabilities: vec![],
        settings: None,
        lifecycle: Default::default(),
        contract: 1,
        id: "org.rad.sleeper".into(),
        version: Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        host_min: 1,
        host_max: 1,
        targets: vec!["x86_64-unknown-linux-gnu".into()],
        actions: vec![Definition {
            gestures: vec![Gesture::Press],
            response_timeout: None,
            id: "run".into(),
            label: "run".into(),
            schema: Schema {
                version: 1,
                fields: vec![],
            },
            credential_slots: vec![],
        }],
        visuals: vec![],
        translations: Default::default(),
    }
}

fn is_dead(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat
            .rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')),
    }
}

/// Kills whatever the test started, so a failing assertion leaks no process.
struct Cleanup {
    parent: std::process::Child,
    plugin: Option<i32>,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.parent.kill();
        let _ = self.parent.wait();
        if let Some(pid) = self.plugin {
            // SAFETY: plain signal to a pid this test learned from its own child.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

#[test]
fn plugin_dies_with_the_runtime_that_launched_it() {
    if std::env::var_os(PARENT).is_some() {
        // Stand-in runtime: `sleep` never reads stdin nor answers the hello.
        let mut host = Host::default();
        host.activate(
            {
                let mut command = Command::new("/bin/sleep");
                command.arg("60");
                command
            },
            manifest(),
            0,
        )
        .unwrap();
        println!(
            "{PID_PREFIX}{}",
            host.process_id("org.rad.sleeper").unwrap()
        );
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    let mut parent = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "plugin_dies_with_the_runtime_that_launched_it",
            "--nocapture",
        ])
        .env(PARENT, "1")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = parent.stdout.take().unwrap();
    let mut cleanup = Cleanup {
        parent,
        plugin: None,
    };
    // Read on a thread so a stand-in that never reports cannot hang the test.
    let (sender, reported) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let pid = BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .find_map(|line| line.strip_prefix(PID_PREFIX).and_then(|p| p.parse().ok()));
        let _ = sender.send(pid);
    });
    let pid: i32 = reported
        .recv_timeout(Duration::from_secs(10))
        .ok()
        .flatten()
        .expect("stand-in runtime reported no plugin pid");
    cleanup.plugin = Some(pid);
    assert!(
        !is_dead(pid),
        "plugin should be running before the runtime dies"
    );
    // SAFETY: signal to the child process this test spawned.
    unsafe { libc::kill(cleanup.parent.id() as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(3);
    while !is_dead(pid) {
        assert!(
            Instant::now() < deadline,
            "plugin {pid} outlived its runtime"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
