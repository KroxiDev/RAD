//! Failures bound to one request, plugin or device are contained where they happen: the
//! turn keeps going and what clients can see stays coherent (ADR 0011).

use super::*;
use std::{
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
};

pub(super) struct Directory(pub(super) std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn runtime(name: &str, session: u128) -> (Directory, Server) {
    let root = Directory(
        std::env::temp_dir().join(format!("rad-contained-{}-{name}", std::process::id())),
    );
    std::fs::create_dir(&root.0).unwrap();
    std::fs::set_permissions(&root.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let server = Server::new(
        &root.0,
        &root.0.join("data"),
        NonZeroU128::new(session).unwrap(),
    )
    .unwrap();
    (root, server)
}

/// A client with its `hello` taken by the server: a started request, ready to be answered.
pub(super) fn started_request(
    root: &Directory,
    server: &mut Server,
) -> (UnixStream, rad_transport::RequestKey) {
    let mut socket = UnixStream::connect(root.0.join("rad/editor.sock")).unwrap();
    socket.set_nonblocking(true).unwrap();
    let hello = br#"{"v":1,"id":"1","op":"hello","args":{}}"#;
    socket
        .write_all(&(hello.len() as u32).to_be_bytes())
        .unwrap();
    socket.write_all(hello).unwrap();
    server.transport.drive(1).unwrap();
    let incoming = server.transport.take_next().unwrap();
    (socket, incoming.key)
}

pub(super) fn read_frame(server: &mut Server, socket: &mut UnixStream, now: u64) -> Value {
    server.transport.drive(now).unwrap();
    let mut bytes = [0; 8192];
    let read = socket.read(&mut bytes).unwrap();
    let length = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
    assert!(read >= 4 + length, "a whole frame was written");
    serde_json::from_slice(&bytes[4..4 + length]).unwrap()
}

#[test]
fn an_oversized_answer_to_a_change_does_not_claim_it_was_not_applied() {
    let (root, mut server) = runtime("oversized-change", 904);
    let (mut socket, key) = started_request(&root, &mut server);
    let oversized = Response {
        v: 1,
        id: Some(key.id().to_string()),
        session_id: Some(format!("{:032x}", server.session)),
        status: Status::Applied,
        data: rad_api::ResponseData::PluginLog(rad_api::PluginLogView {
            plugin_id: "org.rad.test".into(),
            lines: vec!["x".repeat(1000); 100],
        }),
        error: None,
    };
    server.respond(key, oversized).unwrap();
    let frame = read_frame(&mut server, &mut socket, 2);
    assert_eq!(frame["error"]["code"], "resource_exhausted");
    assert_eq!(frame["error"]["outcome"], "unknown");
}
