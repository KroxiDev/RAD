//! Actual native worker/socket + GUI scheduling: cleanup owns its tokens until ACK.
use super::*;
use std::{
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    sync::{Arc, Mutex},
    time::Duration,
};
const SESSION: &str = "00000000000000000000000000000001";
fn receive(stream: &mut UnixStream) -> Option<Value> {
    let mut header = [0; 4];
    stream.read_exact(&mut header).ok()?;
    let mut bytes = vec![0; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}
fn answer(stream: &mut UnixStream, request: &Value, code: Option<&str>, data: Value) {
    let value = json!({"v":1,"id":request["id"],"session_id":SESSION,"status":if code.is_some(){"error"}else{"ok"},"data":data,
        "error":code.map(|code|json!({"code":code,"message":"synthetic cleanup","details":{},"outcome":"not_applied"}))});
    let bytes = serde_json::to_vec(&value).unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
}
#[test]
fn native_cleanup_busy_keeps_token_and_allows_a_read_before_retrying_only_drop() {
    for library in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "rad-image-cleanup-{}-{library}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(root.join("rad")).unwrap();
        std::fs::set_permissions(root.join("rad"), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = root.join("rad/editor.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        let server = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let end = Instant::now() + Duration::from_secs(3);
            let mut stream = loop {
                if let Ok((stream, _)) = listener.accept() {
                    break stream;
                }
                if Instant::now() >= end {
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut drops = 0;
            while let Some(request) = receive(&mut stream) {
                let op = request["op"].as_str().unwrap();
                record.lock().unwrap().push(op.to_owned());
                match op {
                    "hello" => answer(
                        &mut stream,
                        &request,
                        None,
                        json!({"protocol":1,"session_id":SESSION,"limits":serde_json::from_str::<Value>(include_str!("../../../rad-api/vectors/limits-v1.json")).unwrap(),"capabilities":[]}),
                    ),
                    "transfer_begin" => {
                        answer(&mut stream, &request, None, json!({"transfer_id":"t1"}))
                    }
                    "transfer_write" => answer(&mut stream, &request, None, json!({})),
                    "transfer_finish" => answer(
                        &mut stream,
                        &request,
                        None,
                        json!({"media_ref":"prepared-image"}),
                    ),
                    "edit" => answer(&mut stream, &request, Some("cursor_expired"), Value::Null),
                    "transfer_drop" => {
                        drops += 1;
                        answer(
                            &mut stream,
                            &request,
                            (drops == 1).then_some("resource_exhausted"),
                            if drops == 1 { Value::Null } else { json!({}) },
                        );
                    }
                    "transfer_read" => {
                        answer(&mut stream, &request, None, json!({"base64":"","eof":true}))
                    }
                    _ => answer(
                        &mut stream,
                        &request,
                        Some("resource_exhausted"),
                        Value::Null,
                    ),
                }
            }
        });
        let ctx = egui::Context::default();
        let mut app = App::with_preferences_path(root.join("unused"), root.join("theme"), None);
        app.worker = native::start(root.clone(), root.join("theme"));
        app.worker.submit(Job::Connect).unwrap();
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            std::thread::yield_now();
        }
        if library {
            app.command(Job::SaveIcon { state: None, button: "b2".into(), ticket: 1,
            png: Arc::new(b"\x89PNG\r\n\x1a\n".to_vec()), choice: Default::default(),
            request: serde_json::from_value(json!({"op":"edit","args":{"expected_revision":"1","changes":[
                {"kind":"set","id":"b2","field":"appearance","value":{"text":"Local","image":null}}
            ]}})).unwrap(),
        });
        } else {
            app.discarded_imports.push("t1".into());
            app.pump_reads();
        }
        let end = Instant::now() + Duration::from_secs(2);
        let mut read = false;
        while Instant::now() < end {
            let _ = app.pump_replies(&ctx);
            if !app.worker.busy()
                && !read
                && seen.lock().unwrap().iter().any(|v| v == "transfer_drop")
            {
                app.send(Request::TransferRead {
                    transfer_id: "read-fixture".into(),
                    offset: "0".into(),
                });
                read = true;
            } else if !app.discarded_imports.is_empty() {
                app.pump_reads();
            }
            if seen
                .lock()
                .unwrap()
                .iter()
                .filter(|v| *v == "transfer_drop")
                .count()
                == 2
                && !app.worker.busy()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let diagnostic = (
            app.message.clone(),
            app.connected,
            app.discarded_imports.clone(),
            app.image_cleanup_attempt,
        );
        drop(app);
        server.join().unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.iter().filter(|v| *v == "transfer_drop").count(),
            2,
            "busy cleanup lost its token instead of retrying: {diagnostic:?}; operations={seen:?}"
        );
        let first = seen.iter().position(|v| v == "transfer_drop").unwrap();
        let last = seen.iter().rposition(|v| v == "transfer_drop").unwrap();
        assert!(
            seen[first + 1..last].iter().any(|v| v == "transfer_read"),
            "cleanup backoff must allow reads"
        );
        assert_eq!(
            seen.iter().filter(|v| *v == "edit").count(),
            usize::from(library),
            "cleanup never replays the edit"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn terminal_cleanup_closes_only_its_socket_invalidates_prepared_images_and_reconnects_without_replay()
 {
    for permanent in [true, false] {
        let root = std::env::temp_dir().join(format!(
            "rad-image-terminal-{}-{permanent}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(root.join("rad")).unwrap();
        std::fs::set_permissions(root.join("rad"), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("rad/editor.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        let server = std::thread::spawn(move || {
            for connection in 0..2 {
                let deadline = Instant::now() + Duration::from_secs(6);
                let mut stream = loop {
                    if let Ok((s, _)) = listener.accept() {
                        break s;
                    }
                    if Instant::now() > deadline {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(6)))
                    .unwrap();
                while let Some(request) = receive(&mut stream) {
                    let op = request["op"].as_str().unwrap();
                    record.lock().unwrap().push(op.to_owned());
                    match op {
                        "hello" => answer(
                            &mut stream,
                            &request,
                            None,
                            json!({"protocol":1,"session_id":SESSION,"limits":serde_json::from_str::<Value>(include_str!("../../../rad-api/vectors/limits-v1.json")).unwrap(),"capabilities":[]}),
                        ),
                        "transfer_drop" => answer(
                            &mut stream,
                            &request,
                            Some(if permanent {
                                "storage_error"
                            } else {
                                "resource_exhausted"
                            }),
                            Value::Null,
                        ),
                        "transfer_read" => {
                            answer(&mut stream, &request, None, json!({"base64":"","eof":true}));
                            if connection == 1 {
                                return;
                            }
                        }
                        _ => answer(
                            &mut stream,
                            &request,
                            Some("resource_exhausted"),
                            Value::Null,
                        ),
                    }
                }
                record.lock().unwrap().push("disconnected".into());
            }
        });
        let ctx = egui::Context::default();
        let mut app = App::with_preferences_path(root.join("unused"), root.join("theme"), None);
        app.es = crate::preferences::Language::En;
        app.worker = native::start(root.clone(), root.join("theme"));
        app.worker.submit(Job::Connect).unwrap();
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            std::thread::yield_now();
        }
        app.adopt_inspection(Inspection {button:json!({"id":"b2","view_revision":"1","fields":{"name":"Key","mode":"sequential","gesture":"press"}}),appearance:json!({"text":"Base","image":"stored-image","visual_type":"org.rad.audio#microphone","states":{"muted":{"text":"Quiet","image":null}}}),actions:vec![]});
        app.draft.select_state("muted".into());
        app.draft.text = "Safe local text".into();
        app.draft.image = "unpublished-image".into();
        app.prepared_imports = vec![("prepared-token".into(), "unpublished-image".into())];
        app.draft.select_state("unmuted".into());
        app.draft.select_state("muted".into());
        app.queue_image_cleanup(["discard-token".into()]);
        let end = Instant::now() + Duration::from_secs(5);
        while app.connected && Instant::now() < end {
            app.pump_reads();
            let _ = app.pump_replies(&ctx);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            !app.connected,
            "terminal cleanup did not close its own IPC session"
        );
        assert_eq!(app.draft.text, "Safe local text");
        assert!(app.draft.image.is_empty());
        assert_eq!(app.draft.base_face.image.as_deref(), Some("stored-image"));
        assert!(app.prepared_imports.is_empty() && app.discarded_imports.is_empty());
        assert!(app.message.contains("Your text is still here"));
        app.reconnect_needed = false;
        app.worker.submit(Job::Connect).unwrap();
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            std::thread::yield_now();
        }
        assert!(app.connected);
        app.send(Request::TransferRead {
            transfer_id: "fresh-read".into(),
            offset: "0".into(),
        });
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            std::thread::yield_now();
        }
        // A subsequent explicit Save cannot reference any lease of the closed socket.
        let (saved, edits) = std::sync::mpsc::channel();
        app.worker = Worker::spawn(move |job| {
            if let Job::Request(request @ Request::Edit { .. }) = job {
                saved.send(serde_json::to_value(request).unwrap()).unwrap();
            }
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
                result: Ok(Reply::Theme),
            }
        });
        app.save();
        let saved = edits.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!saved.to_string().contains("unpublished-image"));
        assert!(saved.to_string().contains("Safe local text"));
        drop(app);
        server.join().unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.iter().filter(|op| *op == "hello").count(), 2);
        assert!(seen.iter().any(|op| op == "disconnected"));
        assert_eq!(
            seen.iter().filter(|op| *op == "transfer_drop").count(),
            if permanent { 1 } else { 4 }
        );
        assert!(!seen.iter().any(|op| op == "edit" || op == "run"));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn cleanup_backoff_requests_an_egui_deadline_and_progresses_without_polling() {
    let ctx = egui::Context::default();
    for _ in 0..3 {
        let _ = ctx.run_ui(Default::default(), |_| {});
    }
    let (wake, deadlines) = std::sync::mpsc::channel();
    ctx.set_request_repaint_callback(move |request| {
        let _ = wake.send(request.delay);
    });
    let mut app = App::with_preferences_path("unused".into(), "unused-theme".into(), None);
    app.connected = true;
    app.force = false;
    app.styled = Some(app.palette.clone());
    let (sent, jobs) = std::sync::mpsc::channel();
    app.worker = Worker::spawn(move |job| {
        sent.send(job).unwrap();
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
            result: Ok(Reply::Theme),
        }
    });
    app.image_cleanup_result(crate::icon_save::Cleanup {
        retry: vec!["owned-token".into()],
        ..Default::default()
    });
    let _ = ctx.run_ui(Default::default(), |ui| app.pump(ui.ctx()));
    let delay = deadlines
        .try_iter()
        .find(|delay| !delay.is_zero() && *delay <= Duration::from_millis(200))
        .expect("quiet UI must schedule a repaint at the cleanup deadline");
    // egui requests its wake one predicted frame early. Model that frame plus the
    // requested wait, not a loop that pumps the application every millisecond.
    let frame = ctx.input(|input| Duration::from_secs_f32(input.predicted_dt));
    std::thread::sleep(delay + frame + Duration::from_millis(2));
    while app.worker.busy() {
        let _ = app.pump_replies(&ctx);
        std::thread::yield_now();
    }
    let _ = ctx.run_ui(Default::default(), |ui| app.pump(ui.ctx()));
    let end = Instant::now() + Duration::from_secs(1);
    loop {
        let job = jobs
            .recv_timeout(end.saturating_duration_since(Instant::now()))
            .unwrap();
        if let Job::DropImages { tokens, .. } = job {
            assert_eq!(tokens, ["owned-token"]);
            break;
        }
    }
}
