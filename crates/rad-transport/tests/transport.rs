use rad_transport::*;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rad-transport-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

use std::{io::Write, os::unix::net::UnixStream};
fn connect(f: &Fixture, endpoint: &str) -> UnixStream {
    let s = UnixStream::connect(f.0.join("rad").join(endpoint)).unwrap();
    s.set_nonblocking(true).unwrap();
    s
}
fn send(s: &mut UnixStream, json: &str) {
    let bytes = [
        (json.len() as u32).to_be_bytes().as_slice(),
        json.as_bytes(),
    ]
    .concat();
    s.write_all(&bytes).unwrap();
}
const HELLO: &str = r#"{"v":1,"id":"1","op":"hello","args":{}}"#;

#[test]
fn connection_caps_reserve_two_editor_slots_and_release_on_disconnect() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut clients = Vec::new();
    for _ in 0..7 {
        clients.push(connect(&f, "automation.sock"));
        t.drive(0).unwrap();
    }
    assert_eq!(t.connection_count(), 6);
    for _ in 0..3 {
        clients.push(connect(&f, "editor.sock"));
        t.drive(0).unwrap();
    }
    assert_eq!(t.connection_count(), 8);
    drop(clients);
    t.drive(1).unwrap();
    assert_eq!(t.connection_count(), 0);
}

fn status(id: u64) -> String {
    format!(r#"{{"v":1,"id":"{id}","op":"status","args":{{}}}}"#)
}

#[test]
fn global_32_includes_started_disconnected_and_only_finish_releases_them() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut clients = Vec::new();
    for _ in 0..8 {
        let mut c = connect(&f, "editor.sock");
        t.drive(1).unwrap();
        send(&mut c, HELLO);
        for id in 2..=4 {
            send(&mut c, &status(id));
        }
        t.drive(1).unwrap();
        clients.push(c);
    }
    let mut started = Vec::new();
    while let Some(r) = t.take_next() {
        started.push(r.key);
    }
    assert_eq!(started.len(), 32);
    drop(clients);
    t.drive(2).unwrap();
    assert_eq!(t.request_count(), 32);
    let mut c = connect(&f, "editor.sock");
    t.drive(3).unwrap();
    send(&mut c, HELLO);
    t.drive(4).unwrap();
    assert_eq!(t.request_count(), 32);
    assert!(t.take_next().is_none());
    t.finish(started[0], b"{}").unwrap();
    send(&mut c, &status(2));
    t.drive(5).unwrap();
    assert_eq!(t.take_next().unwrap().key.id(), 2);
    assert_eq!(t.request_count(), 32);
}

#[test]
fn unsafe_paths_symlinks_and_hardlinked_lock_are_preserved() {
    use std::os::unix::fs::symlink;
    for kind in 0..5 {
        let f = Fixture::new();
        let target = f.0.join("target");
        fs::write(&target, b"keep").unwrap();
        let rad = f.0.join("rad");
        if kind == 0 {
            symlink(&target, &rad).unwrap();
        } else {
            fs::create_dir(&rad).unwrap();
            fs::set_permissions(&rad, fs::Permissions::from_mode(0o700)).unwrap();
            match kind {
                1 => symlink(&target, rad.join("instance.lock")).unwrap(),
                2 => fs::hard_link(&target, rad.join("instance.lock")).unwrap(),
                3 => symlink(&target, rad.join("editor.sock")).unwrap(),
                _ => {
                    fs::set_permissions(&rad, fs::Permissions::from_mode(0o755)).unwrap();
                }
            }
        }
        assert!(matches!(
            Transport::bind(&f.0, 1),
            Err(TransportError::UnsafePath)
        ));
        assert_eq!(fs::read(&target).unwrap(), b"keep");
    }
    let f = Fixture::new();
    let alias = f.0.join("alias");
    symlink(&f.0, &alias).unwrap();
    assert!(matches!(
        Transport::bind(&alias, 1),
        Err(TransportError::UnsafePath)
    ));
    assert!(matches!(
        Transport::bind(std::path::Path::new("relative"), 1),
        Err(TransportError::UnsafePath)
    ));
}

#[test]
fn framing_bounds_malformed_utf8_and_untrusted_role_never_reach_authority() {
    for bytes in [
        0u32.to_be_bytes().to_vec(),
        65537u32.to_be_bytes().to_vec(),
        vec![0, 0, 0, 1, 255],
        b"\0\0\0\x02{}".to_vec(),
    ] {
        let f = Fixture::new();
        let mut t = Transport::bind(&f.0, 1).unwrap();
        let mut c = connect(&f, "editor.sock");
        t.drive(0).unwrap();
        c.write_all(&bytes).unwrap();
        t.drive(1).unwrap();
        assert_eq!(t.connection_count(), 0);
        assert!(t.take_next().is_none());
    }
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "automation.sock");
    t.drive(0).unwrap();
    send(&mut c, HELLO);
    t.drive(1).unwrap();
    t.take_next().unwrap();
    send(
        &mut c,
        r#"{"v":1,"id":"2","op":"admin","args":{"client_role":"editor"}}"#,
    );
    t.drive(2).unwrap();
    let r = t.take_next().unwrap();
    assert_eq!(r.origin, Origin::Automation);
}
