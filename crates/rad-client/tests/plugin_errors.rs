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
    "plugin_already_installed",
    "plugin_mismatch",
    "plugin_same_version",
    "plugin_failed",
    "source_changed",
];
fn rejected(id: u64, code: &str) -> Response {
    Response {
        v: 1,
        id: Some(id.to_string()),
        session_id: Some(SESSION.into()),
        status: Status::Error,
        data: rad_api::ResponseData::None,
        error: Some(Error {
            code: rad_api::ErrorCode::parse(code).unwrap(),
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
        assert_eq!(decode(&bytes, 167, Some(SESSION)).unwrap(), original);
        assert!(decode(&bytes, 168, Some(SESSION)).is_err());
        assert!(decode(&bytes, 167, Some("00000000000000000000000000000368")).is_err());
    }
    for code in ["plugin_arbitrary", "plugin_in_use_extra", "NETWORK", ""] {
        let mut invalid = serde_json::to_value(rejected(167, "plugin_in_use")).unwrap();
        invalid["error"]["code"] = json!(code);
        assert!(decode(&serde_json::to_vec(&invalid).unwrap(), 167, Some(SESSION)).is_err());
    }
}
#[test]
fn source_changed_carries_both_origins_to_the_client() {
    let details = json!({
        "registered": "https://github.com/owner/repo",
        "requested": "https://github.com/other/repo/releases/tag/v2",
    });
    let mut original = rejected(167, "source_changed");
    original.error.as_mut().unwrap().details = details.clone();
    let bytes = serde_json::to_vec(&original).unwrap();
    let decoded = decode(&bytes, 167, Some(SESSION)).unwrap();
    assert_eq!(decoded, original);
    assert_eq!(decoded.error.unwrap().details, details);
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
                    data: rad_api::ResponseData::Hello(serde_json::from_value(json!({"protocol":1,"session_id":SESSION,"limits":serde_json::from_str::<serde_json::Value>(include_str!("../../rad-api/vectors/limits-v1.json")).unwrap(),"capabilities":["plugin_admin","status"]})).unwrap()),
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
                    data: rad_api::ResponseData::Status(serde_json::from_value(json!({"storage_state":"ready","view_revision":"1","active_folder_id":"f1","grid":{"columns":8,"rows":4}})).unwrap()),
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
        assert_eq!(
            serde_json::to_value(&response.data).unwrap()["storage_state"],
            "ready"
        );
        Ok::<_, rad_client::connection::Failure>(())
    })();
    drop(client);
    server.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
    result.expect("a correlated not_applied plugin rejection must not poison the client");
}

fn decode(
    bytes: &[u8],
    id: u64,
    session: Option<&str>,
) -> Result<Response, rad_api::response::InvalidResponse> {
    Response::decode_value(
        rad_wire::parse_json(bytes).map_err(|_| rad_api::response::InvalidResponse)?,
        id,
        session,
        rad_api::Operation::PluginAdmin,
    )
}
