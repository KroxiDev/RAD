//! A worker thread that dies must neither leave the editor busy nor repeat its job.

use super::*;
use crate::worker::Respawner;
use std::sync::{Arc, Mutex};

fn event(connected: bool, result: Result<Reply, crate::session::Error>) -> Event {
    Event {
        plugin_apply: false,
        plugin_admin: false,
        plugin_manifest: false,
        plugin_dependencies: false,
        plugin_form: false,
        plugin_visual: false,
        connected,
        transfer_job: false,
        palette: Palette::default(),
        result,
    }
}

fn app() -> App {
    let root = std::env::temp_dir().join(format!("rad-editor-worker-loss-{}", std::process::id()));
    let mut app = App::with_preferences_path(root.clone(), root.join("no-theme"), None);
    app.es = crate::preferences::Language::En;
    app
}
/// A worker whose thread dies on its first job, as a panic inside `run` does.
fn dying() -> Worker<Job, Event> {
    Worker::spawn(|_job: Job| -> Event { panic!("synthetic worker death") })
}
fn pump_until(app: &mut App, ctx: &egui::Context, mut done: impl FnMut(&App) -> bool) {
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while !done(app) {
        assert!(Instant::now() < deadline, "the editor never settled");
        app.pump(ctx);
        std::thread::yield_now();
    }
}

#[test]
fn a_mutation_lost_with_its_worker_is_resolved_by_rereading_and_never_repeated() {
    let mut app = app();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    // Replacement workers record what they are given, and reach the runtime for real.
    app.worker_respawn = Respawner::new(move || {
        let record = record.clone();
        Worker::spawn(move |job: Job| {
            record.lock().unwrap().push(match job {
                Job::Connect => "connect",
                Job::Refresh { .. } => "refresh",
                Job::Request(Request::Undo {}) => "undo",
                Job::Request(_) => "read",
                _ => "other",
            });
            match job {
                Job::Connect => event(true, Ok(Reply::Connected)),
                _ => event(true, Err(crate::session::Error::protocol())),
            }
        })
    });
    app.worker = dying();
    app.connected = true;
    app.send(Request::Undo {});
    assert!(app.worker.busy());
    let ctx = egui::Context::default();
    pump_until(&mut app, &ctx, |_| {
        seen.lock().unwrap().contains(&"refresh")
    });
    for _ in 0..50 {
        app.pump(&ctx);
        std::thread::yield_now();
    }
    let seen = seen.lock().unwrap();
    assert_eq!(seen[..2], ["connect", "refresh"]);
    assert!(!seen.contains(&"undo"), "the lost mutation was repeated");
    assert!(
        app.resolving_unknown,
        "the lost mutation is settled by reading"
    );
}
