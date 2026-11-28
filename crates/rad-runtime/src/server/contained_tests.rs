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

pub(super) fn started_request(
    root: &Directory,
    server: &mut Server,
) -> (UnixStream, rad_transport::RequestKey) {
    let mut socket = UnixStream::connect(root.0.join("rad/editor.sock")).unwrap();
    socket.set_nonblocking(true).unwrap();
    write_request(&mut socket, 1, "hello", json!({}));
    server.transport.drive(1).unwrap();
    let incoming = server.transport.take_next().unwrap();
    (socket, incoming.key)
}

fn write_request(socket: &mut UnixStream, id: u64, operation: &str, args: Value) {
    let bytes = serde_json::to_vec(&json!({
        "v":1, "id":id.to_string(), "op":operation, "args":args
    }))
    .unwrap();
    socket
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&bytes).unwrap();
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

fn options_manifest() -> rad_plugin_contract::Manifest {
    serde_json::from_slice(include_bytes!(
        "../../../rad-plugin-host/test-plugins/options-manifest.json"
    ))
    .unwrap()
}

fn pending_options(root: &Directory, server: &mut Server, plugin: &str) -> UnixStream {
    let mut socket = UnixStream::connect(root.0.join("rad/editor.sock")).unwrap();
    socket.set_nonblocking(true).unwrap();
    write_request(&mut socket, 1, "hello", json!({}));
    server.turn().unwrap();
    let hello = read_frame(server, &mut socket, monotonic_ms().unwrap());
    assert_eq!(hello["status"], "ok", "{hello}");
    write_request(
        &mut socket,
        2,
        "options",
        json!({"type":format!("{plugin}:pick"),"field":"group","cursor":null,"context":{"scenario":"normal"}}),
    );
    server.turn().unwrap();
    assert_eq!(
        server.pending_requests(),
        1,
        "options should be waiting for the host"
    );
    socket
}

fn assert_options_rejected(server: &mut Server, socket: &mut UnixStream) {
    server.turn().unwrap();
    assert_eq!(server.pending_requests(), 0, "stale options still pending");
    let response = read_frame(server, socket, monotonic_ms().unwrap());
    assert_eq!(response["id"], "2");
    assert_eq!(
        response["error"]["code"], "executor_unavailable",
        "{response}"
    );
}

#[test]
fn pending_options_reject_a_new_selection_of_the_same_package() {
    let (root, mut server) = runtime("option-selection", 940);
    let manifest = options_manifest();
    server
        .actor
        .register_plugin(manifest.clone(), rad_plugin_host::TARGET)
        .unwrap();
    let mut socket = pending_options(&root, &mut server, &manifest.id);
    server
        .actor
        .register_plugin(manifest, rad_plugin_host::TARGET)
        .unwrap();
    assert_options_rejected(&mut server, &mut socket);
}

#[test]
fn pending_options_reject_a_restarted_process_of_the_same_selection() {
    let (root, mut server) = runtime("option-process", 941);
    let manifest = options_manifest();
    let plugin = manifest.id.clone();
    server
        .actor
        .install_plugin(manifest, 17, rad_plugin_host::TARGET)
        .unwrap();
    let (_, selection, _) = server.actor.plugin_manifests().next().unwrap();
    let mut socket = pending_options(&root, &mut server, &plugin);
    server.actor.deactivate_plugin(&plugin, 17).unwrap();
    server
        .actor
        .activate_plugin(&plugin, selection, 18)
        .unwrap();
    assert_options_rejected(&mut server, &mut socket);
}
