//! Synthetic plugin for the dynamic-options tests: `org.rad.test.options` 1.0.0, major 1 with dynamic_options.
//! Lists come from a fixed table keyed by the `scenario` dependency, which also injects
//! faults (slow, late, crash, invalid). No apps, audio, hardware, network or files.
use rad_plugin_contract::*;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const ID: &str = "org.rad.test.options";
const VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};
/// 16x16 baseline JPEG (301 bytes) written by ImageMagick; small enough to be a visual state.
const JPEG_HEX: &str = "ffd8ffe000104a46494600010100000100010000ffdb0043000d090a0b0a080d0b0a0b0e0e0d0f13201513121213271c1e17202e2931302e292d2c333a4a3e333646372c2d405741464c4e525352323e5a615a50604a51524fffdb0043010e0e0e131113261515264f352d354f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4fffc00011080010001003012200021101031101ffc400160001010100000000000000000000000000000405ffc400161000030000000000000000000000000000001461ffc4001501010100000000000000000000000000000002ffc40017110003010000000000000000000000000000146115ffda000c03010002110311003f0089581586cab02b09d6a1187fffd9";

#[derive(Default)]
struct Shared {
    invocations: AtomicU64,
    settings_text: Mutex<Option<String>>,
    subscription: Mutex<Option<Correlation>>,
}

fn write_frame(payload: &[u8]) {
    let mut out = std::io::stdout().lock();
    let written = out
        .write_all(&(payload.len() as u32).to_be_bytes())
        .and_then(|_| out.write_all(payload))
        .and_then(|_| out.flush());
    if written.is_err() {
        std::process::exit(0);
    }
}
fn send(correlation: Correlation, message: Message) {
    let envelope = Envelope {
        protocol: 1,
        correlation,
        message,
    };
    write_frame(&encode(&envelope).expect("valid frame")[4..]);
}
fn probe_state(shared: &Shared) -> Message {
    Message::Visual {
        valid_for_ms: 1000,
        state: VisualState::Current {
            content: VisualContent {
                text: Some(
                    shared
                        .settings_text
                        .lock()
                        .expect("settings")
                        .clone()
                        .unwrap_or_else(|| {
                            format!("invocations:{}", shared.invocations.load(Ordering::SeqCst))
                        }),
                ),
                png_hex: None,
                jpeg_hex: Some(JPEG_HEX.into()),
                color: None,
            },
        },
    }
}
fn option(value: &str, label: &str) -> ChoiceOption {
    ChoiceOption {
        value: value.into(),
        label: label.into(),
    }
}
fn list(field: &str, context: &Parameters) -> ChoiceList {
    let text = |key: &str| context.values.get(key).and_then(Value::as_str);
    let options = match (text("scenario"), field) {
        (Some("empty"), _) => vec![],
        (Some("normal" | "slow" | "late"), "group") => {
            vec![option("a", "Group A"), option("b", "Group B")]
        }
        (Some("normal" | "slow" | "late"), "member") => match text("group") {
            Some("a") => vec![option("a1", "Member A1"), option("a2", "Member A2")],
            Some("b") => vec![option("b1", "Member B1")],
            _ => vec![],
        },
        _ => return ChoiceList::Unavailable,
    };
    ChoiceList::Available { options }
}
fn answer_options(correlation: Correlation, field: &str, context: &Parameters) {
    let delay = match context.values.get("scenario").and_then(Value::as_str) {
        Some("crash") => std::process::exit(29),
        Some("invalid") => {
            // Deliberately outside the contract: 65 options. Raw JSON bypasses `encode`.
            let options: Vec<_> = (0..=MAX_CHOICES)
                .map(|n| json!({"value":format!("v{n}"),"label":format!("Option {n}")}))
                .collect();
            let frame = json!({"protocol":1,"correlation":correlation,"message":{"kind":"choices",
                "list":{"availability":"available","options":options}}});
            write_frame(&serde_json::to_vec(&frame).expect("json"));
            return;
        }
        Some("slow") => Some(3000),
        Some("late") => Some(500),
        _ => None,
    };
    let message = Message::Choices {
        list: list(field, context),
    };
    match delay {
        // A thread keeps the plugin serving other frames while the reply is pending.
        Some(ms) => {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(ms));
                send(correlation, message);
            });
        }
        None => send(correlation, message),
    }
}
fn handle(envelope: Envelope, shared: &Arc<Shared>) {
    let correlation = envelope.correlation;
    match envelope.message {
        Message::Hello { capabilities, .. } => send(
            correlation,
            Message::Ready {
                capabilities,
                plugin_id: ID.into(),
                plugin_version: VERSION,
            },
        ),
        Message::Invoke { .. } => {
            shared.invocations.fetch_add(1, Ordering::SeqCst);
            send(
                correlation,
                Message::Result {
                    outcome: Outcome::Success,
                },
            );
        }
        Message::Subscribe { settings, .. } => {
            *shared.settings_text.lock().expect("settings") =
                settings.and_then(|s| s.values.get("port").map(|v| format!("port{v}")));
            *shared.subscription.lock().expect("subscription") = Some(correlation);
            send(correlation, probe_state(shared));
        }
        Message::Unsubscribe { subscription } => {
            let mut current = shared.subscription.lock().expect("subscription");
            if *current == Some(subscription) {
                *current = None;
            }
        }
        Message::Options {
            field,
            context,
            settings,
            ..
        } => {
            if let Some(port) = settings.and_then(|s| s.values.get("port").cloned()) {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(500));
                    send(
                        correlation,
                        Message::Choices {
                            list: ChoiceList::Available {
                                options: vec![option(
                                    &format!("port{port}"),
                                    &format!("Port {port}"),
                                )],
                            },
                        },
                    );
                });
            } else {
                answer_options(correlation, &field, &context);
            }
        }
        _ => std::process::exit(2),
    }
}
fn main() {
    let shared = Arc::new(Shared::default());
    let ticker = Arc::clone(&shared);
    // The runtime expires visuals after 1000 ms: refresh well inside that.
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let subscription = *ticker.subscription.lock().expect("subscription");
            if let Some(correlation) = subscription {
                send(correlation, probe_state(&ticker));
            }
        }
    });
    let mut input = std::io::stdin().lock();
    let mut buffer = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        match input.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
        loop {
            match decode(&buffer) {
                Ok(Some((envelope, used))) => {
                    buffer.drain(..used);
                    handle(envelope, &shared);
                }
                Ok(None) => break,
                Err(_) => return,
            }
        }
    }
}
