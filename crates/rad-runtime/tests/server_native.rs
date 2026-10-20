use rad_client::connection::{Client, Endpoint};
use rad_launcher::Launcher;
use rad_runtime::server::Server;
use serde_json::json;
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
fn launcher_catalog_configuration_and_run_survive_editor_close() {
    let directory =
        Directory(std::env::temp_dir().join(format!("rad-server-native-{}", std::process::id())));
    for sub in [
        "",
        "home",
        "data/applications",
        "system",
        "config",
        "cache",
        "runtime",
    ] {
        std::fs::create_dir_all(directory.0.join(sub)).unwrap();
    }
    std::fs::set_permissions(
        directory.0.join("runtime"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let helper =
        std::env::var("RAD_TEST_LAUNCHER_HELPER").expect("runner builds private helper fixture");
    let fixture =
        std::env::var("RAD_TEST_LAUNCHER_FIXTURE").expect("runner builds benign Rust fixture app");
    let output = directory.0.join("invoked.json");
    std::fs::write(directory.0.join("data/applications/test.desktop"), format!("[Desktop Entry]\nType=Application\nName=Private fixture\nExec=\"{fixture}\" literal\nTerminal=false\n")).unwrap();
    let mut command = std::process::Command::new(helper);
    command
        .env_clear()
        .env("HOME", directory.0.join("home"))
        .env("XDG_DATA_HOME", directory.0.join("data"))
        .env("XDG_DATA_DIRS", directory.0.join("system"))
        .env("XDG_CONFIG_HOME", directory.0.join("config"))
        .env("XDG_CACHE_HOME", directory.0.join("cache"))
        .env("XDG_RUNTIME_DIR", directory.0.join("runtime"))
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent-rad-private-bus",
        )
        .env("RAD_FIXTURE_OUTPUT", &output)
        .env("PATH", "/nonexistent-rad-private-bin");
    let mut server = Server::new(
        &directory.0.join("runtime"),
        &directory.0.join("store"),
        NonZeroU128::new(122).unwrap(),
    )
    .unwrap();
    server
        .attach_launcher(Launcher::spawn(command).unwrap())
        .unwrap();
    let root = directory.0.join("runtime");
    let (send, receive) = mpsc::sync_channel(1);
    let client = std::thread::spawn(move || {
        let mut editor = Client::connect(&root, Endpoint::Editor).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let response = editor
                .request("launcher_catalog", json!({"cursor":null,"query":"Private"}))
                .unwrap();
            if response.error.is_none() {
                assert_eq!(
                    serde_json::to_value(&response.data).unwrap()["items"][0]["desktop_entry"],
                    "test.desktop"
                );
                assert_eq!(
                    serde_json::to_value(&response.data).unwrap()["items"][0]["name"],
                    "Private fixture"
                );
                break;
            }
            assert!(Instant::now() < deadline, "catalog must become ready");
            std::thread::sleep(Duration::from_millis(5));
        }
        let folder = serde_json::to_value(editor.request("status", json!({})).unwrap().data)
            .unwrap()["active_folder_id"]
            .clone();
        assert!(
            editor
                .request(
                    "create_button",
                    json!({"folder_id":folder,"position":0,"name":"Launch"})
                )
                .unwrap()
                .error
                .is_none()
        );
        assert!(editor.request("add_action",json!({"button_id":"b1","type":"launcher","parameters":{"desktop_entry":"test.desktop"}})).unwrap().error.is_none());
        let response = editor.request("run", json!({"id":"b1"})).unwrap();
        assert_eq!(response.status, rad_client::response::Status::Admitted);
        let id = serde_json::to_value(&response.data).unwrap()["execution_id"].clone();
        drop(editor);
        let mut cli = Client::connect(&root, Endpoint::Automation).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let result = cli.request("result", json!({"execution_id":id})).unwrap();
            assert!(result.error.is_none(), "{result:?}");
            match serde_json::to_value(&result.data).unwrap()["state"].as_str() {
                Some("running") => {}
                Some("succeeded") => break,
                other => panic!("native execution failed: {other:?}"),
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        send.send(()).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while receive.try_recv().is_err() {
        server.turn().unwrap();
        assert!(Instant::now() < deadline);
        server.wait(Duration::from_millis(20)).unwrap();
    }
    client.join().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while std::fs::read_to_string(output.clone()).ok().as_deref() != Some("[\"literal\"]") {
        assert!(
            Instant::now() < deadline,
            "accepted is not yet fixture output readiness"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
