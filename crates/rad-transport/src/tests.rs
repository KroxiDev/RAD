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
        subscribed: false,
        notices: BTreeMap::new(),
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

#[test]
fn notification_output_keeps_latest_unsent_payload_and_never_replaces_partial_frame() {
    let mut c = connection();
    assert!(Transport::queue_notice(&mut c, 0, b"first"));
    assert!(Transport::queue_notice(&mut c, 0, b"latest"));
    assert_eq!(c.notices.len(), 1);
    assert_eq!(c.notices[&0], b"latest");
    assert!(Transport::enqueue(&mut c, b"already framed", 0));
    c.output.front_mut().unwrap().offset = 3;
    let bytes = c.output.front().unwrap().bytes.clone();
    assert!(Transport::queue_notice(&mut c, 0, b"newest"));
    assert_eq!(c.output.front().unwrap().offset, 3);
    assert_eq!(c.output.front().unwrap().bytes, bytes);
    assert_eq!(c.notices[&0], b"newest");
    assert!(!Transport::queue_notice(
        &mut c,
        8,
        b"outside finite key space"
    ));
    assert!(!Transport::queue_notice(&mut c, 1, &vec![0; 4097]));
}

#[test]
fn an_accept_failure_is_the_connections_fault_unless_the_listener_is_broken() {
    use io::ErrorKind::{ConnectionAborted, ConnectionReset};
    for peer in [
        io::Error::from(ConnectionAborted),
        io::Error::from(ConnectionReset),
        io::Error::from_raw_os_error(libc::EPROTO),
    ] {
        assert_eq!(accept_fault(&peer), AcceptFault::Peer, "{peer:?}");
    }
    for exhausted in [libc::EMFILE, libc::ENFILE, libc::ENOBUFS, libc::ENOMEM] {
        let error = io::Error::from_raw_os_error(exhausted);
        assert_eq!(accept_fault(&error), AcceptFault::Exhausted, "{error:?}");
    }
    for broken in [libc::EBADF, libc::EINVAL, libc::ENOTSOCK] {
        let error = io::Error::from_raw_os_error(broken);
        assert_eq!(accept_fault(&error), AcceptFault::Listener, "{error:?}");
    }
}

#[test]
fn a_connection_that_cannot_be_registered_is_closed_and_the_next_one_is_served() {
    use std::{
        io::Read,
        os::unix::{fs::PermissionsExt, net::UnixStream as StdStream},
    };
    let dir = std::env::temp_dir().join(format!("rad-transport-register-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut t = Transport::bind(&dir, 1).unwrap();
    let mut refused = StdStream::connect(dir.join("rad/editor.sock")).unwrap();
    refused
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    t.listeners_ready = [true, false];
    t.accept_with(Origin::Editor, &mut |_, _, _| {
        Err(io::Error::from_raw_os_error(libc::ENOMEM))
    })
    .expect("one connection cannot be watched; the transport goes on");
    assert_eq!(t.connection_count(), 0);
    assert_eq!(t.take_connection_failures(), 1);
    assert_eq!(t.take_connection_failures(), 0, "counted once");
    // The refused peer sees its connection closed, not a hang.
    assert_eq!(refused.read(&mut [0; 1]).unwrap(), 0);
    // The next one is accepted as usual.
    let _next = StdStream::connect(dir.join("rad/editor.sock")).unwrap();
    t.listeners_ready = [true, false];
    t.accept_connections(Origin::Editor).unwrap();
    assert_eq!(t.connection_count(), 1);
    assert_eq!(t.take_connection_failures(), 0);
    drop(t);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn an_exhausted_accept_waits_for_its_retry_instead_of_spinning() {
    use std::os::unix::fs::DirBuilderExt;
    let dir = std::env::temp_dir().join(format!("rad-transport-exhausted-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&dir)
        .unwrap();
    let mut t = Transport::bind(&dir, 1).unwrap();
    t.drive(1000).unwrap();
    t.accept_fault(0, AcceptFault::Exhausted).unwrap();
    // Not ready "now": a loop that sleeps until the deadline does not spin.
    assert_eq!(t.next_deadline(), Some(1000 + ACCEPT_RETRY_MS));
    t.drive(1000 + ACCEPT_RETRY_MS - 1).unwrap();
    assert_eq!(t.next_deadline(), Some(1000 + ACCEPT_RETRY_MS));
    // The retry runs; the queue is empty, so nothing is left pending.
    t.drive(1000 + ACCEPT_RETRY_MS).unwrap();
    assert_eq!(t.next_deadline(), None);
    assert_eq!(t.take_connection_failures(), 1);
    drop(t);
    std::fs::remove_dir_all(dir).unwrap();
}
