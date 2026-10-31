use rad_transport::{Origin, Transport, TransportError};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "rad-t-review-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        Self(p)
    }
    fn client(&self, name: &str) -> UnixStream {
        let s = UnixStream::connect(self.0.join("rad").join(name)).unwrap();
        s.set_nonblocking(true).unwrap();
        s
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn send(c: &mut UnixStream, id: u64, op: &str) {
    let s = format!(r#"{{"v":1,"id":"{id}","op":"{op}","args":{{}}}}"#);
    c.write_all(&(s.len() as u32).to_be_bytes()).unwrap();
    c.write_all(s.as_bytes()).unwrap();
}

#[test]
fn old_disconnected_completion_cannot_route_to_reconnected_peer_same_runtime() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 9).unwrap();
    let mut a = f.client("automation.sock");
    send(&mut a, 1, "hello");
    t.drive(1).unwrap();
    let old = t.take_next().unwrap();
    assert_eq!(old.origin, Origin::Automation);
    drop(a);
    t.drive(2).unwrap();
    let mut b = f.client("editor.sock");
    send(&mut b, 1, "hello");
    t.drive(3).unwrap();
    let new = t.take_next().unwrap();
    assert_ne!(old.key.connection(), new.key.connection());
    assert_eq!(t.request_count(), 2);
    t.finish(old.key, b"old").unwrap();
    t.drive(4).unwrap();
    let mut bytes = [0u8; 32];
    assert_eq!(
        b.read(&mut bytes).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    t.finish(new.key, b"new").unwrap();
    t.drive(5).unwrap();
    let n = b.read(&mut bytes).unwrap();
    assert_eq!(&bytes[..n], b"\0\0\0\x03new");
    assert_eq!(
        t.finish(old.key, b"old"),
        Err(TransportError::UnknownRequest)
    );
}
#[test]
fn deadline_wins_over_ready_final_byte_and_other_client_is_not_starved() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 11).unwrap();
    let mut a = f.client("automation.sock");
    let mut b = f.client("editor.sock");
    send(&mut a, 1, "hello");
    send(&mut b, 1, "hello");
    t.drive(0).unwrap();
    while let Some(r) = t.take_next() {
        t.finish(r.key, b"{}").unwrap();
    }
    t.drive(1).unwrap();
    a.write_all(&[0]).unwrap();
    t.drive(100).unwrap();
    // Completion was already ready when deadline-first drive runs: it must not be admitted.
    let body = r#"{"v":1,"id":"2","op":"run","args":{}}"#;
    let frame = [
        (body.len() as u32).to_be_bytes().as_slice(),
        body.as_bytes(),
    ]
    .concat();
    a.write_all(&frame[1..]).unwrap();
    send(&mut b, 2, "status");
    t.wait_readiness(std::time::Duration::ZERO).unwrap();
    t.drive(2100).unwrap();
    assert_eq!(t.connection_count(), 1);
    let request = t.take_next().unwrap();
    assert_eq!(request.origin, Origin::Editor);
    assert_eq!(request.request.operation(), "status");
    assert!(t.take_next().is_none());
}
