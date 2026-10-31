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
