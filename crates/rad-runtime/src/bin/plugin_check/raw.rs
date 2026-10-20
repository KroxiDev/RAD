//! Adversarial pipe ownership using only the public, typed plugin contract.
use super::{
    Result, SECRET,
    scenario::{Pressure, Scenario},
};
use rad_plugin_contract::*;
use std::{
    io::{Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::Path,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
struct Peer {
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
    stderr: ChildStderr,
    rx: Vec<u8>,
    err: Vec<u8>,
    next: u64,
    capabilities: Vec<String>,
    start: Instant,
}
fn nonblocking(fd: i32) -> Result {
    if unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
        Err("nonblocking_pipe")
    } else {
        Ok(())
    }
}
impl Peer {
    fn new(path: &Path, m: &Manifest, s: &Scenario) -> Result<Self> {
        let mut child = Command::new(path)
            .args(&s.args)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| "raw_spawn")?;
        let input = child.stdin.take().ok_or("raw_stdin")?;
        let output = child.stdout.take().ok_or("raw_stdout")?;
        let stderr = child.stderr.take().ok_or("raw_stderr")?;
        let mut p = Self {
            child,
            input: Some(input),
            output,
            stderr,
            rx: vec![],
            err: vec![],
            next: 1,
            capabilities: m.capabilities.clone(),
            start: Instant::now(),
        };
        nonblocking(p.input.as_ref().unwrap().as_raw_fd())?;
        nonblocking(p.output.as_raw_fd())?;
        nonblocking(p.stderr.as_raw_fd())?;
        if unsafe { libc::fcntl(p.output.as_raw_fd(), libc::F_SETPIPE_SZ, 4096) } != 4096 {
            return Err("pipe_capacity");
        }
        let c = p.send(
            Message::Hello {
                capabilities: SUPPORTED_CAPABILITIES.iter().map(|x| (*x).into()).collect(),
                host_api: HOST_API,
                target: rad_plugin_host::TARGET.into(),
                plugin_id: m.id.clone(),
                plugin_version: m.version,
            },
            s.timeout_ms,
        )?;
        let e = p.receive(s.timeout_ms)?;
        match e.message {
            Message::Ready {
                capabilities,
                plugin_id,
                plugin_version,
            } if e.correlation == c && plugin_id == m.id && plugin_version == m.version => {
                p.capabilities = negotiate_capabilities(&m.capabilities, &capabilities)
                    .map_err(|_| "raw_capabilities")?;
            }
            _ => return Err("raw_handshake"),
        }
        let slots = m.credential_slots();
        if !slots.is_empty() {
            let c = p.send(
                Message::Credentials {
                    revision: 1,
                    values: slots
                        .into_iter()
                        .map(|slot| CredentialValue {
                            slot: slot.into(),
                            value: SECRET.into(),
                        })
                        .collect(),
                },
                s.timeout_ms,
            )?;
            let e = p.receive(s.timeout_ms)?;
            if e.correlation != c
                || !matches!(e.message, Message::CredentialsApplied { revision: 1 })
            {
                return Err("raw_credential_ack");
            }
        }
        Ok(p)
    }
    fn stderr(&mut self) -> Result {
        super::budget()?;
        // Retain a canary-length suffix across reads; never lose a split secret.
        let mut b = [0u8; 4096];
        for _ in 0..16 {
            match self.stderr.read(&mut b) {
                Ok(0) => break,
                Ok(n) => {
                    self.err.extend_from_slice(&b[..n]);
                    if self
                        .err
                        .windows(SECRET.len())
                        .any(|w| w == SECRET.as_bytes())
                    {
                        return Err("credential_leak");
                    }
                    if self.err.len() > SECRET.len() {
                        let split = self.err.len() - SECRET.len();
                        self.err.drain(..split);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => return Err("stderr_read"),
            }
        }
        if self.start.elapsed() > Duration::from_secs(90) {
            return Err("raw_total_budget");
        };
        Ok(())
    }
    fn frame(&mut self, message: Message) -> Result<(Correlation, Vec<u8>)> {
        if !message.permitted(&self.capabilities) {
            return Err("raw_unnegotiated_message");
        }
        let c = Correlation {
            generation: 1,
            request: self.next,
        };
        self.next += 1;
        Ok((
            c,
            encode(&Envelope {
                protocol: 1,
                correlation: c,
                message,
            })
            .map_err(|_| "raw_encode")?,
        ))
    }
    fn send(&mut self, message: Message, timeout: u64) -> Result<Correlation> {
        let (c, bytes) = self.frame(message)?;
        let end = Instant::now() + Duration::from_millis(timeout);
        let mut pos = 0;
        while pos < bytes.len() {
            self.stderr()?;
            match self
                .input
                .as_mut()
                .ok_or("stdin_closed")?
                .write(&bytes[pos..])
            {
                Ok(0) => return Err("stdin_zero"),
                Ok(n) => pos += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err("stdin_write"),
            };
            if Instant::now() >= end {
                return Err("stdin_timeout");
            };
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(c)
    }
    fn receive(&mut self, timeout: u64) -> Result<Envelope> {
        let end = Instant::now() + Duration::from_millis(timeout);
        loop {
            self.stderr()?;
            if let Some((e, n)) = decode(&self.rx).map_err(|_| "raw_decode")? {
                self.rx.drain(..n);
                if !e.message.permitted(&self.capabilities) {
                    return Err("raw_unnegotiated_reply");
                };
                if serde_json::to_string(&e)
                    .map_err(|_| "raw_encode_reply")?
                    .contains(SECRET)
                {
                    return Err("credential_leak");
                }
                return Ok(e);
            }
            let mut b = [0u8; 4096];
            match self.output.read(&mut b) {
                Ok(0) => return Err("stdout_eof"),
                Ok(n) => {
                    self.rx.extend_from_slice(&b[..n]);
                    if self
                        .rx
                        .windows(SECRET.len())
                        .any(|w| w == SECRET.as_bytes())
                    {
                        return Err("credential_leak");
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err("stdout_read"),
            };
            if self.rx.len() > 65540 {
                return Err("raw_rx_budget");
            };
            if Instant::now() >= end {
                return Err("raw_response_timeout");
            };
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn drain(&mut self) -> Result {
        let mut bytes = [0u8; 4096];
        for _ in 0..16 {
            match self.output.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => self.rx.extend_from_slice(&bytes[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => return Err("eof_read"),
            }
            while let Some((e, n)) = decode(&self.rx).map_err(|_| "eof_decode")? {
                self.rx.drain(..n);
                if !e.message.permitted(&self.capabilities) {
                    return Err("eof_unnegotiated_reply");
                }
                if serde_json::to_string(&e)
                    .map_err(|_| "eof_encode")?
                    .contains(SECRET)
                {
                    return Err("credential_leak");
                }
            }
            if self.rx.len() > 65540 {
                return Err("eof_rx_budget");
            }
        }
        Ok(())
    }
    fn status(&mut self) -> Result<Option<ExitStatus>> {
        self.stderr()?;
        self.child.try_wait().map_err(|_| "raw_wait")
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        };
        let _ = self.child.wait();
        let end = Instant::now() + Duration::from_secs(1);
        loop {
            let result = unsafe {
                libc::waitpid(
                    -(self.child.id() as i32),
                    std::ptr::null_mut(),
                    libc::WNOHANG,
                )
            };
            if result < 0 || Instant::now() >= end {
                break;
            };
            if result == 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}
fn vector(m: &Manifest, s: &Scenario) -> Message {
    let revision = (!m.credential_slots().is_empty()).then_some(1);
    match s.pressure {
        Pressure::Options { index, .. } => {
            let o = &s.options[index];
            Message::Options {
                scope: o.scope,
                definition: o.definition.clone(),
                field: o.field.clone(),
                context: o.context.clone(),
                settings: s.settings.clone(),
                credential_revision: revision,
            }
        }
        Pressure::Visual { index, .. } => {
            let v = &s.visuals[index];
            Message::Subscribe {
                source: v.id.clone(),
                parameters: v.parameters.clone(),
                settings: s.settings.clone(),
                credential_revision: revision,
            }
        }
        Pressure::Action { index, .. } => {
            let a = &s.actions[index];
            Message::Invoke {
                action: a.id.clone(),
                parameters: a.parameters.clone(),
                settings: s.settings.clone(),
                credential_revision: revision,
                control: if supports(&m.capabilities, CONTROLS) {
                    Some(ControlEvent {
                        control: Control {
                            kind: "key".into(),
                            index: 0,
                        },
                        gesture: match a.gesture {
                            Gesture::Press => ControlGesture::Press,
                            Gesture::LongPress => ControlGesture::LongPress,
                            Gesture::Hold => ControlGesture::HoldStart,
                        },
                    })
                } else {
                    None
                },
            }
        }
    }
}
pub(super) fn eof(path: &Path, m: &Manifest, s: &Scenario) -> Result {
    let mut p = Peer::new(path, m, s)?;
    let c = p.send(vector(m, s), s.timeout_ms)?;
    let e = p.receive(s.timeout_ms)?;
    if c != e.correlation {
        return Err("eof_vector_unobservable");
    }
    p.input.take();
    let end = Instant::now() + Duration::from_millis(s.timeout_ms);
    loop {
        if let Some(status) = p.status()? {
            p.stderr()?;
            if !status.success() {
                return Err("eof_abnormal_exit");
            }
            if unsafe { libc::kill(-(p.child.id() as i32), 0) } == 0
                || !super::children()?.is_empty()
            {
                return Err("eof_live_descendant");
            }
            p.drain()?;
            if !p.rx.is_empty() {
                return Err("eof_partial_output");
            }
            return Ok(());
        }
        // Continue draining output: this check isolates EOF from output pressure.
        p.drain()?;
        if Instant::now() >= end {
            return Err("eof_timeout");
        };
        std::thread::sleep(Duration::from_millis(1));
    }
}
pub(super) fn pressure(path: &Path, m: &Manifest, s: &Scenario) -> Result {
    let mut p = Peer::new(path, m, s)?;
    let end = Instant::now() + Duration::from_millis(s.timeout_ms);
    let mut pending = Vec::new();
    let mut offset = 0;
    let mut count = 0;
    let mut sub = None;
    let mut end_hold = false;
    let status = loop {
        if let Some(status) = p.status()? {
            break status;
        }
        if Instant::now() >= end {
            return Err("blocked_stdout_not_terminal");
        }
        if offset == pending.len() {
            // A visual source never accumulates subscriptions: alternate high/low.
            let mut message = if let Some(c) = sub.take() {
                Message::Unsubscribe { subscription: c }
            } else {
                vector(m, s)
            };
            if let Message::Invoke {
                control: Some(control),
                ..
            } = &mut message
                && control.gesture == ControlGesture::HoldStart
            {
                if end_hold {
                    control.gesture = ControlGesture::HoldEnd;
                }
                end_hold = !end_hold;
            }
            let is_sub = matches!(message, Message::Subscribe { .. });
            let (c, bytes) = p.frame(message)?;
            if is_sub {
                sub = Some(c)
            };
            pending = bytes;
            offset = 0;
            count += 1;
            if count > 100_000 {
                return Err("pressure_request_budget");
            }
        }
        match p.input.as_mut().unwrap().write(&pending[offset..]) {
            Ok(n) => offset += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::BrokenPipe
                ) => {}
            Err(_) => return Err("pressure_write"),
        };
        // Deliberately no stdout read until process termination.
        std::thread::sleep(Duration::from_micros(100));
    };
    p.stderr()?;
    if status.success() {
        return Err("pressure_not_output_failure");
    }
    let mut bytes = Vec::new();
    let mut b = [0u8; 4096];
    loop {
        match p.output.read(&mut b) {
            Ok(0) => break,
            Ok(n) => {
                bytes.extend_from_slice(&b[..n]);
                if bytes.len() > 65540 {
                    return Err("pressure_output_budget");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => return Err("pressure_read"),
        }
    }
    if bytes.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()) {
        return Err("credential_leak");
    }
    let mut offset = 0;
    let mut largest = 0;
    while let Some((e, n)) = decode(&bytes[offset..]).map_err(|_| "output_after_truncated_frame")? {
        if !e.message.permitted(&p.capabilities) {
            return Err("pressure_unnegotiated_reply");
        };
        offset += n;
        largest = largest.max(n);
    }
    // An incomplete final frame is permitted; an independently valid later frame is not.
    for i in offset + 1..bytes.len() {
        if matches!(decode(&bytes[i..]), Ok(Some(_))) {
            return Err("output_after_truncated_frame");
        }
    }
    if bytes.is_empty() || bytes.len() + largest < 4096 {
        return Err("stdout_pressure_not_observed");
    }
    Ok(())
}

/// Observe actual source quiescence, not merely removal from Host's projection.
pub(super) fn unsubscribe(path: &Path, m: &Manifest, s: &Scenario) -> Result {
    if s.visuals.is_empty() {
        return Ok(());
    }
    let mut p = Peer::new(path, m, s)?;
    let revision = (!m.credential_slots().is_empty()).then_some(1);
    let mut subscriptions = Vec::new();
    for v in &s.visuals {
        let c = p.send(
            Message::Subscribe {
                source: v.id.clone(),
                parameters: v.parameters.clone(),
                settings: s.settings.clone(),
                credential_revision: revision,
            },
            s.timeout_ms,
        )?;
        let end = Instant::now() + Duration::from_millis(s.timeout_ms);
        loop {
            let e = p.receive(s.timeout_ms)?;
            if e.correlation == c
                && matches!(
                    e.message,
                    Message::Visual {
                        state: VisualState::Current { .. },
                        ..
                    }
                )
            {
                break;
            }
            if Instant::now() >= end {
                return Err("unsubscribe_source_unobservable");
            }
        }
        subscriptions.push(c);
    }
    for c in &subscriptions {
        p.send(Message::Unsubscribe { subscription: *c }, s.timeout_ms)?;
    }
    let grace = Instant::now() + Duration::from_millis(s.timeout_ms);
    let end = grace + Duration::from_millis(s.freshness_ms);
    while Instant::now() < end {
        p.stderr()?;
        let mut bytes = [0u8; 4096];
        match p.output.read(&mut bytes) {
            Ok(0) => return Err("unsubscribe_process_exited"),
            Ok(n) => p.rx.extend_from_slice(&bytes[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return Err("unsubscribe_read"),
        }
        if p.rx.len() > 65540 {
            return Err("unsubscribe_rx_budget");
        }
        while let Some((e, n)) = decode(&p.rx).map_err(|_| "unsubscribe_decode")? {
            p.rx.drain(..n);
            if serde_json::to_string(&e)
                .map_err(|_| "unsubscribe_encode")?
                .contains(SECRET)
            {
                return Err("credential_leak");
            }
            if !subscriptions.contains(&e.correlation)
                || !matches!(
                    e.message,
                    Message::Visual { .. } | Message::VisualRenew { .. }
                )
            {
                return Err("unsubscribe_unexpected_reply");
            }
            if Instant::now() >= grace {
                return Err("unsubscribe_source_still_active");
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    if !p.rx.is_empty() {
        return Err("unsubscribe_partial_output");
    }
    Ok(())
}
