use super::*;
fn connection() -> Connection {
    let (stream, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    stream.set_nonblocking(true).unwrap();
    Connection {
        stream: UnixStream::from_std(stream),
        origin: Origin::Editor,
        wire: rad_wire::Session::default(),
        input: Vec::new(),
        readable: false,
        writable: false,
        output: VecDeque::new(),
        accepted: 0,
        last_received: 0,
        hello: true,
        close_after_flush: false,
        frame_started: None,
    }
}
#[test]
fn output_caps_are_inclusive_and_account_for_headers_and_retained_partial_frames() {
    let mut c = connection();
    for _ in 0..16 {
        assert!(Transport::enqueue(&mut c, b"x", 0));
    }
    assert!(!Transport::enqueue(&mut c, b"x", 0));
    let mut c = connection();
    for _ in 0..15 {
        assert!(Transport::enqueue(&mut c, &vec![0; 65536], 0));
    }
    assert!(!Transport::enqueue(&mut c, &vec![0; 65473], 0));
    assert!(Transport::enqueue(&mut c, &vec![0; 65472], 0));
    assert_eq!(
        c.output.iter().map(|o| o.bytes.len()).sum::<usize>(),
        1048576
    );
    c.output.front_mut().unwrap().offset = 65539;
    assert!(!Transport::enqueue(&mut c, b"x", 0));
}
#[test]
fn expiring_multiple_queued_requests_survives_output_overflow_disconnect() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("rad-transport-expiry-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut t = Transport::bind(&dir, 1).unwrap();
    let mut c = connection();
    for _ in 0..16 {
        assert!(Transport::enqueue(&mut c, b"x", 0));
    }
    t.connections.insert(2, c);
    for id in 1..=2 {
        t.pending.insert(
            RequestKey {
                session: 1,
                connection: 2,
                id,
            },
            Pending {
                started: false,
                received: 0,
                operation: "run".into(),
            },
        );
    }
    t.now = 5000;
    t.expire_queued();
    assert_eq!(t.connection_count(), 0);
    assert_eq!(t.request_count(), 0);
    drop(t);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn wait_budget_is_clamped_to_deadlines_even_if_caller_requests_long_wait() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("rad-transport-wait-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut t = Transport::bind(&dir, 1).unwrap();
    let mut c = connection();
    c.hello = false;
    t.connections.insert(2, c);
    assert_eq!(
        t.wait_timeout(Duration::from_secs(60)),
        Duration::from_secs(2)
    );
    t.now = 1999;
    assert_eq!(
        t.wait_timeout(Duration::from_secs(60)),
        Duration::from_millis(1)
    );
    t.now = 2000;
    assert_eq!(t.wait_timeout(Duration::from_secs(60)), Duration::ZERO);
    drop(t);
    std::fs::remove_dir_all(dir).unwrap();
}
