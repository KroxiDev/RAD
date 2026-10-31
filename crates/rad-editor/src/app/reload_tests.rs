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

#[test]
fn manual_inspection_after_removing_a_linked_action_saves_the_visible_base_face() {
    // Replay the separate Board/Inspector reads after removing the linked action.
    // No reselection: this goes through pump_replies, not just Draft::load.
    for route in 0..3 {
        let ctx = egui::Context::default();
        let mut app = app();
        app.board = Some(board(1));
        app.control.select(Some("b2".into()));
        let mut linked = inspection(
            1,
            vec![json!({"id":"a3","fields":{"type":"org.rad.audio:microphone"}})],
        );
        linked.appearance = json!({"text":"Base","image":null,"text_position":"bottom",
        "visual_type":"org.rad.audio#microphone","states":{"muted":{"text":"Quiet","image":null}}});
        let (sent, received) = std::sync::mpsc::channel();
        let mut next = Some(linked);
        app.worker = Worker::spawn(move |job| {
            event(Ok(match job {
                Job::Inspect(_) => {
                    if let Some(view) = next.take() {
                        Reply::Inspection(view)
                    } else {
                        match route {
                            0 => Reply::Inspection(inspection(2, vec![])),
                            1 => Reply::Refresh(Refresh {
                                status: Value::Null,
                                board: Some(board(2)),
                                inspection: Some(inspection(2, vec![])),
                            }),
                            _ => Reply::InspectedAhead(inspection(2, vec![])),
                        }
                    }
                }
                Job::Transfer { .. } => Reply::TransferFinished {
                    image_token: Some("late-token".into()),
                    kind: native::TransferKind::Image,
                    data: json!({"media_ref":"late-old-state-image"}),
                    path: Default::default(),
                },
                Job::Request(Request::Undo {}) => {
                    sent.send(json!({"undo":true})).unwrap();
                    Reply::Response {
                        operation: "undo".into(),
                        response: rad_client::response::Response {
                            v: 1,
                            id: Some("1".into()),
                            session_id: None,
                            status: rad_api::Status::Applied,
                            data: rad_api::ResponseData::Applied(rad_api::Applied {
                                changed: true,
                                revision: "2".into(),
                            }),
                            error: None,
                        },
                    }
                }
                Job::Request(request) => {
                    sent.send(request.arguments()).unwrap();
                    Reply::Theme
                }
                _ => Reply::Theme,
            }))
        });
        app.worker.submit(Job::Inspect("b2".into())).unwrap();
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            std::thread::yield_now();
        }
        app.draft.select_state("muted".into());
        if route == 2 {
            let mut input = egui::RawInput::default();
            input.events.push(egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            });
            ctx.begin_pass(input);
            app.shortcuts(&ctx);
            let _ = ctx.end_pass();
            while app.worker.busy() {
                let _ = app.pump_replies(&ctx);
                std::thread::yield_now();
            }
            assert_eq!(
                received
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap(),
                json!({"undo":true})
            );
        }
        app.image_target = Some(("b2".into(), Some("muted".into())));
        app.icon_library.begin("b2", None);
        app.board = Some(board(2));
        if route == 2 {
            app.inspection = None;
        }
        app.worker.submit(Job::Inspect("b2".into())).unwrap();
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            std::thread::yield_now();
        }
        app.draft.text = "Visible manual text".into();
        app.draft.image = "visible-manual-image".into();
        assert!(
            !app.icon_library.open,
            "visual change invalidates library recipe"
        );
        app.worker
            .submit(Job::Transfer {
                kind: native::TransferKind::Image,
                path: Default::default(),
            })
            .unwrap();
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            std::thread::yield_now();
        }
        assert!(
            !app.draft.states.contains_key("muted"),
            "late import cannot revive hidden face"
        );
        assert!(
            app.discarded_imports
                .iter()
                .any(|token| token == "late-token")
        );
        app.save();
        let request = received
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let appearance = request["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|change| change["field"] == "appearance")
            .unwrap();
        assert_eq!(appearance["value"]["text"], "Visible manual text");
        assert_eq!(appearance["value"]["image"], "visible-manual-image");
        assert!(app.draft.state.is_none(), "Manual has no hidden state tab");
    }
}

#[test]
fn appearance_conflict_rereads_without_replaying_and_retains_only_the_same_visual_edits() {
    for same_visual in [true, false] {
        let ctx = egui::Context::default();
        let mut app = app();
        app.board = Some(board(1));
        app.control.select(Some("b2".into()));
        let mut original = inspection(1, vec![]);
        original.appearance = json!({"text":"Base","image":null,"visual_type":"org.rad.audio#microphone",
            "states":{"muted":{"text":"Quiet","image":null}}});
        app.adopt_inspection(original.clone());
        app.draft.select_state("muted".into());
        app.draft.text = "My face".into();
        app.draft.image = "my-image".into();
        let (sent, received) = std::sync::mpsc::channel();
        let mut first = true;
        app.worker = Worker::spawn(move |job| {
            if let Job::Request(request) = job {
                sent.send(request.arguments()).unwrap();
                if std::mem::take(&mut first) {
                    let mut error = crate::session::Error::protocol();
                    error.code = "cursor_expired".into();
                    return event(Err(error));
                }
            }
            event(Ok(Reply::Theme))
        });
        app.save();
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            std::thread::yield_now();
        }
        assert_eq!(received.recv().unwrap()["expected_revision"], "1");
        assert!(
            app.inspect_needed,
            "conflict must reread before another Save"
        );
        assert!(received.try_recv().is_err(), "no automatic mutation replay");
        let mut fresh = original;
        fresh.button["view_revision"] = json!("2");
        fresh.button["fields"]["name"] = json!("Remote name");
        if !same_visual {
            fresh.appearance["visual_type"] = Value::Null;
            fresh.appearance["states"] = json!({});
        }
        // The same adoption is shared by all three worker reply variants above.
        app.adopt_inspection(fresh);
        assert_eq!(
            app.draft.name, "Remote name",
            "untouched fields follow new publication"
        );
        if same_visual {
            assert_eq!(app.draft.text, "My face");
            assert_eq!(app.draft.image, "my-image");
            assert!(
                app.draft.published_image.is_empty(),
                "local image is not yet published"
            );
            app.save();
            assert_eq!(
                received
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap()["expected_revision"],
                "2"
            );
        } else {
            assert_eq!(app.draft.text, "Base");
            assert!(app.draft.state.is_none());
            assert!(app.draft.states.is_empty());
            assert!(received.try_recv().is_err());
        }
    }
}

#[test]
fn abandoning_imported_faces_drops_known_tokens_before_scheduling_more_reads() {
    for change_visual in [true, false] {
        let mut app = app();
        app.adopt_inspection(inspection(1, vec![]));
        app.prepared_imports = vec![
            ("t1".into(), "first".into()),
            ("t2".into(), "second".into()),
        ];
        let (sent, received) = std::sync::mpsc::channel();
        app.worker = Worker::spawn(move |job| {
            sent.send(job).unwrap();
            event(Ok(Reply::Theme))
        });
        if change_visual {
            let mut fresh = inspection(2, vec![]);
            fresh.appearance["visual_type"] = json!("org.rad.obs#mute");
            app.adopt_inspection(fresh);
        } else {
            app.select(1, Some("b9".into()));
        }
        app.pump_reads();
        let Job::DropImages { tokens, .. } = received
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
        else {
            panic!("cleanup precedes reads without replaying an edit")
        };
        assert_eq!(tokens, ["t1", "t2"]);
        assert!(app.prepared_imports.is_empty());
        assert_eq!(
            app.discarded_imports.len(),
            2,
            "tokens stay owned until acknowledgement"
        );
    }
}

#[test]
fn conflict_refresh_and_followup_inspection_do_not_erase_the_rebased_face() {
    let ctx = egui::Context::default();
    let mut app = app();
    app.board = Some(board(1));
    app.control.select(Some("b2".into()));
    let mut old = inspection(1, vec![]);
    old.appearance = json!({"text":"Base","image":null,"visual_type":"org.rad.audio#microphone","states":{"muted":{"text":"Quiet","image":null}}});
    app.adopt_inspection(old.clone());
    app.draft.select_state("muted".into());
    app.draft.text = "Keep my text".into();
    let mut fresh = old;
    fresh.button["view_revision"] = json!("2");
    fresh.appearance["states"]["muted"]["image"] = json!("remote-image");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    app.worker = Worker::spawn(move |job| {
        event(match job {
            Job::Request(Request::Edit { .. }) => {
                record.lock().unwrap().push("edit");
                let mut e = crate::session::Error::protocol();
                e.code = "cursor_expired".into();
                Err(e)
            }
            Job::Refresh { .. } => {
                record.lock().unwrap().push("refresh");
                Ok(Reply::Refresh(Refresh {
                    status: Value::Null,
                    board: Some(board(2)),
                    inspection: Some(fresh.clone()),
                }))
            }
            Job::Inspect(_) => {
                record.lock().unwrap().push("inspect");
                Ok(Reply::Inspection(fresh.clone()))
            }
            _ => Ok(Reply::Theme),
        })
    });
    app.save();
    for _ in 0..1000 {
        let _ = app.pump_replies(&ctx);
        app.pump_reads();
        std::thread::sleep(std::time::Duration::from_millis(1));
        if seen.lock().unwrap().contains(&"refresh")
            && !app.force
            && !app.inspect_needed
            && !app.worker.busy()
        {
            break;
        }
    }
    assert!(seen.lock().unwrap().contains(&"refresh"));
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .filter(|v| **v == "edit")
            .count(),
        1,
        "no replay"
    );
    assert_eq!(
        app.draft.text, "Keep my text",
        "scheduler's next inspection erased the rebased face"
    );
    // A coalesced notice can still request the same immutable inspection later.
    app.worker.enqueue(Job::Inspect("b2".into())).unwrap();
    while app.worker.busy() {
        let _ = app.pump_replies(&ctx);
        std::thread::yield_now();
    }
    assert_eq!(
        app.draft.text, "Keep my text",
        "duplicate reply must not discard local edits"
    );
    assert_eq!(
        app.draft.image, "remote-image",
        "editing only text must not replace the remote image"
    );
}

#[test]
fn deleted_key_and_changed_folder_abandon_imports_and_do_not_claim_late_success() {
    for change_folder in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app();
        app.board = Some(board(1));
        app.control.select(Some("b2".into()));
        app.adopt_inspection(inspection(1, vec![]));
        app.prepared_imports = vec![("old-token".into(), "old-image".into())];
        app.image_target = Some(("b2".into(), None));
        let mut gone = board(2);
        gone.buttons.clear();
        gone.folder["fields"]["slots"] = json!(vec![Value::Null; 32]);
        if change_folder {
            gone.folder["id"] = json!("f9");
        }
        let mut first = Some(gone);
        app.worker = Worker::spawn(move |_| {
            event(Ok(if let Some(board) = first.take() {
                Reply::Refresh(Refresh {
                    status: Value::Null,
                    board: Some(board),
                    inspection: None,
                })
            } else {
                Reply::TransferFinished {
                    kind: native::TransferKind::Image,
                    image_token: Some("late-token".into()),
                    data: json!({"media_ref":"late-image"}),
                    path: None,
                }
            }))
        });
        app.worker.submit(Job::Theme).unwrap();
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            std::thread::yield_now();
        }
        assert!(app.selected().is_none());
        assert!(
            app.draft.id.is_empty(),
            "lost destination has no editable old draft"
        );
        assert!(
            app.image_target.is_none(),
            "lost destination cannot accept late image"
        );
        assert!(app.discarded_imports.iter().any(|t| t == "old-token"));
        app.worker.submit(Job::Theme).unwrap();
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            std::thread::yield_now();
        }
        assert!(app.discarded_imports.iter().any(|t| t == "late-token"));
        assert_ne!(
            app.message, "Image prepared: save within 60 seconds",
            "discarded image must not announce success"
        );
    }
}
