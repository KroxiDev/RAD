use rad_client::connection::{Client, Endpoint};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};
const SESSION: &str = "00000000000000000000000000000001";
struct Fixture {
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "rad-client-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(path.join("rad")).unwrap();
        fs::set_permissions(path.join("rad"), fs::Permissions::from_mode(0o700)).unwrap();
        Self { path }
    }
    fn listener(&self, name: &str) -> UnixListener {
        let p = self.path.join("rad").join(name);
        let l = UnixListener::bind(&p).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
        l
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}
fn receive(s: &mut UnixStream) -> Value {
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut h = [0; 4];
    s.read_exact(&mut h).unwrap();
    let mut b = vec![0; u32::from_be_bytes(h) as usize];
    s.read_exact(&mut b).unwrap();
    serde_json::from_slice(&b).unwrap()
}
fn send(s: &mut UnixStream, id: &str, data: Value) {
    let b = serde_json::to_vec(
        &json!({"v":1,"id":id,"session_id":SESSION,"status":"ok","data":data,"error":null}),
    )
    .unwrap();
    let mut frame = (b.len() as u32).to_be_bytes().to_vec();
    frame.extend(b);
    for chunk in frame.chunks(7) {
        s.write_all(chunk).unwrap();
    }
}
fn hello(s: &mut UnixStream) {
    assert_eq!(receive(s), json!({"v":1,"id":"1","op":"hello","args":{}}));
    send(
        s,
        "1",
        json!({"protocol":1,"session_id":SESSION,"limits":serde_json::from_str::<serde_json::Value>(include_str!("../../rad-api/vectors/limits-v1.json")).unwrap(),"capabilities":["status","run","result"]}),
    );
}
#[test]
fn connection_uses_selected_endpoint_handshakes_once_and_correlates_fragmented_responses() {
    let f = Fixture::new();
    let l = f.listener("automation.sock");
    let worker = thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        hello(&mut s);
        assert_eq!(
            receive(&mut s),
            json!({"v":1,"id":"2","op":"status","args":{}})
        );
        send(
            &mut s,
            "2",
            json!({"storage_state":"ready","view_revision":"1","active_folder_id":"f1","grid":{"columns":8,"rows":4}}),
        );
        assert_eq!(
            receive(&mut s),
            json!({"v":1,"id":"3","op":"status","args":{}})
        );
        send(
            &mut s,
            "3",
            json!({"storage_state":"uncertain","view_revision":"1","active_folder_id":"f1","grid":{"columns":8,"rows":4}}),
        );
    });
    let mut c = Client::connect(&f.path, Endpoint::Automation).unwrap();
    assert_eq!(
        serde_json::to_value(c.request("status", json!({})).unwrap().data).unwrap()["storage_state"],
        "ready"
    );
    assert_eq!(
        c.request("status", json!({})).unwrap().id.as_deref(),
        Some("3")
    );
    worker.join().unwrap();
}
#[test]
fn lost_mutation_response_is_unknown_and_the_connection_never_replays() {
    let f = Fixture::new();
    let l = f.listener("automation.sock");
    let worker = thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        hello(&mut s);
        assert_eq!(receive(&mut s)["op"], "run");
    });
    let mut c = Client::connect(&f.path, Endpoint::Automation).unwrap();
    let e = c.request("run", json!({"id":"b1"})).unwrap_err();
    assert_eq!(e.outcome, Some(rad_client::response::Outcome::Unknown));
    assert_eq!(e.id, Some(2));
    assert_eq!(e.session_id.as_deref(), Some(SESSION));
    assert_eq!(
        c.request("run", json!({"id":"b1"})).unwrap_err().outcome,
        Some(rad_client::response::Outcome::NotApplied)
    );
    worker.join().unwrap();
}
#[test]
fn invalid_length_and_wrong_request_reply_poison_only_this_client() {
    for invalid_length in [false, true] {
        let f = Fixture::new();
        let l = f.listener("automation.sock");
        let worker = thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            hello(&mut s);
            receive(&mut s);
            if invalid_length {
                s.write_all(&65537u32.to_be_bytes()).unwrap();
            } else {
                send(&mut s, "3", json!({}));
            }
        });
        let mut c = Client::connect(&f.path, Endpoint::Automation).unwrap();
        let error = c.request("status", json!({})).unwrap_err();
        assert_eq!(error.code, "invalid_request");
        assert_eq!(error.outcome, None);
        assert!(c.request("status", json!({})).is_err());
        worker.join().unwrap();
    }
}
#[test]
fn response_got_one_byte_does_not_extend_absolute_deadline() {
    let f = Fixture::new();
    let l = f.listener("automation.sock");
    let worker = thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        hello(&mut s);
        receive(&mut s);
        s.write_all(&[0]).unwrap();
        thread::sleep(Duration::from_millis(100));
        let _ = s.write_all(&[0]);
    });
    let mut c = Client::connect(&f.path, Endpoint::Automation).unwrap();
    let now = std::time::Instant::now();
    let error = c
        .request_until("run", json!({"id":"b1"}), now + Duration::from_millis(25))
        .unwrap_err();
    assert_eq!(error.outcome, Some(rad_client::response::Outcome::Unknown));
    assert!(now.elapsed() < Duration::from_secs(1));
    worker.join().unwrap();
}
#[test]
fn unsafe_runtime_directory_is_rejected_before_connecting() {
    let f = Fixture::new();
    let l = f.listener("automation.sock");
    fs::set_permissions(&f.path, fs::Permissions::from_mode(0o770)).unwrap();
    l.set_nonblocking(true).unwrap();
    let worker = thread::spawn(move || {
        for _ in 0..50 {
            match l.accept() {
                Ok((mut s, _)) => {
                    hello(&mut s);
                    return true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2))
                }
                Err(e) => panic!("{e}"),
            }
        }
        false
    });
    let result = Client::connect(&f.path, Endpoint::Automation);
    let connected = worker.join().unwrap();
    assert!(
        result.is_err(),
        "must reject unsafe XDG runtime permissions"
    );
    assert!(!connected, "must validate directory before any request");
}
fn admitted(s: &mut UnixStream) {
    let b=serde_json::to_vec(&json!({"v":1,"id":"2","session_id":SESSION,"status":"admitted","data":{"execution_id":format!("{SESSION}:1")},"error":null})).unwrap();
    s.write_all(&(b.len() as u32).to_be_bytes()).unwrap();
    s.write_all(&b).unwrap();
}
#[test]
fn cli_wait_sends_run_once_polls_result_and_prints_only_the_final_response() {
    let f = Fixture::new();
    let l = f.listener("automation.sock");
    let worker = thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        hello(&mut s);
        assert_eq!(receive(&mut s)["op"], "run");
        admitted(&mut s);
        let now = std::time::Instant::now();
        for (id, state) in [("3", "running"), ("4", "succeeded")] {
            assert_eq!(
                receive(&mut s),
                json!({"v":1,"id":id,"op":"result","args":{"execution_id":format!("{SESSION}:1")}})
            );
            send(
                &mut s,
                id,
                json!({"execution_id":format!("{SESSION}:1"),"state":state,"steps":[]}),
            );
        }
        assert!(now.elapsed() >= Duration::from_millis(200));
    });
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rad-cli"))
        .env_clear()
        .env("XDG_RUNTIME_DIR", &f.path)
        .args(["run", "b1", "--wait", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["data"]["state"], "succeeded");
    assert_eq!(result["id"], "4");
    assert!(output.stderr.is_empty());
    worker.join().unwrap();
}
#[test]
fn result_wait_deadline_also_bounds_the_first_result_request_and_does_not_cancel() {
    let f = Fixture::new();
    let l = f.listener("automation.sock");
    let worker = thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        hello(&mut s);
        assert_eq!(receive(&mut s)["op"], "result");
        thread::sleep(Duration::from_millis(150));
        let mut byte = [0];
        assert_eq!(
            s.read(&mut byte).unwrap(),
            0,
            "no cancellation or second request"
        );
    });
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rad-cli"))
        .env_clear()
        .env("XDG_RUNTIME_DIR", &f.path)
        .args([
            "result",
            &format!("{SESSION}:1"),
            "--wait",
            "--wait-timeout-ms",
            "25",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(9));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["error"]["code"], "wait_timeout");
    assert_eq!(
        result["error"]["details"]["execution_id"],
        format!("{SESSION}:1")
    );
    assert_eq!(result["id"], "2");
    worker.join().unwrap();
}
#[test]
fn run_cannot_be_reported_successful_by_a_generic_ok_response() {
    let f = Fixture::new();
    let l = f.listener("automation.sock");
    let worker = thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        hello(&mut s);
        receive(&mut s);
        send(&mut s, "2", json!({}));
    });
    let mut c = Client::connect(&f.path, Endpoint::Automation).unwrap();
    let e = c.request("run", json!({"id":"b1"})).unwrap_err();
    assert_eq!(e.code, "invalid_request");
    assert_eq!(e.outcome, Some(rad_client::response::Outcome::Unknown));
    worker.join().unwrap();
}
#[test]
fn cli_get_assembles_all_sections_and_pages_into_one_response() {
    let f = Fixture::new();
    let l = f.listener("automation.sock");
    let worker = thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        hello(&mut s);
        assert_eq!(receive(&mut s)["op"], "get");
        send(
            &mut s,
            "2",
            json!({"visibility":"visible","id":"b1","kind":"button","fields":{"name":"Launch","mode":"sequential","gesture":"press","permissions":{"inherit":true},"effective_permissions":{"consult":true,"modify":true,"execute":true},"folder_id":"f1","position":0},"sections":["actions"],"view_revision":"1"}),
        );
        for (id, cursor, items, next) in [
            (
                "3",
                Value::Null,
                json!([{"visibility":"visible","id":"a1","kind":"action","fields":{"type":"launcher","permissions":{"inherit":true},"effective_permissions":{"consult":true,"modify":true,"execute":true},"parent_id":"b1","parameters":{"desktop_entry":"fixture.desktop"},"folder_id":null},"view_revision":"1","sections":[]}]),
                json!("next"),
            ),
            (
                "4",
                json!("next"),
                json!([{"visibility":"restricted"}]),
                Value::Null,
            ),
        ] {
            assert_eq!(
                receive(&mut s),
                json!({"v":1,"id":id,"op":"get_section","args":{"id":"b1","section":"actions","cursor":cursor}})
            );
            send(
                &mut s,
                id,
                json!({"section":"actions","items":items,"next_cursor":next,"view_revision":"1"}),
            );
        }
    });
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rad-cli"))
        .env_clear()
        .env("XDG_RUNTIME_DIR", &f.path)
        .args(["get", "b1", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["data"]["section_data"]["actions"],
        json!([{"visibility":"visible","id":"a1","kind":"action","fields":{"type":"launcher","permissions":{"inherit":true},"effective_permissions":{"consult":true,"modify":true,"execute":true},"parent_id":"b1","parameters":{"desktop_entry":"fixture.desktop"},"folder_id":null},"view_revision":"1","sections":[]},{"visibility":"restricted"}])
    );
    worker.join().unwrap();
}
#[test]
fn wait_connection_loss_preserves_known_execution_id_for_manual_result_query() {
    let f = Fixture::new();
    let l = f.listener("automation.sock");
    let worker = thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        hello(&mut s);
        receive(&mut s);
        admitted(&mut s);
        assert_eq!(receive(&mut s)["op"], "result");
    });
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_rad-cli"))
        .env_clear()
        .env("XDG_RUNTIME_DIR", &f.path)
        .args(["run", "b1", "--wait", "--json"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(8));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        v["error"]["details"]["execution_id"],
        format!("{SESSION}:1")
    );
    assert_eq!(v["id"], "3");
    assert!(!o.stderr.is_empty());
    worker.join().unwrap();
}
#[test]
fn result_without_wait_rejects_unknown_execution_state_instead_of_exiting_success() {
    let f = Fixture::new();
    let l = f.listener("automation.sock");
    let worker = thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        hello(&mut s);
        receive(&mut s);
        send(
            &mut s,
            "2",
            json!({"execution_id":format!("{SESSION}:1"),"state":"mystery","steps":[]}),
        );
    });
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_rad-cli"))
        .env_clear()
        .env("XDG_RUNTIME_DIR", &f.path)
        .args(["result", &format!("{SESSION}:1"), "--json"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["error"]["code"], "invalid_request");
    worker.join().unwrap();
}

#[test]
fn editor_catalog_failure_is_a_read_error_and_keeps_connection_usable() {
    for operation in [
        "launcher_catalog",
        "plugin_catalog",
        "plugin_configuration",
        "plugin_status",
        "plugin_visual",
        "plugin_visual_read",
    ] {
        let f = Fixture::new();
        let l = f.listener("editor.sock");
        let worker = thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            hello(&mut s);
            assert_eq!(receive(&mut s)["op"], operation);
            let bytes = serde_json::to_vec(&json!({
            "v":1,"id":"2","session_id":SESSION,"status":"error","data":null,
            "error":{"code":"service_unavailable","message":"Catalog not ready","details":{},"outcome":null}
        })).unwrap();
            s.write_all(&(bytes.len() as u32).to_be_bytes()).unwrap();
            s.write_all(&bytes).unwrap();
            if let Ok(mut copy) = s.try_clone() {
                let mut header = [0; 4];
                if copy.read_exact(&mut header).is_ok() {
                    let mut payload = vec![0; u32::from_be_bytes(header) as usize];
                    copy.read_exact(&mut payload).unwrap();
                    assert_eq!(
                        serde_json::from_slice::<Value>(&payload).unwrap()["op"],
                        "status"
                    );
                    send(
                        &mut s,
                        "3",
                        json!({"storage_state":"ready","view_revision":"1","active_folder_id":"f1","grid":{"columns":8,"rows":4}}),
                    );
                }
            }
        });
        let mut c = Client::connect(&f.path, Endpoint::Editor).unwrap();
        let response = c.request(
            operation,
            match operation {
                "launcher_catalog" => json!({"cursor":null,"query":""}),
                "plugin_catalog" => json!({"cursor":null}),
                "plugin_configuration" => json!({"id":"b1","cursor":null}),
                "plugin_visual" => json!({"id":"b1"}),
                "plugin_visual_read" => json!({"id":"b1","revision":"1","offset":"0"}),
                _ => json!({}),
            },
        );
        let followup = c.request("status", json!({}));
        drop(c);
        worker.join().unwrap();
        let response = response.expect("a read-only catalog failure must not poison the client");
        assert_eq!(response.error.unwrap().code, "service_unavailable");
        assert_eq!(
            serde_json::to_value(followup.unwrap().data).unwrap()["storage_state"],
            "ready"
        );
    }
}

#[test]
fn subscription_rejects_positional_notice_arrays() {
    let f = Fixture::new();
    let listener = f.listener("editor.sock");
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        hello(&mut stream);
        assert_eq!(receive(&mut stream)["op"], "subscribe");
        send(&mut stream, "2", json!({}));
        let payload = serde_json::to_vec(&json!([1, SESSION, "board", "2"])).unwrap();
        stream
            .write_all(&(payload.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&payload).unwrap();
    });
    let client = Client::connect(&f.path, Endpoint::Editor).unwrap();
    let mut subscription = client.subscribe().unwrap();
    assert_eq!(subscription.recv().unwrap_err().code, "invalid_request");
    worker.join().unwrap();
}
