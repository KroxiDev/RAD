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
#[test]
fn owns_two_private_endpoints_and_holds_instance_lock() {
    let fixture = Fixture::new();
    let transport = Transport::bind(&fixture.0, 1).unwrap();
    for name in ["editor.sock", "automation.sock"] {
        let p = fixture.0.join("rad").join(name);
        assert_eq!(fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert!(matches!(
        Transport::bind(&fixture.0, 1),
        Err(TransportError::InstanceBusy)
    ));
    drop(transport);
    assert!(!fixture.0.join("rad/editor.sock").exists());
    assert!(Transport::bind(&fixture.0, 1).is_ok());
}
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
};
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
fn real_fragmented_request_is_fifo_and_origin_comes_from_listener() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut auto = connect(&f, "automation.sock");
    let mut editor = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    auto.write_all(&[0, 0]).unwrap();
    t.drive(1).unwrap();
    assert!(t.take_next().is_none());
    let bytes = [
        (HELLO.len() as u32).to_be_bytes().as_slice(),
        HELLO.as_bytes(),
    ]
    .concat();
    auto.write_all(&bytes[2..]).unwrap();
    t.drive(2).unwrap();
    let a = t.take_next().unwrap();
    assert_eq!(a.origin, Origin::Automation);
    assert_eq!(a.key.id(), 1);
    send(&mut editor, HELLO);
    t.drive(3).unwrap();
    let e = t.take_next().unwrap();
    assert_eq!(e.origin, Origin::Editor);
    assert_ne!(a.key.connection(), e.key.connection());
    assert!(t.take_next().is_none());
}
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
#[test]
fn finish_sends_exact_frame_once_and_releases_started_request() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut client = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    send(&mut client, HELLO);
    t.drive(1).unwrap();
    let incoming = t.take_next().unwrap();
    t.finish(incoming.key, b"{}").unwrap();
    t.drive(2).unwrap();
    let mut bytes = [0; 16];
    let n = client.read(&mut bytes).unwrap();
    assert_eq!(&bytes[..n], &[0, 0, 0, 2, b'{', b'}']);
    assert_eq!(
        t.finish(incoming.key, b"{}"),
        Err(TransportError::UnknownRequest)
    );
}
fn status(id: u64) -> String {
    format!(r#"{{"v":1,"id":"{id}","op":"status","args":{{}}}}"#)
}
#[test]
fn per_connection_four_inflight_rejects_fifth_without_second_queue() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    send(&mut c, HELLO);
    for id in 2..=5 {
        send(&mut c, &status(id));
    }
    t.drive(1).unwrap();
    assert_eq!(t.request_count(), 4);
    let mut got = Vec::new();
    while let Some(r) = t.take_next() {
        got.push(r);
    }
    assert_eq!(got.len(), 4);
    assert_eq!(t.request_count(), 4);
    let mut response = [0; 1024];
    let n = c.read(&mut response).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&response[4..n]).unwrap();
    assert_eq!(v["error"]["code"], "resource_exhausted");
    assert_eq!(v["id"], "5");
    t.finish(got[0].key, b"{}").unwrap();
    send(&mut c, &status(6));
    t.drive(2).unwrap();
    assert_eq!(t.take_next().unwrap().key.id(), 6);
    assert_eq!(t.request_count(), 4);
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
fn hello_deadline_absolute_two_seconds_cannot_be_extended_by_goteo() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(100).unwrap();
    c.write_all(&[0]).unwrap();
    t.drive(1000).unwrap();
    c.write_all(&[0]).unwrap();
    t.drive(2099).unwrap();
    assert_eq!(t.connection_count(), 1);
    t.drive(2100).unwrap();
    assert_eq!(t.connection_count(), 0);
}
#[test]
fn frame_deadline_starts_at_first_byte_and_is_absolute_after_hello() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    send(&mut c, HELLO);
    t.drive(1).unwrap();
    let r = t.take_next().unwrap();
    t.finish(r.key, b"{}").unwrap();
    t.drive(2).unwrap();
    c.write_all(&[0]).unwrap();
    t.drive(3000).unwrap();
    c.write_all(&[0]).unwrap();
    t.drive(4999).unwrap();
    assert_eq!(t.connection_count(), 1);
    t.drive(5000).unwrap();
    assert_eq!(t.connection_count(), 0);
}
#[test]
fn idle_deadline_is_sixty_seconds_since_received_byte_not_response() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    send(&mut c, HELLO);
    t.drive(1).unwrap();
    let r = t.take_next().unwrap();
    t.drive(59999).unwrap();
    t.finish(r.key, b"{}").unwrap();
    t.drive(60000).unwrap();
    assert_eq!(t.connection_count(), 1);
    t.drive(60001).unwrap();
    assert_eq!(t.connection_count(), 0);
}
#[test]
fn queued_request_expires_at_five_seconds_but_started_is_retained() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    send(&mut c, HELLO);
    t.drive(1).unwrap();
    let started = t.take_next().unwrap();
    send(&mut c, r#"{"v":1,"id":"2","op":"run","args":{}}"#);
    t.drive(2).unwrap();
    t.drive(5001).unwrap();
    assert_eq!(t.request_count(), 2);
    t.drive(5002).unwrap();
    assert_eq!(t.request_count(), 1);
    assert!(t.take_next().is_none());
    let mut response = [0; 1024];
    let n = c.read(&mut response).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&response[4..n]).unwrap();
    assert_eq!(v["error"]["code"], "deadline_exceeded");
    assert_eq!(v["error"]["outcome"], "not_applied");
    t.finish(started.key, b"{}").unwrap();
    assert_eq!(t.request_count(), 0);
}
#[test]
fn output_deadline_runs_from_enqueue_before_any_attempt_to_write() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    send(&mut c, HELLO);
    t.drive(1).unwrap();
    let r = t.take_next().unwrap();
    t.finish(r.key, b"{}").unwrap();
    t.drive(2001).unwrap();
    assert_eq!(t.connection_count(), 0);
    let mut bytes = [0; 16];
    assert_eq!(c.read(&mut bytes).unwrap(), 0);
}
#[test]
fn slow_reader_is_disconnected_at_bounded_output_without_cancelling_started() {
    for size in [4096, 65536] {
        let f = Fixture::new();
        let mut t = Transport::bind(&f.0, 1).unwrap();
        let mut c = connect(&f, "editor.sock");
        t.drive(0).unwrap();
        send(&mut c, HELLO);
        t.drive(1).unwrap();
        let held = t.take_next().unwrap();
        for id in 2..=512 {
            send(&mut c, &status(id));
            t.drive(1).unwrap();
            let r = t.take_next().unwrap();
            t.finish(r.key, &vec![b' '; size]).unwrap();
            if t.connection_count() == 0 {
                break;
            }
        }
        assert_eq!(t.connection_count(), 0, "unbounded output for {size}");
        assert_eq!(t.request_count(), 1);
        t.finish(held.key, b"{}").unwrap();
        assert_eq!(t.request_count(), 0);
    }
}
#[test]
fn correlated_wire_error_is_sent_before_close_without_admitting_request() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    send(&mut c, r#"{"v":2,"id":"1","op":"hello","args":{}}"#);
    t.drive(1).unwrap();
    assert!(t.take_next().is_none());
    let mut bytes = [0; 1024];
    let n = c.read(&mut bytes).unwrap();
    assert!(n > 4);
    let v: serde_json::Value = serde_json::from_slice(&bytes[4..n]).unwrap();
    assert_eq!(v["error"]["code"], "unsupported_version");
    assert_eq!(v["id"], "1");
    assert_eq!(t.connection_count(), 0);
}
#[test]
fn clock_reversal_and_unrepresentable_deadlines_do_not_process_io() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(100).unwrap();
    send(&mut c, HELLO);
    assert_eq!(t.drive(99), Err(TransportError::TimeReversed));
    assert!(t.take_next().is_none());
    assert_eq!(t.drive(u64::MAX), Err(TransportError::TimeOverflow));
    assert_eq!(t.connection_count(), 1);
    t.drive(100).unwrap();
    assert_eq!(t.take_next().unwrap().key.id(), 1);
}
#[test]
fn zero_session_is_rejected_before_creating_instance_paths() {
    let f = Fixture::new();
    assert!(matches!(
        Transport::bind(&f.0, 0),
        Err(TransportError::InvalidSession)
    ));
    assert!(!f.0.join("rad").exists());
}
#[test]
fn next_deadline_tracks_partial_frames_and_pending_readiness_without_idle_spin() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    assert_eq!(t.next_deadline(), None);
    let mut c = connect(&f, "editor.sock");
    t.drive(100).unwrap();
    assert_eq!(t.next_deadline(), Some(2100));
    send(&mut c, HELLO);
    t.drive(101).unwrap();
    let r = t.take_next().unwrap();
    assert_eq!(t.next_deadline(), Some(60101));
    t.finish(r.key, b"{}").unwrap();
    assert_eq!(t.next_deadline(), Some(101));
    t.drive(102).unwrap();
    assert_eq!(t.next_deadline(), Some(60101));
    c.write_all(&[0]).unwrap();
    t.drive(200).unwrap();
    assert_eq!(t.next_deadline(), Some(2200));
}
#[test]
fn readiness_wait_records_edges_without_io_then_drive_uses_fresh_clock() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.wait_readiness(std::time::Duration::ZERO).unwrap();
    assert_eq!(t.connection_count(), 0);
    assert_eq!(t.next_deadline(), Some(0));
    t.drive(100).unwrap();
    send(&mut c, HELLO);
    t.wait_readiness(std::time::Duration::ZERO).unwrap();
    assert_eq!(t.request_count(), 0);
    assert_eq!(t.next_deadline(), Some(100));
    t.drive(2100).unwrap();
    assert_eq!(t.connection_count(), 0);
    assert!(t.take_next().is_none());
}
#[test]
fn private_directories_reject_special_permission_bits_without_repairing_them() {
    let f = Fixture::new();
    fs::set_permissions(&f.0, fs::Permissions::from_mode(0o1700)).unwrap();
    assert!(matches!(
        Transport::bind(&f.0, 1),
        Err(TransportError::UnsafePath)
    ));
    assert_eq!(
        fs::metadata(&f.0).unwrap().permissions().mode() & 0o7777,
        0o1700
    );
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
fn only_stale_owned_socket_is_removed_and_live_listener_is_never_unlinked() {
    use std::os::unix::{fs::MetadataExt, net::UnixListener};
    let f = Fixture::new();
    let rad = f.0.join("rad");
    fs::create_dir(&rad).unwrap();
    fs::set_permissions(&rad, fs::Permissions::from_mode(0o700)).unwrap();
    let path = rad.join("editor.sock");
    let live = UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let ino = fs::metadata(&path).unwrap().ino();
    assert!(matches!(
        Transport::bind(&f.0, 1),
        Err(TransportError::InstanceBusy)
    ));
    assert_eq!(fs::metadata(&path).unwrap().ino(), ino);
    drop(live);
    let t = Transport::bind(&f.0, 1).unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"replacement").unwrap();
    drop(t);
    assert_eq!(fs::read(&path).unwrap(), b"replacement");
    assert!(!rad.join("automation.sock").exists());
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
#[test]
fn maximum_frame_and_coalesced_backlog_continue_without_another_edge() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    send(&mut c, HELLO);
    t.drive(1).unwrap();
    let r = t.take_next().unwrap();
    t.finish(r.key, b"{}").unwrap();
    let mut max = status(2);
    max.push_str(&" ".repeat(65536 - max.len()));
    send(&mut c, &max);
    t.drive(2).unwrap();
    assert!(t.take_next().is_none());
    assert_eq!(t.next_deadline(), Some(2));
    t.drive(2).unwrap();
    assert_eq!(t.take_next().unwrap().key.id(), 2);
    for id in 3..=20 {
        send(&mut c, &status(id));
    }
    for _ in 0..8 {
        t.drive(3).unwrap();
        while let Some(r) = t.take_next() {
            t.finish(r.key, b"{}").unwrap();
        }
    }
    assert!(t.connection_count() > 0);
    assert!(t.next_deadline().is_some_and(|n| n > 3));
}
#[test]
fn disconnect_drops_only_queued_and_reconnect_requires_new_hello() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    send(&mut c, HELLO);
    t.drive(1).unwrap();
    let held = t.take_next().unwrap();
    send(&mut c, &status(2));
    t.drive(2).unwrap();
    assert_eq!(t.request_count(), 2);
    drop(c);
    t.drive(3).unwrap();
    assert_eq!(t.request_count(), 1);
    assert!(t.take_next().is_none());
    let mut c = connect(&f, "editor.sock");
    t.drive(4).unwrap();
    send(&mut c, &status(2));
    t.drive(5).unwrap();
    assert_eq!(t.connection_count(), 0);
    assert_eq!(t.request_count(), 1);
    t.finish(held.key, b"{}").unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(6).unwrap();
    send(&mut c, HELLO);
    t.drive(7).unwrap();
    let r = t.take_next().unwrap();
    assert_ne!(r.key.connection(), held.key.connection());
    assert_eq!(r.key.id(), 1);
}
#[test]
fn nonobject_args_reports_error_consumes_id_and_keeps_handshake_state() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    send(&mut c, HELLO);
    t.drive(1).unwrap();
    t.take_next().unwrap();
    send(&mut c, r#"{"v":1,"id":"2","op":"status","args":[]}"#);
    send(&mut c, &status(3));
    t.drive(2).unwrap();
    assert_eq!(t.take_next().unwrap().key.id(), 3);
    assert_eq!(t.connection_count(), 1);
    let mut bytes = [0; 1024];
    let n = c.read(&mut bytes).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes[4..n]).unwrap();
    assert_eq!(value["error"]["code"], "invalid_request");
    assert_eq!(value["id"], "2");
    assert_eq!(value["session_id"], "00000000000000000000000000000001");
    send(&mut c, &status(3));
    t.drive(3).unwrap();
    assert_eq!(t.connection_count(), 0);
    assert!(t.take_next().is_none());
}
#[test]
fn stale_completion_from_previous_runtime_never_completes_new_request() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    send(&mut c, HELLO);
    t.drive(1).unwrap();
    let old = t.take_next().unwrap().key;
    drop(t);
    drop(c);
    let mut t = Transport::bind(&f.0, 2).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    send(&mut c, HELLO);
    t.drive(1).unwrap();
    let current = t.take_next().unwrap().key;
    assert_eq!(t.finish(old, b"{}"), Err(TransportError::UnknownRequest));
    assert_eq!(t.request_count(), 1);
    t.finish(current, b"{}").unwrap();
}
#[test]
fn runtime_path_that_clients_cannot_address_is_rejected_before_creating_rad() {
    let f = Fixture::new();
    let long = f.0.join("x".repeat(100));
    fs::create_dir(&long).unwrap();
    fs::set_permissions(&long, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(
        Transport::bind(&long, 1),
        Err(TransportError::UnsafePath)
    ));
    assert!(!long.join("rad").exists());
}
#[test]
fn largest_clock_epoch_is_checked_and_invalid_completion_preserves_slot() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    let now = u64::MAX - 60000;
    t.drive(now).unwrap();
    send(&mut c, HELLO);
    t.drive(now).unwrap();
    let r = t.take_next().unwrap();
    assert_eq!(t.next_deadline(), Some(u64::MAX));
    assert_eq!(t.drive(now + 1), Err(TransportError::TimeOverflow));
    assert_eq!(t.finish(r.key, b""), Err(TransportError::InvalidResponse));
    assert_eq!(
        t.finish(r.key, &vec![0; 65537]),
        Err(TransportError::InvalidResponse)
    );
    assert_eq!(t.request_count(), 1);
    t.finish(r.key, b"{}").unwrap();
    assert_eq!(t.request_count(), 0);
}

#[test]
fn pre_admission_rejections_preserve_cause_and_mutation_outcome() {
    for operation in [
        "set_plugin_settings",
        "plugin_retry",
        "plugin_admin",
        "credential_replace",
        "credential_remove",
        "transfer_write",
        "transfer_read",
        "media_read",
        "status",
    ] {
        for exhausted in [false, true] {
            let f = Fixture::new();
            let mut t = Transport::bind(&f.0, 1).unwrap();
            let mut c = connect(&f, "editor.sock");
            t.drive(0).unwrap();
            send(&mut c, HELLO);
            t.drive(1).unwrap();
            let _hello = t.take_next().unwrap();
            if exhausted {
                for id in 2..=4 {
                    send(&mut c, &status(id));
                }
                t.drive(2).unwrap();
                while t.take_next().is_some() {}
            }
            send(
                &mut c,
                &format!(r#"{{"v":1,"id":"5","op":"{operation}","args":{{}}}}"#),
            );
            t.drive(3).unwrap();
            if !exhausted {
                t.drive(5003).unwrap();
            }
            let mut bytes = [0; 2048];
            let n = c.read(&mut bytes).unwrap();
            let response: serde_json::Value = serde_json::from_slice(&bytes[4..n]).unwrap();
            assert_eq!(
                response["error"]["code"],
                if exhausted {
                    "resource_exhausted"
                } else {
                    "deadline_exceeded"
                }
            );
            assert!(
                rad_api::Response::decode_value(
                    response.clone(),
                    5,
                    Some("00000000000000000000000000000001"),
                    rad_api::Operation::parse(operation).unwrap()
                )
                .is_ok()
            );
            let mutation = !matches!(operation, "transfer_read" | "media_read" | "status");
            assert_eq!(
                response["error"]["outcome"],
                if mutation {
                    serde_json::json!("not_applied")
                } else {
                    serde_json::Value::Null
                },
                "{operation}, exhausted={exhausted}"
            );
        }
    }
}

#[test]
fn correlated_wire_rejection_keeps_known_mutation_not_applied() {
    let f = Fixture::new();
    let mut t = Transport::bind(&f.0, 1).unwrap();
    let mut c = connect(&f, "editor.sock");
    t.drive(0).unwrap();
    send(&mut c, HELLO);
    t.drive(1).unwrap();
    let _hello = t.take_next().unwrap();
    send(
        &mut c,
        r#"{"v":2,"id":"2","op":"plugin_retry","args":{"plugin_id":"fixture"}}"#,
    );
    t.drive(2).unwrap();
    let mut bytes = [0; 2048];
    let n = c.read(&mut bytes).unwrap();
    let response: serde_json::Value = serde_json::from_slice(&bytes[4..n]).unwrap();
    assert_eq!(response["error"]["code"], "unsupported_version");
    assert_eq!(response["error"]["outcome"], "not_applied");
    assert!(t.take_next().is_none());
}
