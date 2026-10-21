use rad_editor::native::{self, Job, Reply};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
};
fn exchange(stream: &mut UnixStream, op: &str, status: &str, data: Value) {
    let mut header = [0; 4];
    stream.read_exact(&mut header).unwrap();
    let mut body = vec![0; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut body).unwrap();
    let request: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(request["op"], op);
    let response = json!({"v":1,"id":request["id"],"session_id":"00000000000000000000000000000001","status":status,"data":data,"error":null});
    let bytes = serde_json::to_vec(&response).unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
}
#[test]
fn editor_worker_uses_editor_socket_and_real_client_handshake_before_run() {
    let root = std::env::temp_dir().join(format!("rad-editor-socket-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::create_dir(root.join("rad")).unwrap();
    std::fs::set_permissions(root.join("rad"), std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = UnixListener::bind(root.join("rad/editor.sock")).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        exchange(
            &mut stream,
            "hello",
            "ok",
            json!({"protocol":1,"session_id":"00000000000000000000000000000001","limits":serde_json::from_str::<serde_json::Value>(include_str!("../../rad-api/vectors/limits-v1.json")).unwrap(),"capabilities":[]}),
        );
        exchange(
            &mut stream,
            "run",
            "admitted",
            json!({"execution_id":"00000000000000000000000000000001:1"}),
        );
    });
    let mut worker = native::start(root.clone(), root.join("missing-theme"));
    worker.submit(Job::Connect).unwrap();
    assert!(matches!(
        worker.receive().ready().unwrap().result,
        Ok(Reply::Connected)
    ));
    worker
        .submit(Job::Request(request("run", json!({"id":"b1"}))))
        .unwrap();
    let event = worker.receive().ready().unwrap();
    let Ok(Reply::Response {
        operation,
        response,
    }) = event.result
    else {
        panic!("real Client response")
    };
    assert_eq!(operation, "run");
    assert_eq!(
        serde_json::to_value(&response.data).unwrap()["execution_id"],
        "00000000000000000000000000000001:1"
    );
    server.join().unwrap();
    drop(worker);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reconnect_same_runtime_revision_recovers_pixels_and_new_config_refetches() {
    use image::ImageEncoder;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let root = std::env::temp_dir().join(format!("rad-editor-reconnect-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::create_dir(root.join("rad")).unwrap();
    std::fs::set_permissions(root.join("rad"), std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = UnixListener::bind(root.join("rad/editor.sock")).unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let server_reads = reads.clone();
    let server = std::thread::spawn(move || {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[1, 2, 3, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        let hex: String = png.iter().map(|b| format!("{b:02x}")).collect();
        // Same runtime/session/configuration and visual revision on both connections.
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            loop {
                let mut header = [0; 4];
                if stream.read_exact(&mut header).is_err() {
                    break;
                }
                let size = u32::from_be_bytes(header) as usize;
                assert!(size <= 65536);
                let mut body = vec![0; size];
                stream.read_exact(&mut body).unwrap();
                let request: Value = serde_json::from_slice(&body).unwrap();
                let data = match request["op"].as_str().unwrap() {
                    "hello" => {
                        json!({"protocol":1,"session_id":"00000000000000000000000000000001","limits":serde_json::from_str::<serde_json::Value>(include_str!("../../rad-api/vectors/limits-v1.json")).unwrap(),"capabilities":[]})
                    }
                    "plugin_visual" => {
                        json!({"text_position":"bottom","availability":"current","revision":"3","text":null,"color":null,"image":true})
                    }
                    "plugin_visual_read" => {
                        assert_eq!(request["args"]["revision"], "3");
                        server_reads.fetch_add(1, Ordering::SeqCst);
                        json!({"hex":hex,"next_offset":null})
                    }
                    "status" => break,
                    op => panic!("unexpected operation {op}"),
                };
                let response = json!({"v":1,"id":request["id"],"session_id":"00000000000000000000000000000001","status":"ok","data":data,"error":null});
                let bytes = serde_json::to_vec(&response).unwrap();
                stream
                    .write_all(&(bytes.len() as u32).to_be_bytes())
                    .unwrap();
                stream.write_all(&bytes).unwrap();
            }
        }
    });
    let mut worker = native::start(root.clone(), root.join("missing-theme"));
    let connect = |worker: &mut rad_editor::worker::Worker<Job, rad_editor::native::Event>| {
        worker.submit(Job::Connect).unwrap();
        assert!(matches!(
            worker.receive().ready().unwrap().result,
            Ok(Reply::Connected)
        ));
    };
    let visual = |worker: &mut rad_editor::worker::Worker<Job, rad_editor::native::Event>,
                  epoch: &str| {
        worker
            .submit(Job::PluginVisuals {
                ids: vec!["b1".into()],
                revision: epoch.into(),
            })
            .unwrap();
        let Ok(Reply::PluginVisuals {
            revision,
            mut visuals,
        }) = worker.receive().ready().unwrap().result
        else {
            panic!("visual reply");
        };
        assert_eq!(revision, epoch); // UI can reject late replies from a previous configuration.
        visuals.remove(0)
    };
    connect(&mut worker);
    assert_eq!(
        visual(&mut worker, "7").pixels.unwrap().rgba,
        [1, 2, 3, 255]
    );
    assert!(visual(&mut worker, "7").pixels.is_none());
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    worker
        .submit(Job::Request(request("status", json!({}))))
        .unwrap();
    let disconnected = worker.receive().ready().unwrap();
    assert!(!disconnected.connected && disconnected.result.is_err());
    connect(&mut worker);
    let recovered = visual(&mut worker, "7");
    // The consumer discarded textures on disconnect, despite unchanged revisions.
    assert!(
        recovered.pixels.is_some(),
        "same-runtime reconnect must restore discarded pixels"
    );
    assert_eq!(recovered.pixels.unwrap().rgba, [1, 2, 3, 255]);
    assert!(visual(&mut worker, "7").pixels.is_none());
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert!(visual(&mut worker, "8").pixels.is_some());
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    worker
        .submit(Job::Request(request("status", json!({}))))
        .unwrap();
    assert!(worker.receive().ready().unwrap().result.is_err());
    server.join().unwrap();
    drop(worker);
    std::fs::remove_dir_all(root).unwrap();
}

fn request(op: &str, args: serde_json::Value) -> rad_api::Request {
    rad_api::Request::parse(op, args).expect("valid typed fixture request")
}
