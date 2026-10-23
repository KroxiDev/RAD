//! Headless instrument of this package's real Worker/Respawner, never a GUI.
#[allow(dead_code)]
#[path = "../src/worker.rs"]
mod worker;

use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use worker::{Polled, Respawner, Worker};

fn emit(value: Value) {
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &value).unwrap();
    writeln!(output).unwrap();
    output.flush().unwrap();
}

fn phase(name: &str, seconds: u64, healthy: &mut Worker<u64, u64>) {
    emit(json!({"event":"phase_begin","phase":name}));
    let started = Instant::now();
    let mut measurements = Vec::new();
    while started.elapsed() < Duration::from_secs(seconds) {
        let t = Instant::now();
        healthy.submit(10).unwrap();
        assert_eq!(healthy.receive(), Polled::Ready(11));
        measurements.push(t.elapsed().as_nanos());
        std::thread::sleep(Duration::from_millis(50));
    }
    emit(json!({"event":"phase_end","phase":name}));
    for ns in measurements {
        emit(json!({"event":"sample","phase":name,"operation":"healthy_worker_answer","ns":ns}));
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
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(args.len(), 2, "fault_profile NEW_ROOT SECONDS");
    let root = PathBuf::from(&args[0]);
    let seconds: u64 = args[1].parse().unwrap();
    assert!((1..=10).contains(&seconds));
    private_root(&root);
    std::panic::set_hook(Box::new(|_| {})); // Expected synthetic panic, no private data in logs.
    let mut healthy = Worker::spawn(|job: u64| job + 1);
    let mut failed = Worker::spawn(|job: u64| {
        assert_ne!(job, 0, "synthetic worker death");
        job
    });
    let mut replacement = Respawner::new(|| Worker::spawn(|job: u64| job + 1));
    phase("before", seconds, &mut healthy);
    let t = Instant::now();
    failed.submit(0).unwrap();
    failed.enqueue(7).unwrap();
    assert_eq!(failed.receive(), Polled::Failed { unsent: true });
    assert!(!failed.busy());
    emit(
        json!({"event":"sample","operation":"worker_failure_detected","ns":t.elapsed().as_nanos(),"clock":"wall_monotonic_includes_thread_exit_and_channel"}),
    );
    phase("during", seconds, &mut healthy);
    assert_eq!(
        failed.poll(),
        Polled::Pending,
        "death is reported exactly once"
    );
    let t = Instant::now();
    assert!(replacement.replace(&mut failed, || {}));
    failed.submit(10).unwrap();
    assert_eq!(failed.receive().ready(), Some(11));
    replacement.answered();
    assert!(!replacement.gave_up());
    emit(
        json!({"event":"sample","operation":"worker_replacement_answer","ns":t.elapsed().as_nanos(),"queued_old_job_replayed":false}),
    );
    phase("after", seconds, &mut healthy);
    drop(failed);
    drop(healthy);
    drop(replacement);
    // Worker owns no JoinHandle. Observe actual thread retirement before declaring completion.
    let deadline = Instant::now() + Duration::from_secs(2);
    while std::fs::read_dir("/proc/self/task").unwrap().count() != 1 {
        assert!(
            Instant::now() < deadline,
            "instrument worker thread not retired"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    emit(
        json!({"event":"complete","clean_shutdown":true,"scope":"real_worker_component_not_gui_or_server","probe_period_ms":50}),
    );
}
