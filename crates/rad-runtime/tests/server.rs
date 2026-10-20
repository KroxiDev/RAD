use rad_client::connection::{Client, Endpoint};
use rad_runtime::server::Server;
use std::{
    num::NonZeroU128,
    os::unix::fs::PermissionsExt,
    sync::mpsc,
    time::{Duration, Instant},
};
struct Directory(std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn editor_can_close_while_runtime_still_serves_the_automation_endpoint() {
    let directory = Directory(
        std::env::temp_dir().join(format!("rad-server-integration-{}", std::process::id())),
    );
    std::fs::create_dir(&directory.0).unwrap();
    std::fs::set_permissions(&directory.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut server = Server::new(
        &directory.0,
        &directory.0.join("data"),
        NonZeroU128::new(378).unwrap(),
    )
    .unwrap();
    for endpoint in [Endpoint::Editor, Endpoint::Automation] {
        let (send, receive) = mpsc::sync_channel(1);
        let root = directory.0.clone();
        let client = std::thread::spawn(move || {
            let result = Client::connect(&root, endpoint).map(|mut client| {
                let status = client.request("status", serde_json::json!({})).unwrap();
                if endpoint == Endpoint::Editor {
                    assert_eq!(
                        serde_json::to_value(&status.data).unwrap()["deck_state"],
                        "disabled"
                    );
                    assert_eq!(
                        serde_json::to_value(&status.data).unwrap()["recovered_previous"],
                        false
                    );
                } else {
                    assert!(
                        serde_json::to_value(&status.data)
                            .unwrap()
                            .get("recovered_previous")
                            .is_none()
                    );
                }
            });
            send.send(result).unwrap();
        });
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            server.turn().unwrap();
            if let Ok(result) = receive.try_recv() {
                result.expect("same runtime must negotiate both endpoints after GUI closes");
                break;
            }
            assert!(
                Instant::now() < end,
                "runtime must answer bounded client handshake"
            );
            server.wait(Duration::from_millis(20)).unwrap();
        }
        client.join().unwrap();
    }
    assert!(directory.0.join("data/state.sqlite3").exists());
}

#[test]
fn editor_and_automation_share_durable_configuration_and_history_across_restart() {
    use serde_json::json;
    let directory =
        Directory(std::env::temp_dir().join(format!("rad-server-edit-{}", std::process::id())));
    std::fs::create_dir(&directory.0).unwrap();
    std::fs::set_permissions(&directory.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut server = Server::new(
        &directory.0,
        &directory.0.join("data"),
        NonZeroU128::new(379).unwrap(),
    )
    .unwrap();
    let root = directory.0.clone();
    let (send, receive) = mpsc::sync_channel(1);
    let client = std::thread::spawn(move || {
        let mut editor = Client::connect(&root, Endpoint::Editor).unwrap();
        let principal = serde_json::to_value(editor.request("status", json!({})).unwrap().data)
            .unwrap()["active_folder_id"]
            .clone();
        let created = editor
            .request(
                "create_button",
                json!({"folder_id":principal,"position":0,"name":"Shared"}),
            )
            .unwrap();
        assert_eq!(created.status, rad_client::response::Status::Applied);
        let mut cli = Client::connect(&root, Endpoint::Automation).unwrap();
        assert_eq!(
            serde_json::to_value(cli.request("get", json!({"id":"b1"})).unwrap().data).unwrap()["fields"]
                ["name"],
            "Shared"
        );
        assert_eq!(
            cli.request(
                "edit",
                json!({"changes":[{"kind":"set","id":"b1","field":"name","value":"CLI"}]})
            )
            .unwrap()
            .status,
            rad_client::response::Status::Applied
        );
        assert_eq!(
            serde_json::to_value(editor.request("get", json!({"id":"b1"})).unwrap().data).unwrap()
                ["fields"]["name"],
            "CLI"
        );
        for (op, expected) in [("undo", "Shared"), ("redo", "CLI")] {
            assert_eq!(
                editor.request(op, json!({})).unwrap().status,
                rad_client::response::Status::Applied
            );
            assert_eq!(
                serde_json::to_value(cli.request("get", json!({"id":"b1"})).unwrap().data).unwrap()
                    ["fields"]["name"],
                expected
            );
        }
        send.send(()).unwrap();
    });
    let end = Instant::now() + Duration::from_secs(5);
    while receive.try_recv().is_err() {
        server
            .turn()
            .expect("commands must flow through the one Authority and real Store worker");
        assert!(
            Instant::now() < end,
            "shared edit/history flow must complete"
        );
        server.wait(Duration::from_millis(10)).unwrap();
    }
    client.join().unwrap();
    drop(server);
    let mut restarted = Server::new(
        &directory.0,
        &directory.0.join("data"),
        NonZeroU128::new(380).unwrap(),
    )
    .unwrap();
    let root = directory.0.clone();
    let (send, receive) = mpsc::sync_channel(1);
    let client = std::thread::spawn(move || {
        let mut cli = Client::connect(&root, Endpoint::Automation).unwrap();
        let v = cli.request("get", json!({"id":"b1"})).unwrap();
        assert_eq!(
            serde_json::to_value(&v.data).unwrap()["fields"]["name"],
            "CLI"
        );
        assert_eq!(
            v.session_id.as_deref(),
            Some("0000000000000000000000000000017c")
        );
        send.send(()).unwrap();
    });
    let end = Instant::now() + Duration::from_secs(3);
    while receive.try_recv().is_err() {
        restarted.turn().unwrap();
        assert!(Instant::now() < end);
        restarted.wait(Duration::from_millis(10)).unwrap();
    }
    client.join().unwrap();
}

#[test]
fn subscribed_editor_receives_coalesced_invalidation_then_reads_atomic_views() {
    use rad_api::{BoardView, InspectorView, NoticeKind, Request};
    use serde_json::json;
    let directory =
        Directory(std::env::temp_dir().join(format!("rad-server-notices-{}", std::process::id())));
    std::fs::create_dir(&directory.0).unwrap();
    std::fs::set_permissions(&directory.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut server = Server::new(
        &directory.0,
        &directory.0.join("data"),
        NonZeroU128::new(391).unwrap(),
    )
    .unwrap();
    let root = directory.0.clone();
    let (send, receive) = mpsc::sync_channel(1);
    let client = std::thread::spawn(move || {
        let mut subscription = Client::connect(&root, Endpoint::Editor)
            .unwrap()
            .subscribe()
            .unwrap();
        let cancel = subscription.cancellation().unwrap();
        // Initial notices close the subscribe/snapshot race without polling.
        let mut initial = std::collections::BTreeSet::new();
        while initial.len() < NoticeKind::ALL.len() {
            initial.insert(subscription.recv().unwrap().kind);
        }
        let mut editor = Client::connect(&root, Endpoint::Editor).unwrap();
        let rad_api::ResponseData::Overview(overview) =
            editor.call(Request::Overview {}).unwrap().data
        else {
            panic!("overview");
        };
        assert_eq!(overview.deck, rad_api::DeckState::Disabled);
        let id = overview.active_folder_id.unwrap();
        let created = editor
            .call(Request::CreateButton {
                folder_id: id,
                position: 0,
                name: "Atomic".into(),
            })
            .unwrap();
        assert!(created.error.is_none());
        loop {
            if subscription.recv().unwrap().kind == NoticeKind::Board {
                break;
            }
        }
        let board: BoardView = serde_json::from_value(
            serde_json::to_value(
                editor
                    .call(Request::Board { folder_id: None })
                    .unwrap()
                    .data,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(board.keys.len(), board.grid.columns * board.grid.rows);
        let key = board.keys[0].as_ref().unwrap();
        assert_eq!(key.entity.view_revision, board.view_revision);
        let inspector: InspectorView = serde_json::from_value(
            serde_json::to_value(
                editor
                    .call(Request::Inspector {
                        id: key.entity.id.clone(),
                    })
                    .unwrap()
                    .data,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(inspector.view_revision, board.view_revision);
        assert_eq!(
            serde_json::to_value(&inspector.entity).unwrap()["fields"]["name"],
            json!("Atomic")
        );
        cancel.cancel();
        assert!(subscription.recv().is_err());
        send.send(()).unwrap();
    });
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        server.turn().unwrap();
        if receive.try_recv().is_ok() {
            break;
        }
        assert!(
            Instant::now() < end,
            "subscription must wake on publication without poll"
        );
        server.wait(Duration::from_millis(10)).unwrap();
    }
    client.join().unwrap();
}
