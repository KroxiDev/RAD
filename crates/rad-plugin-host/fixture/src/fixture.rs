// Standalone synthetic fixture. No RAD dependency, apps, hardware, network or files.
use serde_json::{Value, json};
use std::io::{Read, Write};
fn send(v: Value) {
    let b = serde_json::to_vec(&v).unwrap();
    let mut out = std::io::stdout().lock();
    if out
        .write_all(&(b.len() as u32).to_be_bytes())
        .and_then(|_| out.write_all(&b))
        .and_then(|_| out.flush())
        .is_err()
    {
        std::process::exit(0);
    }
}
pub fn run(major: u32) {
    let mut input = std::io::stdin().lock();
    let mut subscription: Option<Value> = None;
    loop {
        let mut header = [0; 4];
        if input.read_exact(&mut header).is_err() {
            return;
        }
        let len = u32::from_be_bytes(header) as usize;
        if !(1..=65536).contains(&len) {
            return;
        }
        let mut b = vec![0; len];
        if input.read_exact(&mut b).is_err() {
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(&b) else {
            return;
        };
        let c = &v["correlation"];
        match v["message"]["kind"].as_str() {
            Some("hello") => send(
                json!({"protocol":1,"correlation":c,"message":{"kind":"ready","capabilities":v["message"]["capabilities"],"plugin_id":"org.rad.fixture","plugin_version":{"major":major,"minor":0,"patch":0}}}),
            ),
            Some("invoke") => {
                let mode = v["message"]["parameters"]["values"]["mode"]
                    .as_str()
                    .unwrap_or("echo");
                if mode == "crash" {
                    std::process::exit(23);
                }
                if mode == "slow" {
                    if let Some(key) = &subscription {
                        send(
                            json!({"protocol":1,"correlation":key,"message":{"kind":"visual","valid_for_ms":1000,"state":{"availability":"current","content":{"text":"running","png_hex":null,"color":null}}}}),
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                if mode == "late" {
                    let mut old = c.clone();
                    old["request"] = json!(c["request"].as_u64().unwrap().saturating_sub(1));
                    send(
                        json!({"protocol":1,"correlation":old,"message":{"kind":"result","outcome":{"status":"success"}}}),
                    );
                }
                let environment_valid = std::env::args().next().as_deref()
                    == Some("org.rad.fixture")
                    && std::env::var("XDG_RUNTIME_DIR").as_deref() == Ok("/synthetic-runtime")
                    && std::env::var("DBUS_SESSION_BUS_ADDRESS").as_deref()
                        == Ok("unix:path=/synthetic-bus")
                    && std::env::vars_os().count() == 2;
                let outcome = if mode == "fail"
                    || (mode == "end_fail" && v["message"]["control"]["gesture"] == "hold_end")
                    || (mode == "environment" && !environment_valid)
                {
                    json!({"status":"failure","code":"unavailable"})
                } else {
                    json!({"status":"success"})
                };
                let reply = json!({"protocol":1,"correlation":c,"message":{"kind":"result","outcome":outcome}});
                for _ in 0..if mode == "flood" { 1000 } else { 1 } {
                    send(reply.clone());
                }
            }
            Some("subscribe") => {
                subscription = Some(c.clone());
                send(
                    json!({"protocol":1,"correlation":c,"message":{"kind":"visual","valid_for_ms":1000,"state":{"availability":"current","content":{"text":format!("v{major}"),"png_hex":null,"color":null}}}}),
                );
            }
            Some("unsubscribe") => {
                if subscription.as_ref() == Some(&v["message"]["subscription"]) {
                    subscription = None;
                }
            }
            _ => return,
        }
    }
}
