use super::*;
struct Frame {
    texts: Vec<(String, egui::Rect)>,
    intent: Option<Intent>,
}
fn frame(ctx: &egui::Context, field: &mut Field, events: Vec<egui::Event>) -> Frame {
    let mut intent = None;
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900., 600.),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            intent = field_ui(ui, field, false, true);
            ui.button("Outside").clicked();
        },
    );
    Frame {
        intent,
        texts: output
            .shapes
            .into_iter()
            .filter_map(|s| {
                if let egui::epaint::Shape::Text(t) = s.shape {
                    Some((
                        t.galley.job.text.clone(),
                        t.galley.rect.translate(t.pos.to_vec2()),
                    ))
                } else {
                    None
                }
            })
            .collect(),
    }
}
fn click(pos: egui::Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        },
    ]
}
fn at(frame: &Frame, label: &str) -> egui::Pos2 {
    frame
        .texts
        .iter()
        .find(|(s, _)| s == label)
        .unwrap_or_else(|| panic!("missing {label}; {:?}", frame.texts))
        .1
        .center()
}
fn panel_frame(
    ctx: &egui::Context,
    panel: &mut Panel,
    worker: &mut CredentialWorker,
    events: Vec<egui::Event>,
) -> Frame {
    panel.pump(worker);
    let output = ctx.run_ui(
        egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900., 600.),
            )),
            ..Default::default()
        },
        |ui| {
            panel.show(ui, worker, true, false);
            ui.button("Outside").clicked();
        },
    );
    Frame {
        intent: None,
        texts: output
            .shapes
            .into_iter()
            .filter_map(|s| {
                if let egui::epaint::Shape::Text(t) = s.shape {
                    Some((
                        t.galley.job.text.clone(),
                        t.galley.rect.translate(t.pos.to_vec2()),
                    ))
                } else {
                    None
                }
            })
            .collect(),
    }
}
#[test]
fn panel_confirms_only_ack_and_discards_unsubmitted_drafts_on_context_change() {
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    let (asked_tx, asked) = mpsc::channel();
    let (answer, answers) = mpsc::channel();
    let mut worker = CredentialWorker::spawn(move |job: Job| {
        let generation = job.generation;
        let slot = job.slot.clone();
        asked_tx.send(job).unwrap();
        Event {
            generation,
            slot,
            result: answers.recv().unwrap(),
        }
    });
    let mut panel = Panel::default();
    let ctx = egui::Context::default();
    panel.bind("first", "p:authenticate", &["password".into()]);
    panel_frame(&ctx, &mut panel, &mut worker, vec![]);
    let query = asked
        .recv_timeout(Duration::from_secs(1))
        .expect("opening form queries declared slot");
    assert_eq!(query.plugin, "p");
    assert_eq!(query.slot, "password");
    assert!(matches!(query.operation, Operation::Status));
    answer.send(Ok(State::Present)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let first = loop {
        let f = panel_frame(&ctx, &mut panel, &mut worker, vec![]);
        if f.texts.iter().any(|(s, _)| s == "••••••••") {
            break f;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    };
    panel_frame(&ctx, &mut panel, &mut worker, click(at(&first, "••••••••")));
    let typed = panel_frame(
        &ctx,
        &mut panel,
        &mut worker,
        vec![egui::Event::Text("synthetic-panel".into())],
    );
    assert!(asked.try_recv().is_err());
    panel_frame(&ctx, &mut panel, &mut worker, click(at(&typed, "Outside")));
    let warned = panel_frame(&ctx, &mut panel, &mut worker, vec![]);
    panel_frame(
        &ctx,
        &mut panel,
        &mut worker,
        click(at(&warned, "Save credential")),
    );
    let write = asked.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        matches!(write.operation,Operation::Replace(ref secret) if secret.0.as_str()=="synthetic-panel")
    );
    let pending = panel_frame(&ctx, &mut panel, &mut worker, vec![]);
    assert!(
        pending
            .texts
            .iter()
            .any(|(s, _)| s == "Saving — awaiting durable confirmation")
    );
    assert!(asked.try_recv().is_err());
    answer
        .send(Err(crate::session::Error {
            code: "storage_uncertain".into(),
            unknown: true,
        }))
        .unwrap();
    let failed = loop {
        let f = panel_frame(&ctx, &mut panel, &mut worker, vec![]);
        if f.texts.iter().any(|(s, _)| s.contains("Outcome unknown")) {
            break f;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    };
    assert!(!failed.texts.iter().any(|(s, _)| s == "••••••••"));
    assert!(asked.try_recv().is_err());
    panel_frame(
        &ctx,
        &mut panel,
        &mut worker,
        click(at(&failed, "New credential")),
    );
    panel_frame(
        &ctx,
        &mut panel,
        &mut worker,
        vec![egui::Event::Text("discard-me".into())],
    );
    panel.bind("second", "p#indicator", &["password".into()]);
    panel_frame(&ctx, &mut panel, &mut worker, vec![]);
    let fresh = asked.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(fresh.operation, Operation::Status));
    assert!(panel.fields["password"].draft.0.is_empty());
    panel.clear();
    answer.send(Ok(State::Present)).unwrap();
}
#[test]
fn worker_uses_editor_socket_and_waits_for_durable_ack_without_replay() {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        sync::mpsc,
        time::Duration,
    };
    let root = std::env::temp_dir().join(format!("rad-credentials-editor-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::create_dir(root.join("rad")).unwrap();
    std::fs::set_permissions(root.join("rad"), std::fs::Permissions::from_mode(0o700)).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let listener = UnixListener::bind(root.join("rad/editor.sock")).unwrap();
    let (asked_tx, asked) = mpsc::channel();
    let (release, hold) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        for expected in ["hello", "credential_replace"] {
            let mut header = [0; 4];
            stream.read_exact(&mut header).unwrap();
            let mut bytes = vec![0; u32::from_be_bytes(header) as usize];
            stream.read_exact(&mut bytes).unwrap();
            let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(request["op"], expected);
            let data = if expected == "hello" {
                serde_json::json!({"protocol":1,"session_id":"00000000000000000000000000000001","limits":{},"capabilities":[]})
            } else {
                assert_eq!(
                    request["args"],
                    serde_json::json!({"plugin_id":"p","slot":"password","secret":"synthetic-worker"})
                );
                asked_tx.send(()).unwrap();
                hold.recv_timeout(Duration::from_secs(3)).unwrap();
                serde_json::json!({"state":"present"})
            };
            let response = serde_json::json!({"v":1,"id":request["id"],"session_id":"00000000000000000000000000000001","status":"ok","data":data,"error":null});
            let body = serde_json::to_vec(&response).unwrap();
            stream
                .write_all(&(body.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let mut worker = start(root);
    worker
        .submit(Job {
            generation: 1,
            plugin: "p".into(),
            slot: "password".into(),
            operation: Operation::Replace(SecretDraft(Zeroizing::new("synthetic-worker".into()))),
        })
        .unwrap();
    asked
        .recv_timeout(Duration::from_secs(2))
        .expect("worker must send replace on editor endpoint");
    assert!(worker.poll().is_none());
    assert!(worker.busy());
    release.send(()).unwrap();
    let event = worker.receive().unwrap();
    assert_eq!(event.result.unwrap(), State::Present);
    assert_eq!(event.generation, 1);
    assert_eq!(event.slot, "password");
    server.join().unwrap();
}
#[test]
fn malformed_success_cannot_confirm_mutation_or_leak_payload() {
    for data in [
        serde_json::json!({"state":"bad"}),
        serde_json::json!({"state":"present","secret":"synthetic-malformed"}),
    ] {
        let error = decode_state(data.clone(), true, rad_client::response::Status::Ok).unwrap_err();
        assert!(
            error.unknown,
            "a malformed success does not prove whether the mutation applied"
        );
        assert!(!format!("{error:?}").contains("synthetic-malformed"));
        assert!(
            !decode_state(data, false, rad_client::response::Status::Ok)
                .unwrap_err()
                .unknown
        );
    }
}
#[test]
fn oversized_utf8_is_explained_and_cannot_be_saved() {
    let ctx = egui::Context::default();
    let mut field = Field::default();
    let first = frame(&ctx, &mut field, vec![]);
    frame(&ctx, &mut field, click(at(&first, "New credential")));
    let typed = frame(&ctx, &mut field, vec![egui::Event::Text("é".repeat(4096))]);
    frame(&ctx, &mut field, click(at(&typed, "Outside")));
    let warned = frame(&ctx, &mut field, vec![]);
    assert!(!warned.texts.iter().any(|(s, _)| s == "Save credential"));
    assert!(
        warned
            .texts
            .iter()
            .any(|(s, _)| s == "Too long — maximum 4096 UTF-8 bytes")
    );
    assert!(warned.texts.iter().any(|(s, _)| s == "Cancel"));
}
fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}
#[test]
fn empty_replacement_enter_clipboard_and_undo_are_safe() {
    let ctx = egui::Context::default();
    let mut field = Field::default();
    let first = frame(&ctx, &mut field, vec![]);
    frame(&ctx, &mut field, click(at(&first, "New credential")));
    frame(
        &ctx,
        &mut field,
        vec![egui::Event::Text("synthetic-private".into())],
    );
    frame(
        &ctx,
        &mut field,
        vec![key(egui::Key::A, egui::Modifiers::COMMAND)],
    );
    let mut intent = None;
    let output = ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::Copy],
            ..Default::default()
        },
        |ui| {
            intent = field_ui(ui, &mut field, false, true);
        },
    );
    assert!(intent.is_none());
    assert!(
        output.platform_output.commands.is_empty(),
        "password fields cannot copy"
    );
    // An undo command cannot resurrect text retained by the egui field.
    frame(
        &ctx,
        &mut field,
        vec![key(egui::Key::Backspace, egui::Modifiers::NONE)],
    );
    frame(
        &ctx,
        &mut field,
        vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
    );
    assert!(field.draft.0.is_empty());
    let entered = frame(
        &ctx,
        &mut field,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert!(
        entered.intent.is_none(),
        "Enter only leaves the editor, never saves"
    );
    let warned = frame(&ctx, &mut field, vec![]);
    let saved = frame(&ctx, &mut field, click(at(&warned, "Save credential")));
    assert!(matches!(saved.intent,Some(Intent::Save(ref secret)) if secret.0.is_empty()));
    assert!(!format!("{:?}", saved.intent).contains("synthetic-private"));
}
#[test]
fn cancel_preserves_present_and_remove_is_explicit_not_an_empty_save() {
    let ctx = egui::Context::default();
    let mut field = Field {
        state: Some(State::Present),
        ..Default::default()
    };
    let first = frame(&ctx, &mut field, vec![]);
    frame(&ctx, &mut field, click(at(&first, "••••••••")));
    let typed = frame(
        &ctx,
        &mut field,
        vec![egui::Event::Text("cancelled-secret".into())],
    );
    frame(&ctx, &mut field, click(at(&typed, "Outside")));
    let warned = frame(&ctx, &mut field, vec![]);
    assert!(
        frame(&ctx, &mut field, click(at(&warned, "Cancel")))
            .intent
            .is_none()
    );
    let cancelled = frame(&ctx, &mut field, vec![]);
    assert!(field.draft.0.is_empty());
    assert!(cancelled.texts.iter().any(|(s, _)| s == "••••••••"));
    let removed = frame(&ctx, &mut field, click(at(&cancelled, "Remove credential")));
    assert!(matches!(removed.intent, Some(Intent::Remove)));
    field.state = Some(State::Absent);
    assert!(
        !frame(&ctx, &mut field, vec![])
            .texts
            .iter()
            .any(|(s, _)| s == "Remove credential")
    );
}
#[test]
fn saved_hint_pending_and_error_never_expose_or_claim_saved_bytes() {
    let ctx = egui::Context::default();
    let mut field = Field {
        state: Some(State::Present),
        ..Default::default()
    };
    let first = frame(&ctx, &mut field, vec![]);
    assert!(first.texts.iter().any(|(s, _)| s == "••••••••"));
    assert!(field.draft.0.is_empty());
    assert!(first.texts.iter().any(|(s, _)| s == "Remove credential"));
    field.pending = true;
    let pending = frame(&ctx, &mut field, vec![]);
    assert!(
        pending
            .texts
            .iter()
            .any(|(s, _)| s == "Saving — awaiting durable confirmation")
    );
    assert!(
        !pending
            .texts
            .iter()
            .any(|(s, _)| s == "••••••••" || s == "Remove credential")
    );
    field.pending = false;
    field.state = None;
    field.error = Some(crate::session::Error {
        code: "storage_uncertain".into(),
        unknown: true,
    });
    let failed = frame(&ctx, &mut field, vec![]);
    assert!(
        failed
            .texts
            .iter()
            .any(|(s, _)| s.contains("Outcome unknown"))
    );
    assert!(!failed.texts.iter().any(|(s, _)| s == "••••••••"));
}
#[test]
fn focus_type_blur_cancel_and_save_require_explicit_choice() {
    let ctx = egui::Context::default();
    let mut field = Field::default();
    let first = frame(&ctx, &mut field, vec![]);
    assert!(first.intent.is_none());
    let focused = frame(&ctx, &mut field, click(at(&first, "New credential")));
    assert!(focused.intent.is_none());
    let typed = frame(
        &ctx,
        &mut field,
        vec![egui::Event::Text("synthetic-canary".into())],
    );
    assert!(typed.intent.is_none());
    assert!(
        !typed
            .texts
            .iter()
            .any(|(s, _)| s.contains("synthetic-canary") || s == "Save credential")
    );
    frame(&ctx, &mut field, click(at(&typed, "Outside")));
    let warned = frame(&ctx, &mut field, vec![]);
    assert!(warned.intent.is_none());
    let cancelled = frame(&ctx, &mut field, click(at(&warned, "Cancel")));
    assert!(cancelled.intent.is_none());
    assert!(field.draft.0.is_empty());
    let empty = frame(&ctx, &mut field, vec![]);
    frame(&ctx, &mut field, click(at(&empty, "New credential")));
    frame(
        &ctx,
        &mut field,
        vec![egui::Event::Text("synthetic-new".into())],
    );
    let typed = frame(&ctx, &mut field, vec![]);
    frame(&ctx, &mut field, click(at(&typed, "Outside")));
    let warned = frame(&ctx, &mut field, vec![]);
    let saved = frame(&ctx, &mut field, click(at(&warned, "Save credential")));
    assert!(
        matches!(saved.intent,Some(Intent::Save(ref draft)) if draft.0.as_str()=="synthetic-new")
    );
    assert!(field.draft.0.is_empty());
    assert!(frame(&ctx, &mut field, vec![]).intent.is_none());
}

#[test]
fn credential_confirmation_does_not_dispatch_product_shortcuts() {
    use crate::{
        app::App,
        native::{Event, Job},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let root = std::env::temp_dir().join(format!(
        "rad-editor-shortcuts-unused-{}",
        std::process::id()
    ));
    let mut app = App::new(root.join("no-runtime"), root.join("no-theme"));
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    app.worker = crate::worker::Worker::spawn(move |_job: Job| {
        observed.fetch_add(1, Ordering::SeqCst);
        Event {
            plugin_apply: false,
            plugin_admin: false,
            plugin_form: false,
            plugin_visual: false,
            connected: false,
            transfer_job: false,
            result: Err(crate::session::Error::protocol()),
            palette: Default::default(),
        }
    });
    app.connected = true;
    app.control.select(Some("selected-button".into()));
    app.credentials
        .bind("selected", "p:action", &["password".into()]);
    app.credentials.fields.get_mut("password").unwrap().dirty = true;
    app.credentials.fields.get_mut("password").unwrap().confirm = true;
    let ctx = egui::Context::default();
    let _ = ctx.run_ui(
        egui::RawInput {
            events: vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
            ..Default::default()
        },
        |ui| app.shortcuts(ui.ctx()),
    );
    assert!(
        !app.worker.busy(),
        "credential confirmation must not queue a product command"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!app.credential_worker.busy());
    assert!(app.draft.entries.is_empty());
}

#[test]
fn only_final_ok_is_a_credential_ack_not_admitted_or_applied() {
    use rad_client::response::Status;
    for status in [Status::Admitted, Status::Applied] {
        let result = decode_state(serde_json::json!({"state":"present"}), true, status);
        assert!(result.is_err(), "only credential final Ok is an ACK");
        assert!(result.unwrap_err().unknown);
    }
}
