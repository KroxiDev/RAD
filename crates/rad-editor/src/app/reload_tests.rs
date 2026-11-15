//! A change replaces what is on screen at once, never blank or mixing revisions, and
//! Delete removes the selected key.
use super::*;
use crate::session::{Grid, Inspection, KeyView, Refresh};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

pub(super) fn event(result: Result<Reply, crate::session::Error>) -> Event {
    Event {
        paste: false,
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
/// Preserve the production Save envelope while fixtures answer ordinary requests.
pub(super) fn worker(mut run: impl FnMut(Job) -> Event + Send + 'static) -> Worker<Job, Event> {
    Worker::spawn(move |job| {
        let (ticket, job) = match job {
            Job::Save { ticket, request } => (Some(ticket), Job::Request(request)),
            job => (None, job),
        };
        let mut event = run(job);
        if let Some(ticket) = ticket {
            event.result = Ok(Reply::Saved {
                ticket,
                result: Box::new(event.result),
            });
        }
        event
    })
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
    worker(move |job: Job| {
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
fn inspector_refresh_preserves_unsaved_fields_and_adopts_untouched_fields() {
    let actions = vec![
        json!({"id":"a3","fields":{"type":"wait","parameters":{"milliseconds":100}}}),
        json!({"id":"a4","fields":{"type":"wait","parameters":{"milliseconds":200}}}),
    ];
    let mut app = app();
    app.board = Some(board(1));
    app.control.select(Some("b2".into()));
    app.adopt_inspection(inspection(1, actions.clone()));
    app.draft.name = "Draft name".into();
    app.draft.text = "Draft text".into();
    app.draft.waits[0] = "250".into();

    let mut fresh = inspection(2, actions);
    fresh.button["fields"]["name"] = json!("Remote name");
    fresh.button["fields"]["gesture"] = json!("hold");
    fresh.actions[1]["fields"]["parameters"]["milliseconds"] = json!(350);
    let (saved, received) = std::sync::mpsc::channel();
    app.worker = worker(move |job| {
        if let Job::Request(request @ Request::Edit { .. }) = job {
            saved.send(request.arguments()).unwrap();
            return event(Ok(Reply::Theme));
        }
        event(Ok(Reply::Refresh(Refresh {
            status: Value::Null,
            board: Some(board(2)),
            inspection: Some(fresh.clone()),
        })))
    });
    app.worker.submit(Job::Theme).unwrap();
    pump_until(&mut app, &egui::Context::default(), |app| {
        app.inspection
            .as_ref()
            .is_some_and(|v| revision_of(&v.button) == "2")
    });
    assert_eq!(app.draft.name, "Draft name");
    assert_eq!(app.draft.text, "Draft text");
    assert_eq!(app.draft.waits, ["250", "350"]);
    assert_eq!(app.draft.gesture, "hold");
    assert!(app.unsaved());
    app.save();
    let mut saved = None;
    pump_until(&mut app, &egui::Context::default(), |_| {
        saved = received.try_recv().ok();
        saved.is_some()
    });
    let saved = saved.unwrap();
    let changes = saved["changes"].as_array().unwrap();
    for (id, field, value) in [
        ("b2", "name", json!("Draft name")),
        ("b2", "gesture", json!("hold")),
        ("a3", "milliseconds", json!(250)),
        ("a4", "milliseconds", json!(350)),
    ] {
        assert!(changes.iter().any(|change| change["id"] == id
            && change["field"] == field
            && change["value"] == value));
    }
}

#[test]
fn action_list_changes_preserve_edited_parameters_by_identity() {
    let actions = vec![
        json!({"id":"a3","fields":{"type":"sleep","parameters":{}}}),
        json!({"id":"a4","fields":{"type":"wait","parameters":{"milliseconds":100}}}),
        json!({"id":"a5","fields":{"type":"launcher","parameters":{"desktop_entry":"before.desktop"}}}),
        json!({"id":"a6","fields":{"type":"brightness","parameters":{"direction":"increase"}}}),
        json!({"id":"a7","fields":{"type":"wait","parameters":{"milliseconds":200}}}),
    ];
    for change in ["remove", "add", "reorder"] {
        let mut app = app();
        app.adopt_inspection(inspection(1, actions.clone()));
        app.draft.waits[1] = "250".into();
        app.draft.entries[2] = "edited.desktop".into();
        app.draft.brightness_directions[3] = rad_api::BrightnessDirection::Decrease;
        let mut fresh = actions.clone();
        fresh[4]["fields"]["parameters"]["milliseconds"] = json!(350);
        match change {
            "remove" => {
                fresh.remove(0);
            }
            "add" => fresh.insert(
                0,
                json!({"id":"a8","fields":{"type":"wait","parameters":{"milliseconds":900}}}),
            ),
            _ => fresh.rotate_left(2),
        }
        let count = fresh.len();
        app.adopt_inspection(inspection(2, fresh));
        let (sent, received) = std::sync::mpsc::channel();
        app.worker = worker(move |job| {
            sent.send(job).unwrap();
            event(Ok(Reply::Theme))
        });
        app.save();
        let Job::Request(request @ Request::Edit { .. }) = received
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
        else {
            panic!("expected a key edit after {change}");
        };
        let args = request.arguments();
        let changes = args["changes"].as_array().unwrap();
        for (id, field, value) in [
            ("a4", "milliseconds", json!(250)),
            ("a5", "desktop_entry", json!("edited.desktop")),
            ("a6", "parameters", json!({"direction":"decrease"})),
            ("a7", "milliseconds", json!(350)),
        ] {
            assert!(
                changes
                    .iter()
                    .any(|c| c["id"] == id && c["field"] == field && c["value"] == value),
                "{change} lost {id}'s {field}: {changes:?}"
            );
        }
        assert!(
            changes
                .iter()
                .any(|c| c["indices"] == json!((0..count).collect::<Vec<_>>()))
        );
        if change == "add" {
            assert!(changes.iter().any(|c| c["id"] == "a8" && c["value"] == 900));
        }
    }
}

#[test]
fn saving_a_formatted_wait_adopts_its_published_number_without_losing_later_edits() {
    for (typed, after_send) in [
        (" 250", None),
        ("250 ", None),
        ("0250", None),
        ("+250", None),
        ("0250", Some("300 ")),
        ("0250", Some("invalid")),
        ("250", Some("100")),
    ] {
        let mut app = app();
        app.board = Some(board(1));
        app.control.select(Some("b2".into()));
        let action = |milliseconds| json!({"id":"a3","fields":{"type":"wait","parameters":{"milliseconds":milliseconds}}});
        app.adopt_inspection(inspection(1, vec![action(100)]));
        app.draft.waits[0] = typed.into();
        app.draft.name = "Saved name".into();
        app.draft.text = "Saved text".into();
        app.worker = worker(move |job| {
            event(Ok(match job {
                Job::Request(request @ Request::Edit { .. }) => {
                    assert!(
                        request.arguments()["changes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|change| change["id"] == "a3"
                                && change["field"] == "milliseconds"
                                && change["value"] == 250)
                    );
                    Reply::Response {
                        operation: "edit".into(),
                        response: rad_api::Response {
                            v: 1,
                            id: None,
                            session_id: None,
                            status: rad_api::Status::Applied,
                            error: None,
                            data: rad_api::ResponseData::Applied(rad_api::Applied {
                                changed: true,
                                revision: "2".into(),
                            }),
                        },
                    }
                }
                Job::Refresh { .. } => {
                    let mut view = inspection(2, vec![action(250)]);
                    view.button["fields"]["name"] = json!("Saved name");
                    view.appearance["text"] = json!("Saved text");
                    Reply::Refresh(Refresh {
                        status: Value::Null,
                        board: Some(board(2)),
                        inspection: Some(view),
                    })
                }
                _ => Reply::Theme,
            }))
        });
        app.save();
        if let Some(edited) = after_send {
            app.draft.waits[0] = edited.into();
            app.draft.name = "Key".into();
            app.draft.text = "Key".into();
        }
        pump_until(&mut app, &egui::Context::default(), |app| {
            app.inspection
                .as_ref()
                .is_some_and(|view| revision_of(&view.button) == "2")
        });
        assert_eq!(app.draft.waits[0], after_send.unwrap_or("250"));
        assert_eq!(
            app.draft.name,
            if after_send.is_some() {
                "Key"
            } else {
                "Saved name"
            }
        );
        assert_eq!(
            app.draft.text,
            if after_send.is_some() {
                "Key"
            } else {
                "Saved text"
            }
        );
        assert_eq!(app.unsaved(), after_send.is_some());
    }
}

#[test]
fn queued_saves_retain_later_edits_when_either_result_is_refused_or_unknown() {
    for failure in ["first", "second", "unknown", "none", "cursor_expired"] {
        let mut app = app();
        app.board = Some(board(1));
        app.control.select(Some("b2".into()));
        let action = |milliseconds| json!({"id":"a3","fields":{"type":"wait","parameters":{"milliseconds":milliseconds}}});
        let view = move |revision, milliseconds| {
            let mut view = inspection(revision, vec![action(milliseconds)]);
            if failure == "cursor_expired" {
                view.appearance["visual_type"] = json!("org.rad.audio#microphone");
                view.appearance["states"] = json!({"off":{"text":"Off"}});
            }
            view
        };
        app.adopt_inspection(view(1, 100));
        if failure == "cursor_expired" {
            app.draft.select_state("off".into());
        }
        let (release, wait) = std::sync::mpsc::channel();
        let mut saved = 100;
        let mut requests = 0;
        app.worker = worker(move |job| {
            event(match job {
                Job::Request(request @ Request::Edit { .. }) => {
                    requests += 1;
                    if requests == 1 {
                        wait.recv().unwrap();
                    }
                    let refused = (failure == "first" && requests == 1)
                        || (matches!(failure, "second" | "cursor_expired") && requests == 2);
                    if !refused {
                        saved = request.arguments()["changes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|change| change["field"] == "milliseconds")
                            .unwrap()["value"]
                            .as_u64()
                            .unwrap();
                    }
                    if refused || (failure == "unknown" && requests == 2) {
                        Err(crate::session::Error {
                            code: if failure == "unknown" {
                                "io_error"
                            } else if failure == "cursor_expired" {
                                "cursor_expired"
                            } else {
                                "permission_denied"
                            }
                            .into(),
                            cause: None,
                            source_change: None,
                            unknown: failure == "unknown",
                        })
                    } else {
                        Ok(Reply::Response {
                            operation: "edit".into(),
                            response: rad_api::Response {
                                v: 1,
                                id: None,
                                session_id: None,
                                status: rad_api::Status::Applied,
                                error: None,
                                data: rad_api::ResponseData::Applied(rad_api::Applied {
                                    changed: true,
                                    revision: (requests + 1).to_string(),
                                }),
                            },
                        })
                    }
                }
                Job::Refresh { .. } => Ok(Reply::Refresh(Refresh {
                    status: Value::Null,
                    board: Some(board(3)),
                    inspection: Some(view(3, saved)),
                })),
                _ => Ok(Reply::Theme),
            })
        });
        app.draft.waits[0] = "250".into();
        app.save();
        app.draft.waits[0] = "300".into();
        app.save();
        app.draft.waits[0] = "100".into();
        release.send(()).unwrap();
        pump_until(&mut app, &egui::Context::default(), |app| {
            app.inspection
                .as_ref()
                .is_some_and(|view| revision_of(&view.button) == "3")
        });
        assert_eq!(app.draft.waits[0], "100", "{failure}");
        assert!(app.unsaved(), "{failure}");
    }
}

#[test]
fn an_incomplete_refresh_retains_the_draft_until_a_coherent_inspector_arrives() {
    for library in [false, true] {
        let mut app = app();
        app.board = Some(board(1));
        app.control.select(Some("b2".into()));
        app.adopt_inspection(inspection(1, vec![]));
        app.draft.name = "Draft name".into();
        app.draft.text = "Draft text".into();
        if library {
            app.icon_library.begin("b2", None);
        }
        let mut first = true;
        app.worker = worker(move |_| {
            event(Ok(if std::mem::take(&mut first) {
                Reply::Refresh(Refresh {
                    status: Value::Null,
                    board: Some(board(2)),
                    inspection: None,
                })
            } else {
                Reply::Inspection(inspection(2, vec![]))
            }))
        });
        app.worker.submit(Job::Theme).unwrap();
        let ctx = egui::Context::default();
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            std::thread::yield_now();
        }
        assert!(app.inspection.is_none());
        app.worker.submit(Job::Inspect("b2".into())).unwrap();
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            std::thread::yield_now();
        }
        assert_eq!(app.draft.name, "Draft name");
        assert_eq!(app.draft.text, "Draft text");
    }
}

#[test]
fn save_followed_by_undo_before_refresh_does_not_revive_the_saved_draft() {
    let mut app = app();
    app.board = Some(board(1));
    app.control.select(Some("b2".into()));
    app.adopt_inspection(inspection(1, vec![]));
    app.draft.name = "Saved name".into();
    app.draft.text = "Saved text".into();
    let (release, blocked) = std::sync::mpsc::channel();
    app.worker = worker(move |job| {
        event(Ok(match job {
            Job::Request(request @ (Request::Edit { .. } | Request::Undo {})) => {
                if matches!(request, Request::Edit { .. }) {
                    assert!(
                        request.arguments()["changes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|c| c["field"] == "name" && c["value"] == "Saved name")
                    );
                    blocked.recv().unwrap();
                }
                Reply::Response {
                    operation: request.operation().as_str().into(),
                    response: rad_api::Response {
                        v: 1,
                        id: None,
                        session_id: None,
                        status: rad_api::Status::Applied,
                        data: rad_api::ResponseData::Applied(rad_api::Applied {
                            changed: true,
                            revision: "3".into(),
                        }),
                        error: None,
                    },
                }
            }
            Job::Refresh { .. } => Reply::Refresh(Refresh {
                status: Value::Null,
                board: Some(board(3)),
                inspection: Some(inspection(3, vec![])),
            }),
            _ => Reply::Theme,
        }))
    });
    app.save();
    app.send(Request::Undo {});
    release.send(()).unwrap();
    pump_until(&mut app, &egui::Context::default(), |app| {
        app.inspection
            .as_ref()
            .is_some_and(|v| revision_of(&v.button) == "3")
    });
    assert_eq!(app.draft.name, "Key");
    assert_eq!(app.draft.text, "Key");
    assert!(!app.unsaved());
}

#[test]
fn delete_removes_the_selected_key_or_the_folder_it_enters_without_asking() {
    let deleted = Arc::new(Mutex::new(Vec::new()));
    let record = deleted.clone();
    let mut app = app();
    app.worker = worker(move |job: Job| {
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
        app.worker = worker(move |job| {
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
        app.worker = worker(move |job| {
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
        app.worker = worker(move |job| {
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
    app.worker = worker(move |job| {
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
        app.worker = worker(move |_| {
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

#[test]
fn text_style_save_keeps_hidden_text_and_sends_only_changed_base_fields_or_complete_states() {
    let mut app = app();
    let mut view = inspection(7, vec![]);
    view.appearance = json!({"text":"Keep me","image":null,"visual_type":null,"text_position":"middle","text_position_automatic":true,"text_size":13,"text_color":"#FFFFFF","text_band":true});
    app.adopt_inspection(view.clone());
    app.draft.style = rad_api::TextStyle {
        position: Some(rad_api::TextPosition::Hidden),
        size: 24,
        color: [12, 34, 56],
        band: false,
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    app.worker = worker(move |job| {
        if let Job::Request(request) = job {
            sender.send(request.arguments()).unwrap();
        }
        event(Ok(Reply::Theme))
    });
    app.save();
    let request = receiver
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let value = request["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["field"] == "appearance")
        .unwrap()["value"]
        .clone();
    assert_eq!(value["text"], "Keep me");
    assert_eq!(value["text_position"], "hidden");
    assert_eq!(value["text_size"], 24);
    assert_eq!(value["text_color"], "#0C2238");
    assert_eq!(value["text_band"], false);

    view.appearance["visual_type"] = json!("org.rad.audio#microphone");
    view.appearance["states"] = json!({"off":{"text":"Quiet","image":null,"text_position":"bottom","text_position_automatic":true,"text_size":13,"text_color":"#FFFFFF","text_band":true},"on":{"text":"Live","image":null,"text_position":"top","text_position_automatic":false,"text_size":20,"text_color":"#FF8040","text_band":false}});
    app.adopt_inspection(view);
    app.draft.select_state("off".into());
    app.draft.style.size = 8;
    let (sender, receiver) = std::sync::mpsc::channel();
    app.worker = worker(move |job| {
        if let Job::Request(request) = job {
            sender.send(request.arguments()).unwrap();
        }
        event(Ok(Reply::Theme))
    });
    app.save();
    let request = receiver
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert_eq!(request["expected_revision"], "7");
    let value = request["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["field"] == "appearance")
        .unwrap()["value"]
        .clone();
    for field in ["text_position", "text_size", "text_color", "text_band"] {
        assert!(value.get(field).is_none(), "unchanged base {field}");
    }
    assert_eq!(value["states"].as_object().unwrap().len(), 2);
    assert_eq!(value["states"]["off"]["text_size"], 8);
    assert!(value["states"]["off"].get("text_position").is_none());
    assert_eq!(value["states"]["on"]["text_position"], "top");
    assert_eq!(value["states"]["on"]["text_size"], 20);
    assert_eq!(value["states"]["on"]["text_color"], "#FF8040");
    assert_eq!(value["states"]["on"]["text_band"], false);
}
