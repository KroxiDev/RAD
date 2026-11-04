//! Clipboard shortcuts cross egui's event boundary and the runtime worker boundary.
use super::*;
use crate::session::{Grid, KeyView};

fn app() -> App {
    let root = std::env::temp_dir().join(format!("rad-editor-clipboard-{}", std::process::id()));
    let mut app = App::with_preferences_path(root.clone(), root.join("no-theme"), None);
    app.worker = Worker::spawn(|_| event(Ok(Reply::Theme)));
    app.connected = true;
    app.es = crate::preferences::Language::En;
    let mut slots = vec![Value::Null; 32];
    slots[1] = json!({"visibility":"visible","id":"b2"});
    app.board = Some(Board {
        grid: Grid::XL,
        folder: json!({"id":"f1","view_revision":"1","fields":{"parent_id":null,"slots":slots}}),
        buttons: vec![KeyView {
            entity: json!({"id":"b2","view_revision":"1","fields":{"name":"Original"}}),
            appearance: json!({"text":"Original"}),
        }],
    });
    select(&mut app, 1, "b2", vec![]);
    app
}
fn select(app: &mut App, position: usize, id: &str, actions: Vec<Value>) {
    app.select(position, Some(id.into()));
    app.inspection = Some(Inspection {
        button: json!({"id":id,"view_revision":"1","fields":{"name":"Original"}}),
        appearance: json!({"text":"Original"}),
        actions,
    });
}
fn input(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
    ctx.begin_pass(egui::RawInput {
        events,
        ..Default::default()
    });
    app.shortcuts(ctx);
    ctx.end_pass()
}
#[test]
fn copy_writes_the_selected_keys_marker_to_the_system_clipboard() {
    let mut app = app();
    let output = input(&mut app, &egui::Context::default(), vec![egui::Event::Copy]);
    assert!(output.platform_output.commands.iter().any(|command| {
        matches!(command, egui::OutputCommand::CopyText(text) if text == "rad-key:b2")
    }));
    assert_eq!(app.message, "Key copied");
}

fn event(result: Result<Reply, crate::session::Error>) -> Event {
    Event {
        paste: matches!(&result, Ok(Reply::Response { operation, .. }) if operation == "duplicate"),
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

#[test]
fn paste_replaces_the_selected_place_in_the_visible_folder() {
    for occupied in [false, true] {
        let mut app = app();
        let ctx = egui::Context::default();
        input(&mut app, &ctx, vec![egui::Event::Copy]);
        app.board.as_mut().unwrap().folder["id"] = json!("f9");
        if occupied {
            app.board.as_mut().unwrap().folder["fields"]["slots"][4] = json!({"id":"b10"});
            select(&mut app, 4, "b10", vec![]);
        } else {
            app.select(4, None);
        }
        let (sent, received) = std::sync::mpsc::channel();
        app.worker = Worker::spawn(move |job| {
            if let Job::Request(request) = job {
                sent.send(request).unwrap();
            }
            event(Ok(Reply::Theme))
        });
        input(
            &mut app,
            &ctx,
            vec![egui::Event::Paste("rad-key:b2".into())],
        );
        assert!(app.worker.busy(), "the paste was not sent");
        assert!(
            matches!(received.recv_timeout(std::time::Duration::from_secs(2)).unwrap(),
            Request::Duplicate { id, folder_id, position: 4, replace: true }
            if id == "b2" && folder_id == "f9")
        );
    }
}

fn applied() -> Reply {
    Reply::Response {
        operation: "duplicate".into(),
        response: rad_api::Response {
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
fn pump_reply(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    while app.worker.busy() {
        assert!(Instant::now() < deadline, "worker did not answer");
        let _ = app.pump_replies(ctx);
        std::thread::yield_now();
    }
}
fn replaced_board(app: &App) -> Board {
    let mut board = app.board.clone().unwrap();
    board.folder["view_revision"] = json!("2");
    board.folder["fields"]["slots"][4] = json!({"id":"b11","visibility":"visible"});
    board
}
fn paste_target(app: &mut App, ctx: &egui::Context) {
    input(app, ctx, vec![egui::Event::Copy]);
    app.board.as_mut().unwrap().folder["fields"]["slots"][4] = json!({"id":"b10"});
    select(app, 4, "b10", vec![]);
}
#[test]
fn pasted_key_is_selected_on_the_first_replacement_board_without_a_success_notice() {
    let mut app = app();
    let ctx = egui::Context::default();
    paste_target(&mut app, &ctx);
    let replacement = replaced_board(&app);
    app.worker = Worker::spawn(move |job| {
        event(Ok(match job {
            Job::Request(Request::Duplicate { .. }) => applied(),
            _ => Reply::Refresh(crate::session::Refresh {
                status: Value::Null,
                board: Some(replacement.clone()),
                inspection: None,
            }),
        }))
    });
    input(
        &mut app,
        &ctx,
        vec![egui::Event::Paste("rad-key:b2".into())],
    );
    pump_reply(&mut app, &ctx);
    app.command(Job::Theme);
    pump_reply(&mut app, &ctx);
    assert_eq!(app.selected().as_deref(), Some("b11"));
    assert_eq!(app.selected_slot, Some(4));
    assert!(app.message.is_empty(), "paste does not announce success");
}

#[test]
fn a_deleted_source_reports_why_paste_failed_and_does_not_select_a_later_replacement() {
    let mut app = app();
    let ctx = egui::Context::default();
    paste_target(&mut app, &ctx);
    let replacement = replaced_board(&app);
    app.worker = Worker::spawn(move |job| {
        if matches!(job, Job::Request(Request::Duplicate { .. })) {
            let mut answer = event(Err(crate::session::Error {
                code: "not_found".into(),
                cause: Some("paste_source_missing".into()),
                source_change: None,
                unknown: false,
            }));
            answer.paste = true;
            answer
        } else {
            event(Ok(Reply::Refresh(crate::session::Refresh {
                status: Value::Null,
                board: Some(replacement.clone()),
                inspection: None,
            })))
        }
    });
    input(
        &mut app,
        &ctx,
        vec![egui::Event::Paste("rad-key:b2".into())],
    );
    pump_reply(&mut app, &ctx);
    assert_eq!(app.message, "The copied key no longer exists");
    assert!(app.message_is_error);
    app.command(Job::Theme);
    pump_reply(&mut app, &ctx);
    assert_eq!(app.selected(), None);
}

#[test]
fn unrelated_clipboard_text_cut_return_and_the_source_itself_have_no_effect() {
    for case in [
        "other text",
        "old marker",
        "same key",
        "return",
        "cut",
        "no selection",
    ] {
        let mut app = app();
        let ctx = egui::Context::default();
        input(&mut app, &ctx, vec![egui::Event::Copy]);
        let events = match case {
            "other text" => {
                app.select(4, None);
                vec![egui::Event::Paste("rad-key:b2\n".into())]
            }
            "old marker" => {
                select(&mut app, 2, "b9", vec![]);
                input(&mut app, &ctx, vec![egui::Event::Copy]);
                app.select(4, None);
                vec![egui::Event::Paste("rad-key:b2".into())]
            }
            "return" => {
                select(&mut app, 1, "b2", vec![json!({"fields":{"type":"return"}})]);
                vec![egui::Event::Copy, egui::Event::Paste("rad-key:b2".into())]
            }
            "cut" => vec![egui::Event::Cut],
            "no selection" => {
                app.control.select(None);
                app.selected_slot = None;
                vec![egui::Event::Copy, egui::Event::Paste("rad-key:b2".into())]
            }
            _ => vec![egui::Event::Paste("rad-key:b2".into())],
        };
        let output = input(&mut app, &ctx, events);
        assert!(output.platform_output.commands.is_empty(), "{case}");
        assert!(!app.worker.busy(), "{case}");
    }
}

#[test]
fn copying_a_folder_keeps_its_marker_and_return_is_never_a_paste_destination() {
    let mut app = app();
    let ctx = egui::Context::default();
    select(
        &mut app,
        1,
        "b2",
        vec![json!({"fields":{"type":"enter_folder","folder_id":"f8"}})],
    );
    let output = input(&mut app, &ctx, vec![egui::Event::Copy]);
    assert!(
        output
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(command,
        egui::OutputCommand::CopyText(text) if text == "rad-key:b2"))
    );
    app.board.as_mut().unwrap().folder["fields"]["slots"][7] = json!({"id":"b9"});
    select(&mut app, 7, "b9", vec![json!({"fields":{"type":"return"}})]);
    input(
        &mut app,
        &ctx,
        vec![egui::Event::Paste("rad-key:b2".into())],
    );
    assert!(!app.worker.busy());
}

#[test]
fn key_clipboard_uses_the_same_screen_focus_popup_and_form_gates_as_delete() {
    for gate in [
        "text focus",
        "popup",
        "settings",
        "key form",
        "disconnected",
        "recovery",
    ] {
        for paste in [false, true] {
            let mut app = app();
            let ctx = egui::Context::default();
            input(&mut app, &ctx, vec![egui::Event::Copy]);
            if paste {
                app.select(4, None);
            }
            let event = if paste {
                egui::Event::Paste("rad-key:b2".into())
            } else {
                egui::Event::Copy
            };
            ctx.begin_pass(egui::RawInput {
                events: vec![event],
                ..Default::default()
            });
            match gate {
                "text focus" => {
                    ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("text")))
                }
                "popup" => egui::Popup::open_id(&ctx, egui::Id::new("menu")),
                "settings" => app.navigation.open(crate::screens::Section::General),
                "key form" => {
                    app.plugins.form = Some((
                        crate::plugins::FormTarget::AddAction("b2".into()),
                        crate::plugins::LoadedForm {
                            type_key: "test:action".into(),
                            id: None,
                            form: None,
                            original: vec![],
                        },
                    ))
                }
                "disconnected" => app.connected = false,
                "recovery" => app.recovery = crate::recovery::Mode::Asking,
                _ => unreachable!(),
            }
            app.shortcuts(&ctx);
            let output = ctx.end_pass();
            assert!(
                output.platform_output.commands.is_empty(),
                "{gate}, paste {paste}"
            );
            assert!(!app.worker.busy(), "{gate}, paste {paste}");
        }
    }
}

#[test]
fn an_earlier_poll_does_not_consume_the_paste_but_a_folder_change_cancels_selection() {
    for changed_folder in [false, true] {
        let mut app = app();
        let ctx = egui::Context::default();
        paste_target(&mut app, &ctx);
        let replacement = replaced_board(&app);
        let mut earlier = app.board.clone().unwrap();
        if changed_folder {
            earlier.folder["id"] = json!("f9");
        }
        let mut first_read = true;
        app.worker = Worker::spawn(move |job| {
            event(Ok(match job {
                Job::Request(Request::Duplicate { .. }) => applied(),
                _ => {
                    let board = if std::mem::take(&mut first_read) {
                        earlier.clone()
                    } else {
                        replacement.clone()
                    };
                    Reply::Refresh(crate::session::Refresh {
                        status: Value::Null,
                        board: Some(board),
                        inspection: None,
                    })
                }
            }))
        });
        app.command(Job::Theme);
        input(
            &mut app,
            &ctx,
            vec![egui::Event::Paste("rad-key:b2".into())],
        );
        pump_reply(&mut app, &ctx);
        app.command(Job::Theme);
        pump_reply(&mut app, &ctx);
        assert_eq!(
            app.selected().as_deref(),
            if changed_folder { None } else { Some("b11") }
        );
    }
}

#[test]
fn paste_failure_checks_the_source_once_and_only_names_a_confirmed_missing_entity() {
    use crate::session::{Port, Session};
    use std::sync::{Arc, Mutex};
    struct Server {
        diagnosis: Result<rad_api::Response, rad_client::connection::Failure>,
        requests: Arc<Mutex<Vec<String>>>,
    }
    fn missing() -> rad_api::Response {
        rad_api::Response {
            v: 1,
            id: Some("1".into()),
            session_id: None,
            status: rad_api::Status::Error,
            data: rad_api::ResponseData::None,
            error: Some(rad_api::Error {
                code: rad_api::ErrorCode::parse("not_found").unwrap(),
                message: "missing".into(),
                details: json!({}),
                outcome: Some(rad_api::Outcome::NotApplied),
            }),
        }
    }
    impl Port for Server {
        fn call(
            &mut self,
            request: Request,
        ) -> Result<rad_api::Response, rad_client::connection::Failure> {
            self.requests
                .lock()
                .unwrap()
                .push(request.operation().as_str().into());
            match request {
                Request::Duplicate { replace: true, .. } => Ok(missing()),
                Request::Get { id } => {
                    assert_eq!(id, "b2");
                    std::mem::replace(&mut self.diagnosis, Ok(missing()))
                }
                _ => panic!("unexpected request"),
            }
        }
    }
    for case in ["destination", "source", "unavailable"] {
        let mut app = app();
        let ctx = egui::Context::default();
        paste_target(&mut app, &ctx);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let diagnosis = match case {
            "destination" => Ok(rad_api::Response {
                v: 1, id: Some("2".into()), session_id: None,
                status: rad_api::Status::Ok,
                data: rad_api::ResponseData::Entity(serde_json::from_value(json!({"visibility":"visible","id":"b2","kind":"button","fields":{"name":"Original","mode":"sequential","gesture":"press","permissions":{"inherit":true},"effective_permissions":{"consult":true,"modify":true,"execute":false},"folder_id":"f1","position":1},"view_revision":"2","sections":[]})).unwrap()),
                error: None,
            }),
            "source" => Ok(missing()),
            _ => Err(rad_client::connection::Failure { code: "runtime_unavailable", id: None, session_id: None, outcome: None }),
        };
        let mut session = Session::new(Server {
            diagnosis,
            requests: requests.clone(),
        });
        app.worker = Worker::spawn(move |job| {
            let Job::Request(request) = job else {
                panic!("unexpected job")
            };
            let result = crate::native::request_reply(&mut session, request);
            let mut answer = event(result);
            answer.paste = true;
            answer.connected = session.is_open();
            answer
        });
        input(
            &mut app,
            &ctx,
            vec![egui::Event::Paste("rad-key:b2".into())],
        );
        pump_reply(&mut app, &ctx);
        assert_eq!(
            app.message,
            match case {
                "destination" => "The destination folder no longer exists",
                "source" => "The copied key no longer exists",
                _ => "It no longer exists",
            },
            "{case}"
        );
        assert_eq!(
            *requests.lock().unwrap(),
            ["duplicate", "get"],
            "one bounded read, no replay"
        );
        assert_eq!(app.connected, case != "unavailable");
        assert!(app.pending_paste.is_none());
    }
}
