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

fn admitted(s: &mut UnixStream) {
    let b=serde_json::to_vec(&json!({"v":1,"id":"2","session_id":SESSION,"status":"admitted","data":{"execution_id":format!("{SESSION}:1")},"error":null})).unwrap();
    s.write_all(&(b.len() as u32).to_be_bytes()).unwrap();
    s.write_all(&b).unwrap();
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
