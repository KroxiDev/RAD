use rad_client::connection::{Client, Endpoint};
use rad_runtime::server::Server;
use serde_json::{Value, json};
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
fn ok(c: &mut Client, op: &str, args: Value) -> Value {
    let r = c.request(op, args).unwrap();
    assert!(r.error.is_none(), "{op}: {r:?}");
    serde_json::to_value(r.data).unwrap()
}
#[test]
fn editor_image_export_and_confirmed_restore_cross_the_real_store_lane() {
    let root = Directory(
        std::env::temp_dir().join(format!("rad-server-transfers-{}", std::process::id())),
    );
    std::fs::create_dir(&root.0).unwrap();
    std::fs::set_permissions(&root.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut server = Server::new(
        &root.0,
        &root.0.join("data"),
        NonZeroU128::new(901).unwrap(),
    )
    .unwrap();
    let path = root.0.clone();
    let (send, receive) = mpsc::sync_channel(1);
    let client = std::thread::spawn(move || {
        let mut c = Client::connect(&path, Endpoint::Editor).unwrap();
        ok(
            &mut c,
            "create_button",
            json!({"folder_id":"f1","position":0,"name":"Original"}),
        );
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
        let token = ok(
            &mut c,
            "transfer_begin",
            json!({"kind":"image_import","size":"70"}),
        )["transfer_id"]
            .clone();
        ok(
            &mut c,
            "transfer_write",
            json!({"transfer_id":token,"offset":"0","base64":png}),
        );
        let key = ok(&mut c, "transfer_finish", json!({"transfer_id":token}))["media_ref"].clone();
        // A second synthetic pixel proves the whole retained group is retryable.
        let green = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNg+M/wHwAEAQH/cetH5QAAAABJRU5ErkJggg==";
        let green_token = ok(
            &mut c,
            "transfer_begin",
            json!({"kind":"image_import","size":"70"}),
        )["transfer_id"]
            .clone();
        ok(
            &mut c,
            "transfer_write",
            json!({"transfer_id":green_token,"offset":"0","base64":green}),
        );
        let green_key = ok(
            &mut c,
            "transfer_finish",
            json!({"transfer_id":green_token}),
        )["media_ref"]
            .clone();
        let stale = ok(&mut c, "inspector", json!({"id":"b1"}))["view_revision"].clone();
        ok(
            &mut c,
            "edit",
            json!({"changes":[{"kind":"set","id":"b1","field":"name","value":"Concurrent"}]}),
        );
        let rejected = c
            .request(
                "edit",
                json!({"expected_revision":stale,"changes":[
                    {"kind":"set","id":"b1","field":"appearance","value":{"text":"Red","image":key,"states":{"off":{"text":"Green","image":green_key}}}}
                ]}),
            )
            .unwrap();
        assert_eq!(rejected.error.unwrap().code, "cursor_expired");
        let fresh = ok(&mut c, "inspector", json!({"id":"b1"}))["view_revision"].clone();
        ok(
            &mut c,
            "edit",
            json!({"expected_revision":fresh,"changes":[
                {"kind":"set","id":"b1","field":"appearance","value":{"text":"Red","image":key,"states":{"off":{"text":"Green","image":green_key}}}},
                {"kind":"set","id":"b1","field":"name","value":"Original"}
            ]}),
        );
        ok(
            &mut c,
            "edit",
            json!({"changes":[{"kind":"set","id":"b1","field":"appearance","value":{"text":"Red","image":key,"states":{}}}]}),
        );
        // Re-importing the same normalized image is a legitimate no-op, not a leaked lease.
        let second = ok(
            &mut c,
            "transfer_begin",
            json!({"kind":"image_import","size":"70"}),
        )["transfer_id"]
            .clone();
        ok(
            &mut c,
            "transfer_write",
            json!({"transfer_id":second,"offset":"0","base64":png}),
        );
        let same =
            ok(&mut c, "transfer_finish", json!({"transfer_id":second}))["media_ref"].clone();
        assert_eq!(same, key);
        assert_eq!(
            ok(
                &mut c,
                "edit",
                json!({"changes":[{"kind":"set","id":"b1","field":"appearance","value":{"text":"Red","image":same}}]})
            )["changed"],
            false
        );
        let mut automation = Client::connect(&path, Endpoint::Automation).unwrap();
        let image = ok(
            &mut automation,
            "media_read",
            json!({"id":"b1","offset":"0"}),
        );
        assert_eq!(image["eof"], true);
        assert!(image["base64"].as_str().unwrap().len() > 20);
        ok(
            &mut c,
            "edit",
            json!({"changes":[{"kind":"set","id":"b1","field":"appearance","value":{"text":"Red","image":null,"states":{"muted":{"text":"Quiet","image":key}}}}]}),
        );
        assert_eq!(
            ok(
                &mut automation,
                "media_read",
                json!({"id":"b1","media_ref":key,"offset":"0"})
            ),
            image
        );
        let denied = automation
            .request(
                "media_read",
                json!({"id":"b1","media_ref":"0".repeat(64),"offset":"0"}),
            )
            .unwrap();
        assert!(denied.error.is_some(), "cannot read an unrelated digest");
        ok(
            &mut c,
            "set_permissions",
            json!({"target":"b1","value":{"own":{"consult":false,"modify":false,"execute":false}}}),
        );
        assert!(
            automation
                .request(
                    "media_read",
                    json!({"id":"b1","media_ref":key,"offset":"0"})
                )
                .unwrap()
                .error
                .is_some()
        );
        ok(
            &mut c,
            "set_permissions",
            json!({"target":"b1","value":{"inherit":true}}),
        );
        let token = ok(
            &mut c,
            "transfer_begin",
            json!({"kind":"export","size":null}),
        )["transfer_id"]
            .clone();
        let mut pieces = vec![];
        let mut offset = 0;
        loop {
            let part = ok(
                &mut c,
                "transfer_read",
                json!({"transfer_id":token,"offset":offset.to_string()}),
            );
            let text = part["base64"].as_str().unwrap().to_owned();
            let n = text.len() / 4 * 3 - text.bytes().rev().take_while(|b| *b == b'=').count();
            assert!(n <= 8192);
            offset += n;
            pieces.push((text, n));
            if part["eof"] == true {
                break;
            }
        }
        ok(
            &mut c,
            "edit",
            json!({"changes":[{"kind":"set","id":"b1","field":"name","value":"Changed"}]}),
        );
        let token = ok(
            &mut c,
            "transfer_begin",
            json!({"kind":"restore","size":offset.to_string()}),
        )["transfer_id"]
            .clone();
        offset = 0;
        for (text, n) in pieces {
            ok(
                &mut c,
                "transfer_write",
                json!({"transfer_id":token,"offset":offset.to_string(),"base64":text}),
            );
            offset += n;
        }
        let prepared = ok(&mut c, "transfer_finish", json!({"transfer_id":token}));
        assert_eq!(prepared["summary"]["media_count"], 1);
        assert_eq!(
            ok(&mut c, "get", json!({"id":"b1"}))["fields"]["name"],
            "Changed",
            "prepare cannot publish"
        );
        ok(
            &mut c,
            "restore_commit",
            json!({"transfer_id":token,"confirm_replace":true}),
        );
        assert_eq!(
            ok(&mut c, "get", json!({"id":"b1"}))["fields"]["name"],
            "Original"
        );
        assert_eq!(
            ok(
                &mut automation,
                "media_read",
                json!({"id":"b1","media_ref":key,"offset":"0"})
            ),
            image
        );
        // Closing only this editor connection releases both unpublished leases.
        // The server and automation client remain live; a new editor can prepare
        // another full pair without waiting for the original 60-second expiry.
        for pixel in [png, green] {
            let token = ok(
                &mut c,
                "transfer_begin",
                json!({"kind":"image_import","size":"70"}),
            )["transfer_id"]
                .clone();
            ok(
                &mut c,
                "transfer_write",
                json!({"transfer_id":token,"offset":"0","base64":pixel}),
            );
            ok(&mut c, "transfer_finish", json!({"transfer_id":token}));
        }
        drop(c);
        let mut replacement = Client::connect(&path, Endpoint::Editor).unwrap();
        for pixel in [png, green] {
            let end = Instant::now() + Duration::from_secs(1);
            let token = loop {
                let response = replacement
                    .request("transfer_begin", json!({"kind":"image_import","size":"70"}))
                    .unwrap();
                match response.error {
                    None => {
                        break serde_json::to_value(response.data).unwrap()["transfer_id"].clone();
                    }
                    Some(error) => {
                        assert_eq!(error.code, "resource_exhausted");
                        assert!(
                            Instant::now() < end,
                            "disconnect did not release the image quota"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            };
            ok(
                &mut replacement,
                "transfer_write",
                json!({"transfer_id":token,"offset":"0","base64":pixel}),
            );
            ok(
                &mut replacement,
                "transfer_finish",
                json!({"transfer_id":token}),
            );
        }
        assert_eq!(
            ok(&mut automation, "get", json!({"id":"b1"}))["fields"]["name"],
            "Original"
        );
        drop(replacement);
        send.send(()).unwrap();
    });
    let end = Instant::now() + Duration::from_secs(6);
    while receive.try_recv().is_err() {
        server.turn().unwrap();
        assert!(Instant::now() < end);
        server.wait(Duration::from_millis(10)).unwrap();
    }
    client.join().unwrap();
}
