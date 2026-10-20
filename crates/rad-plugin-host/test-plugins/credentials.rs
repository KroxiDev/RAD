//! Isolated API3 peer: only compares a fixture string, never connects to a service.
use rad_plugin_contract::*;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    time::{Duration, Instant},
};
fn main() {
    let mut maps = BTreeMap::<u64, CredentialValues>::new();
    let mut last = 0;
    let mut invocations = 0u64;
    let mut ack_delay = Duration::ZERO;
    let mut input = std::io::stdin().lock();
    loop {
        let mut len = [0; 4];
        if input.read_exact(&mut len).is_err() {
            return;
        }
        let size = u32::from_be_bytes(len) as usize;
        if !(1..=65536).contains(&size) {
            return;
        }
        let mut frame = zeroize::Zeroizing::new(len.to_vec());
        frame.resize(size + 4, 0);
        if input.read_exact(&mut frame[4..]).is_err() {
            return;
        }
        let Ok(Some((envelope, _))) = decode(&frame) else {
            return;
        };
        let message = match envelope.message {
            Message::Hello {
                host_api: 3,
                plugin_id,
                plugin_version,
                ..
            } => {
                ack_delay = match plugin_id.as_str() {
                    "org.rad.test.credentials.delayed" => Duration::from_millis(2500),
                    "org.rad.test.credentials.deferred" => Duration::from_millis(100),
                    _ => Duration::ZERO,
                };
                Message::Ready {
                    plugin_id,
                    plugin_version,
                }
            }
            Message::Credentials { revision, values }
                if revision > last
                    && maps.len() < MAX_CREDENTIAL_REVISIONS
                    && values.iter().all(|v| v.slot == "password") =>
            {
                let Ok(values) = CredentialValues::new(values) else {
                    return;
                };
                maps.insert(revision, values);
                last = revision;
                // Private fixture timer only: no host/global clock injection. The loop
                // processes no Invoke while this ACK is pending.
                let until = Instant::now() + ack_delay;
                while let Some(remaining) = until.checked_duration_since(Instant::now()) {
                    std::thread::sleep(remaining);
                }
                Message::CredentialsApplied { revision }
            }
            Message::CredentialsRelease { revision } if maps.remove(&revision).is_some() => {
                Message::CredentialsReleased { revision }
            }
            Message::Invoke {
                credential_revision: Some(revision),
                ..
            } if maps.contains_key(&revision) => {
                let Some(next) = invocations.checked_add(1) else {
                    return;
                };
                invocations = next;
                let valid = maps[&revision]
                    .as_slice()
                    .iter()
                    .any(|v| v.slot == "password" && v.value == "correct");
                Message::Result {
                    outcome: if valid {
                        Outcome::Success
                    } else {
                        Outcome::Failure {
                            code: FailureCode::CredentialsUnavailable,
                        }
                    },
                }
            }
            Message::Options {
                credential_revision: Some(revision),
                field,
                ..
            } if maps.contains_key(&revision) => {
                let marker = if field == "invocations" {
                    format!("count{invocations}")
                } else {
                    format!("rev{revision}")
                };
                Message::Choices {
                    list: ChoiceList::Available {
                        options: vec![ChoiceOption {
                            value: marker.clone(),
                            label: marker,
                        }],
                    },
                }
            }
            Message::Subscribe {
                credential_revision: Some(revision),
                ..
            } if maps.contains_key(&revision) => Message::Visual {
                state: if maps[&revision]
                    .as_slice()
                    .iter()
                    .any(|v| v.slot == "password" && v.value == "correct")
                {
                    VisualState::Current {
                        content: VisualContent {
                            text: Some(format!("rev{revision}")),
                            png_hex: None,
                            jpeg_hex: None,
                            color: None,
                        },
                    }
                } else {
                    VisualState::Unavailable
                },
            },
            Message::Unsubscribe { .. } => continue,
            _ => return,
        };
        let Ok(frame) = encode(&Envelope {
            protocol: 1,
            correlation: envelope.correlation,
            message,
        }) else {
            return;
        };
        let mut output = std::io::stdout().lock();
        if output
            .write_all(&frame)
            .and_then(|()| output.flush())
            .is_err()
        {
            return;
        }
    }
}
