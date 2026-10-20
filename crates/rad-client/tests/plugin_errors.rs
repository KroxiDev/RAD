// Critical closed error vocabulary shared by the runtime, wire decoder and clients.
use rad_client::{
    connection::{Client, Endpoint},
    response::{Error, Outcome, Response, Status},
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::{fs::DirBuilderExt, net::UnixListener},
    time::Duration,
};
const SESSION: &str = "00000000000000000000000000000367";
const CODES: &[&str] = &[
    "plugin_in_use",
    "plugin_residual",
    "publication_pending",
    "incompatible_plugin",
    "invalid_source",
    "invalid_package",
    "network",
    "deadline",
    "io",
    "confirmation_required",
    "recovery_required",
];
fn rejected(id: u64, code: &str) -> Response {
    Response {
        v: 1,
        id: Some(id.to_string()),
        session_id: Some(SESSION.into()),
        status: Status::Error,
        data: Value::Null,
        error: Some(Error {
            code: code.into(),
            message: "Request rejected".into(),
            details: json!({}),
            outcome: Some(Outcome::NotApplied),
        }),
    }
}
#[test]
fn plugin_error_roundtrip_is_closed_and_correlated() {
    for code in CODES {
        let original = rejected(167, code);
        let bytes = serde_json::to_vec(&original).unwrap();
        assert_eq!(
            Response::decode(&bytes, 167, Some(SESSION)).unwrap(),
            original
        );
        assert!(Response::decode(&bytes, 168, Some(SESSION)).is_err());
        assert!(Response::decode(&bytes, 167, Some("00000000000000000000000000000368")).is_err());
    }
    for code in ["plugin_arbitrary", "plugin_in_use_extra", "NETWORK", ""] {
        assert!(
            Response::decode(
                &serde_json::to_vec(&rejected(167, code)).unwrap(),
                167,
                Some(SESSION)
            )
            .is_err()
        );
    }
}
#[test]
fn plugin_admin_rejections_keep_not_applied_and_connection_usable() {
    let root = std::env::temp_dir().join(format!("rad-client-w1-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(root.join("rad"))
        .unwrap();
    let listener = UnixListener::bind(root.join("rad/editor.sock")).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        // Returning on disconnect lets the RED client failure be the primary assertion.
        for n in 1..=(CODES.len() + 2) {
            let mut header = [0; 4];
            if stream.read_exact(&mut header).is_err() {
                return;
            }
            let length = u32::from_be_bytes(header) as usize;
            assert!(length <= 65536);
            let mut bytes = vec![0; length];
            stream.read_exact(&mut bytes).unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(request["id"], n.to_string());
            let response = if n == 1 {
                assert_eq!(request["op"], "hello");
                Response {
                    v: 1,
                    id: Some(n.to_string()),
                    session_id: Some(SESSION.into()),
                    status: Status::Ok,
                    data: json!({"protocol":1,"session_id":SESSION,"limits":{},"capabilities":["plugin_admin","status"]}),
                    error: None,
                }
            } else if n <= CODES.len() + 1 {
                assert_eq!(request["op"], "plugin_admin");
                rejected(n as u64, CODES[n - 2])
            } else {
                assert_eq!(request["op"], "status");
                Response {
                    v: 1,
                    id: Some(n.to_string()),
                    session_id: Some(SESSION.into()),
                    status: Status::Ok,
                    data: json!({"storage_state":"ready"}),
                    error: None,
                }
            };
            let bytes = serde_json::to_vec(&response).unwrap();
            stream
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&bytes).unwrap();
        }
    });
    let mut client = Client::connect(&root, Endpoint::Editor).unwrap();
    let result = (|| {
        for (i, code) in CODES.iter().enumerate() {
            let response=client.request("plugin_admin",json!({"operation":"update","plugin_id":"org.rad.fixture","source":"https://github.com/KroxiDev/rad-plugin-fixture/releases/tag/v2.0.0","trusted":true}))?;
            assert_eq!(response, rejected((i + 2) as u64, code));
        }
        let response = client.request("status", json!({}))?;
        assert_eq!(response.data["storage_state"], "ready");
        Ok::<_, rad_client::connection::Failure>(())
    })();
    drop(client);
    server.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
    result.expect("a correlated not_applied plugin rejection must not poison the client");
}
