//! Synthetic native fixture for the compatibility checker. Never reaches external services.
use rad_plugin_contract::*;
use std::{
    collections::BTreeMap,
    io::Read,
    time::{Duration, Instant},
};
fn emit(e: Envelope) {
    let bytes = encode(&e).unwrap();
    let end = Instant::now() + Duration::from_millis(50);
    let mut offset = 0;
    while offset < bytes.len() {
        let written =
            unsafe { libc::write(1, bytes[offset..].as_ptr().cast(), bytes.len() - offset) };
        let result = if written < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(written as usize)
        };
        match result {
            Ok(0) => std::process::exit(74),
            Ok(n) => offset += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= end {
                    std::process::exit(74)
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(_) => std::process::exit(74),
        }
    }
}
/// Bytes the host has not read yet from this fixture's output pipe.
fn unread_output() -> usize {
    let mut queued: libc::c_int = 0;
    // SAFETY: FIONREAD writes one int for the pipe on fd 1.
    if unsafe { libc::ioctl(1, libc::FIONREAD, &mut queued) } < 0 {
        return 0;
    }
    queued as usize
}
/// The reply a slow option query sends when it finishes: larger than a `busy` refusal.
fn large_choices() -> Message {
    Message::Choices {
        list: ChoiceList::Available {
            options: (0..4)
                .map(|i| ChoiceOption {
                    value: format!("ok{i}"),
                    label: "large ".repeat(16),
                })
                .collect(),
        },
    }
}
fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    unsafe {
        libc::fcntl(0, libc::F_SETFL, libc::O_NONBLOCK);
        libc::fcntl(1, libc::F_SETFL, libc::O_NONBLOCK);
    }
    // A child that lives until EOF, as a plugin's helper process does; it is ended on EOF.
    let mut helper = (mode == "child-until-eof").then(|| {
        std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap()
    });
    // One option query runs at a time, as in the SDK: the others are refused at once with a
    // small `busy` reply while the running one finishes when the host waits for it or when the
    // output can no longer take its larger reply.
    let mut slow_query = None;
    let mut last_input = Instant::now();
    let mut options = 0;
    let mut input = Vec::new();
    let mut subs = BTreeMap::new();
    let mut tick = Instant::now();
    let mut secret = String::new();
    loop {
        let mut b = [0u8; 4096];
        match std::io::stdin().read(&mut b) {
            Ok(0) => {
                if let Some(mut helper) = helper.take() {
                    let _ = helper.kill();
                    let _ = helper.wait();
                }
                if mode == "eof-stderr-leak" {
                    eprintln!("{secret}");
                }
                if mode == "eof-escaped-child" {
                    use std::os::unix::process::CommandExt;
                    let _ = std::process::Command::new("/bin/sleep")
                        .arg("30")
                        .process_group(0)
                        .spawn();
                }
                if mode == "eof-child" {
                    let _ = std::process::Command::new("/bin/sleep").arg("30").spawn();
                }
                break;
            }
            Ok(n) => {
                input.extend_from_slice(&b[..n]);
                last_input = Instant::now();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }
        while let Some((e, n)) = decode(&input).unwrap() {
            input.drain(..n);
            let c = e.correlation;
            let reply = match e.message {
                Message::Hello {
                    mut capabilities,
                    mut plugin_id,
                    plugin_version,
                    ..
                } => {
                    if mode == "identity" {
                        plugin_id = "wrong.id".into()
                    }
                    if mode == "capabilities" {
                        capabilities.clear()
                    }
                    Message::Ready {
                        capabilities,
                        plugin_id,
                        plugin_version,
                    }
                }
                Message::Credentials { revision, values } => {
                    secret = values[0].value.clone();
                    if mode == "stderr-leak" {
                        eprintln!("{secret}")
                    };
                    Message::CredentialsApplied { revision }
                }
                Message::CredentialsRelease { revision } => {
                    Message::CredentialsReleased { revision }
                }
                Message::Invoke { .. } => Message::Result {
                    outcome: Outcome::Success,
                },
                Message::Options { .. } if mode == "late-large-reply" => {
                    let Some(slow) = slow_query else {
                        slow_query = Some(c);
                        continue;
                    };
                    let large = Envelope {
                        protocol: 1,
                        correlation: slow,
                        message: large_choices(),
                    };
                    if unread_output() + encode(&large).unwrap().len() > 4096 {
                        // Its reply no longer fits, while a `busy` one still would.
                        emit(large);
                    }
                    Message::Choices {
                        list: ChoiceList::Unavailable,
                    }
                }
                Message::Options { .. } => {
                    options += 1;
                    if options == 4 && mode == "blocked" {
                        unsafe {
                            libc::fcntl(1, libc::F_SETFL, 0);
                        }
                    }
                    if options == 4 && mode == "truncated" {
                        let partial = [0u8, 0, 16, 0, b'{'];
                        unsafe {
                            libc::write(1, partial.as_ptr().cast(), partial.len());
                        }
                    }
                    Message::Choices {
                        list: ChoiceList::Available {
                            options: vec![ChoiceOption {
                                value: "ok".into(),
                                label: if mode == "message-leak" {
                                    secret.clone()
                                } else {
                                    "ok".into()
                                },
                            }],
                        },
                    }
                }
                Message::Subscribe { .. } => {
                    subs.insert(c.request, c);
                    Message::Visual {
                        valid_for_ms: 1000,
                        state: VisualState::Current {
                            content: VisualContent {
                                text: Some(if mode == "visual-leak" {
                                    secret.clone()
                                } else {
                                    "stable".into()
                                }),
                                ..Default::default()
                            },
                        },
                    }
                }
                Message::Unsubscribe { subscription } => {
                    if mode != "ignored-unsubscribe" {
                        subs.remove(&subscription.request);
                    }
                    continue;
                }
                _ => continue,
            };
            if mode == "transient-leak" && matches!(reply, Message::Visual { .. }) {
                emit(Envelope {
                    protocol: 1,
                    correlation: c,
                    message: Message::Visual {
                        valid_for_ms: 1000,
                        state: VisualState::Current {
                            content: VisualContent {
                                text: Some(secret.clone()),
                                ..Default::default()
                            },
                        },
                    },
                });
            }
            emit(Envelope {
                protocol: 1,
                correlation: c,
                message: reply,
            });
        }
        if let Some(slow) = slow_query
            && last_input.elapsed() > Duration::from_millis(20)
        {
            // The host is waiting for this reply.
            slow_query = None;
            emit(Envelope {
                protocol: 1,
                correlation: slow,
                message: large_choices(),
            });
        }
        if tick.elapsed() > Duration::from_millis(200) {
            if mode != "no-renew" {
                for c in subs.values() {
                    emit(Envelope {
                        protocol: 1,
                        correlation: *c,
                        message: Message::VisualRenew { valid_for_ms: 1000 },
                    })
                }
            }
            tick = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
