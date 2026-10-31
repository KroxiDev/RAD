use super::*;
struct Frame {
    texts: Vec<(String, egui::Rect)>,
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
            cause: None,
            source_change: None,
            unknown: true,
        }))
        .unwrap();
    loop {
        panel_frame(&ctx, &mut panel, &mut worker, vec![]);
        if let Ok(query) = asked.try_recv() {
            assert!(
                matches!(query.operation, Operation::Status),
                "Unknown only rereads, never repeats the write"
            );
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    answer.send(Ok(State::Absent)).unwrap();
    let failed = loop {
        let f = panel_frame(&ctx, &mut panel, &mut worker, vec![]);
        if f.texts.iter().any(|(s, _)| s == "New credential") {
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
                serde_json::json!({"protocol":1,"session_id":"00000000000000000000000000000001","limits":serde_json::from_str::<serde_json::Value>(include_str!("../../../../rad-api/vectors/limits-v1.json")).unwrap(),"capabilities":[]})
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
    assert!(matches!(worker.poll(), crate::worker::Polled::Pending));
    assert!(worker.busy());
    release.send(()).unwrap();
    let crate::worker::Polled::Ready(event) = worker.receive() else {
        panic!("no event")
    };
    assert_eq!(event.result.unwrap(), State::Present);
    assert_eq!(event.generation, 1);
    assert_eq!(event.slot, "password");
    server.join().unwrap();
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

fn decode_state(
    data: serde_json::Value,
    mutating: bool,
    status: rad_client::response::Status,
) -> Result<State, crate::session::Error> {
    let data = rad_api::ResponseData::decode(rad_api::Operation::CredentialStatus, status, data)
        .map_err(|_| crate::session::Error {
            code: "invalid_request".into(),
            cause: None,
            source_change: None,
            unknown: mutating,
        })?;
    super::decode_state(data, mutating, status)
}
