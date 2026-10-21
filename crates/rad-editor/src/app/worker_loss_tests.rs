//! A worker thread that dies must neither leave the editor busy nor repeat its job.
use super::*;
use crate::worker::Respawner;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

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
fn unavailable() -> Event {
    event(
        false,
        Err(crate::session::Error {
            code: "runtime_unavailable".into(),
            cause: None,
            source_change: None,
            unknown: false,
        }),
    )
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

#[test]
fn a_command_waiting_behind_a_lost_job_is_reported_as_not_applied() {
    let mut app = app();
    // The replacement holds its first job, so the message of the loss stays on screen.
    let (hold, held) = std::sync::mpsc::channel::<()>();
    let held = Mutex::new(Some(held));
    app.worker_respawn = Respawner::new(move || {
        let held = held.lock().unwrap().take();
        Worker::spawn(move |_: Job| {
            if let Some(held) = &held {
                let _ = held.recv();
            }
            unavailable()
        })
    });
    app.worker = dying();
    app.connected = true;
    app.worker.submit(Job::Plugins).unwrap();
    app.send(Request::Undo {});
    let ctx = egui::Context::default();
    pump_until(&mut app, &ctx, |app| app.message_is_error);
    assert_eq!(app.message, tr(app.es, "Not applied — try again"));
    drop(hold);
}

#[test]
fn deaths_in_a_row_stop_being_replaced_and_leave_the_editor_disconnected() {
    let mut app = app();
    let created = Arc::new(AtomicUsize::new(0));
    let count = created.clone();
    app.worker_respawn = Respawner::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
        dying()
    });
    app.worker = dying();
    app.connected = true;
    app.send(Request::Undo {});
    let ctx = egui::Context::default();
    pump_until(&mut app, &ctx, |app| app.worker_respawn.gave_up());
    for _ in 0..50 {
        app.pump(&ctx);
        std::thread::yield_now();
    }
    assert_eq!(created.load(Ordering::SeqCst), 3, "no loop of replacements");
    assert!(!app.worker.busy());
    assert!(!app.connected);
    assert!(app.attempted, "the bar offers Retry");
}

#[test]
fn reconnect_after_giving_up_recreates_the_worker_and_restarts_the_count() {
    let mut app = app();
    let created = Arc::new(AtomicUsize::new(0));
    let count = created.clone();
    app.worker_respawn = Respawner::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
        dying()
    });
    app.worker = dying();
    app.connected = true;
    app.send(Request::Undo {});
    let ctx = egui::Context::default();
    pump_until(&mut app, &ctx, |app| app.worker_respawn.gave_up());
    app.send(Request::Undo {});
    assert_eq!(
        app.message,
        tr(app.es, "Not applied — try again"),
        "a gone worker refuses commands instead of pretending to be busy"
    );
    app.reconnect(&ctx);
    assert_eq!(created.load(Ordering::SeqCst), 4, "the click made a worker");
    pump_until(&mut app, &ctx, |app| app.worker_respawn.gave_up());
    assert_eq!(created.load(Ordering::SeqCst), 7, "a fresh count of three");
}

#[test]
fn reconnect_also_revives_the_credential_and_option_workers_that_gave_up() {
    let mut app = app();
    let made = Arc::new(AtomicUsize::new(0));
    let options = made.clone();
    app.options_respawn = Respawner::new(move || {
        options.fetch_add(1, Ordering::SeqCst);
        Worker::spawn(|_job: OptionsJob| -> OptionsEvent { panic!("never asked") })
    });
    let credentials = made.clone();
    app.credential_respawn = Respawner::new(move || {
        credentials.fetch_add(1, Ordering::SeqCst);
        Worker::spawn(|_job: credentials::Job| -> credentials::Event { panic!("never asked") })
    });
    while app.options_respawn.replace(&mut app.options_worker, || {}) {}
    while app
        .credential_respawn
        .replace(&mut app.credential_worker, || {})
    {}
    assert_eq!(made.load(Ordering::SeqCst), 6, "three replacements each");
    app.reconnect(&egui::Context::default());
    assert_eq!(made.load(Ordering::SeqCst), 8, "the click made one of each");
    assert!(!app.options_respawn.gave_up() && !app.credential_respawn.gave_up());
}
