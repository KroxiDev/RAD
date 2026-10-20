use rad_client::connection::{Client, Endpoint};
use rad_runtime::server::Server;
use serde_json::json;
use std::{
    num::NonZeroU128,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    time::{Duration, Instant},
};
struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn credential_ipc_persists_separately_and_never_returns_a_secret() {
    let root =
        Directory(std::env::temp_dir().join(format!("rad-credential-ipc-{}", std::process::id())));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root.0)
        .unwrap();
    let mut server = Server::new(
        &root.0,
        &root.0.join("data"),
        NonZeroU128::new(0xC401).unwrap(),
    )
    .unwrap();
    let path = root.0.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let client = std::thread::spawn(move || {
        let mut editor = Client::connect(&path, Endpoint::Editor).unwrap();
        let status = editor
            .request(
                "credential_status",
                json!({"plugin_id":"fixture","slot":"password"}),
            )
            .unwrap();
        assert!(status.error.is_none(), "status: {:?}", status.error);
        assert_eq!(status.data, json!({"state":"absent"}));
        let before = editor.request("status", json!({})).unwrap().data;
        let saved = editor
            .request(
                "credential_replace",
                json!({"plugin_id":"fixture","slot":"password","secret":"private-runtime-canary"}),
            )
            .unwrap();
        assert!(saved.error.is_none(), "save: {:?}", saved.error);
        assert_eq!(saved.data, json!({"state":"present"}));
        let queried = editor
            .request(
                "credential_status",
                json!({"plugin_id":"fixture","slot":"password"}),
            )
            .unwrap();
        assert_eq!(queried.data, json!({"state":"present"}));
        let response = editor.request("status", json!({})).unwrap();
        assert!(!format!("{response:?}").contains("private-runtime-canary"));
        let after = editor.request("status", json!({})).unwrap().data;
        assert!(before["view_revision"].is_string());
        assert_eq!(before["view_revision"], after["view_revision"]);
        assert_eq!(before["history_operations"], after["history_operations"]);
        let mut cli = Client::connect(&path, Endpoint::Automation).unwrap();
        let denied = cli
            .request(
                "credential_remove",
                json!({"plugin_id":"fixture","slot":"password"}),
            )
            .unwrap();
        assert_eq!(denied.error.unwrap().code, "permission_denied");
        let removed = editor
            .request(
                "credential_remove",
                json!({"plugin_id":"fixture","slot":"password"}),
            )
            .unwrap();
        assert_eq!(removed.data, json!({"state":"absent"}));
        tx.send(()).unwrap();
    });
    let end = Instant::now() + Duration::from_secs(10);
    while rx.try_recv().is_err() {
        server.turn().unwrap();
        if client.is_finished() {
            break;
        }
        assert!(Instant::now() < end);
        server.wait(Duration::from_millis(10)).unwrap();
    }
    client.join().unwrap();
    server.request_shutdown();
    while !server.shutdown_complete() {
        server.turn().unwrap();
        server.wait(Duration::from_millis(10)).unwrap();
        assert!(Instant::now() < end);
    }
    assert!(root.0.join("data/credentials/master.key").exists());
}

#[test]
fn known_prewrite_vault_failure_is_not_applied_even_while_vault_is_unknown() {
    let root = Directory(
        std::env::temp_dir().join(format!("rad-credential-prewrite-{}", std::process::id())),
    );
    for sub in ["", "data"] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(root.0.join(sub))
            .unwrap();
    }
    std::fs::DirBuilder::new()
        .mode(0o755)
        .create(root.0.join("data/credentials"))
        .unwrap();
    let mut server = Server::new(
        &root.0,
        &root.0.join("data"),
        NonZeroU128::new(0xc451).unwrap(),
    )
    .unwrap();
    let path = root.0.clone();
    let client = std::thread::spawn(move || {
        let mut editor = Client::connect(&path, Endpoint::Editor).unwrap();
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            let response = editor
                .request(
                    "credential_replace",
                    json!({"plugin_id":"fixture","slot":"password","secret":"prewrite-fixture"}),
                )
                .unwrap();
            let error = response.error.unwrap();
            if error.code == "busy" {
                assert!(Instant::now() < end);
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            return error;
        }
    });
    let end = Instant::now() + Duration::from_secs(5);
    while !client.is_finished() {
        server.turn().unwrap();
        server.wait(Duration::from_millis(2)).unwrap();
        assert!(Instant::now() < end);
    }
    let error = client.join().unwrap();
    server.request_shutdown();
    while !server.shutdown_complete() {
        server.turn().unwrap();
        server.wait(Duration::from_millis(2)).unwrap();
        assert!(Instant::now() < end);
    }
    assert_eq!(
        std::fs::read_dir(root.0.join("data/credentials"))
            .unwrap()
            .count(),
        0,
        "unsafe vault must remain untouched"
    );
    assert_eq!(error.code, "storage_error");
    assert_eq!(
        error.outcome,
        Some(rad_wire::response::Outcome::NotApplied),
        "known pre-write rejection is not an uncertain mutation"
    );
}
