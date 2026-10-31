//! A change replaces what is on screen at once, never blank or mixing revisions, and
//! Delete removes the selected key.
use super::*;
use crate::session::{Grid, Inspection, KeyView, Refresh};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

fn event(result: Result<Reply, crate::session::Error>) -> Event {
    Event {
        plugin_apply: false,
        plugin_admin: false,
        plugin_manifest: false,
        plugin_dependencies: false,
        plugin_form: false,
        plugin_visual: false,
        background: false,
        connected: true,
        transfer_job: false,
        palette: Palette::default(),
        result,
    }
}
fn board(revision: u64) -> Board {
    let revision = revision.to_string();
    let mut slots = vec![Value::Null; 32];
    slots[1] = json!({"visibility":"visible","id":"b2"});
    Board {
        grid: Grid::XL,
        folder: json!({"id":"f1","view_revision":revision,"fields":{"name":"Main","parent_id":null,"slots":slots}}),
        buttons: vec![KeyView {
            entity: json!({"id":"b2","view_revision":revision,"fields":{"name":"Key","mode":"sequential","gesture":"press"}}),
            appearance: json!({"text":"Key","image":null,"text_position":"bottom","visual_type":null}),
        }],
    }
}
fn inspection(revision: u64, actions: Vec<Value>) -> Inspection {
    Inspection {
        button: json!({"id":"b2","view_revision":revision.to_string(),"fields":{"name":"Key","mode":"sequential","gesture":"press"}}),
        appearance: json!({"text":"Key","image":null,"text_position":"bottom","visual_type":null}),
        actions,
    }
}
fn app() -> App {
    let root = std::env::temp_dir().join(format!("rad-editor-reload-{}", std::process::id()));
    let mut app = App::with_preferences_path(root.clone(), root.join("no-theme"), None);
    app.es = crate::preferences::Language::En;
    app.connected = true;
    app
}
/// A runtime at `revision` that answers reads, as the real worker would.
fn runtime(revision: Arc<AtomicU64>) -> Worker<Job, Event> {
    Worker::spawn(move |job: Job| {
        let now = revision.load(Ordering::SeqCst);
        event(Ok(match job {
            Job::Refresh {
                board: read,
                selected,
                ..
            } => Reply::Refresh(Refresh {
                status: Value::Null,
                board: read.then(|| board(now)),
                inspection: selected
                    .filter(|s| read && s.revision != Some(now.to_string()))
                    .map(|_| inspection(now, vec![])),
            }),
            Job::Inspect(_) => Reply::Inspection(inspection(now, vec![])),
            Job::InspectAhead(_) => Reply::InspectedAhead(inspection(now, vec![])),
            Job::Pictures(_) => Reply::Pictures(vec![]),
            _ => Reply::Theme,
        }))
    })
}
fn pump_until(app: &mut App, ctx: &egui::Context, mut check: impl FnMut(&App) -> bool) {
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while !check(app) {
        assert!(
            Instant::now() < deadline,
            "the editor never settled: board {:?} inspection {:?} force {} busy {}",
            app.board
                .as_ref()
                .map(|b| b.folder["view_revision"].clone()),
            app.inspection
                .as_ref()
                .map(|i| i.button["view_revision"].clone()),
            app.force,
            app.worker.busy()
        );
        app.pump(ctx);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
fn revision_of(value: &Value) -> &str {
    value["view_revision"].as_str().unwrap_or_default()
}

#[test]
fn a_change_to_the_selected_key_replaces_its_panel_without_blanking_it() {
    let revision = Arc::new(AtomicU64::new(1));
    let mut app = app();
    app.worker = runtime(revision.clone());
    let ctx = egui::Context::default();
    pump_until(&mut app, &ctx, |app| app.board.is_some());
    app.select(1, Some("b2".into()));
    pump_until(&mut app, &ctx, |app| app.inspection.is_some());
    // What the runtime's notices ask for after an edit.
    revision.store(2, Ordering::SeqCst);
    app.force = true;
    app.inspect_needed = true;
    pump_until(&mut app, &ctx, |app| {
        assert!(app.inspection.is_some(), "the panel went blank");
        let inspection = app.inspection.as_ref().unwrap();
        let board = app.board.as_ref().unwrap();
        assert!(
            crate::session::coherent(board, inspection),
            "key and panel at different revisions"
        );
        revision_of(&board.folder) == "2" && revision_of(&inspection.button) == "2"
    });
}

#[test]
fn delete_removes_the_selected_key_or_the_folder_it_enters_without_asking() {
    let deleted = Arc::new(Mutex::new(Vec::new()));
    let record = deleted.clone();
    let mut app = app();
    app.worker = Worker::spawn(move |job: Job| {
        if let Job::Request(Request::Delete { id, .. }) = job {
            record.lock().unwrap().push(id);
        }
        event(Ok(Reply::Theme))
    });
    app.board = Some(board(1));
    app.control.select(Some("b2".into()));
    let press = |app: &mut App| {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::Delete,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        });
        ctx.begin_pass(input);
        app.shortcuts(&ctx);
        let _ = ctx.end_pass();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while app.worker.busy() {
            assert!(Instant::now() < deadline);
            app.worker.poll();
            std::thread::yield_now();
        }
    };
    press(&mut app);
    assert!(
        deleted.lock().unwrap().is_empty(),
        "unread, it could be a folder"
    );
    app.inspection = Some(inspection(
        1,
        vec![json!({"id":"a3","fields":{"type":"enter_folder","folder_id":"f9"}})],
    ));
    press(&mut app);
    assert_eq!(*deleted.lock().unwrap(), ["f9"]);
    app.inspection = Some(inspection(1, vec![]));
    press(&mut app);
    assert_eq!(*deleted.lock().unwrap(), ["f9", "b2"]);
}
