//! Bounded Linux pathname transport. Endpoint origin is not proof of human identity.
//!
//! Single-owner event-loop seam: `drive(now_ms)` establishes a fresh clock epoch,
//! expires deadlines, and performs bounded nonblocking I/O. Drain `take_next` only
//! when the authority can START that request; do not build a second work queue.
//! `finish` enqueues a caller-encoded response, without writing or semantic checks.
//! Call `drive` with a fresh timestamp before handling completions/admissions after
//! elapsed external work, and always after `wait_readiness`. Never run blocking
//! authority/Store work inside this owner. The clock is monotonic milliseconds;
//! accepted range is 0..=u64::MAX-60000 (all future deadlines representable).
//! Bind once per unique, nonzero runtime session; do not reuse sessions after drop.
//! Dropping this adapter does not cancel external jobs or commits.
use mio::{
    Events, Interest, Poll, Token,
    net::{UnixListener, UnixStream},
};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Read, Write},
    path::Path,
    time::Duration,
};
mod paths;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    UnsafePath,
    InstanceBusy,
    Io,
    InvalidSession,
    TimeReversed,
    TimeOverflow,
    UnknownRequest,
    InvalidResponse,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Editor,
    Automation,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestKey {
    session: u128,
    connection: u64,
    id: u64,
}
impl RequestKey {
    pub fn connection(self) -> u64 {
        self.connection
    }
    pub fn id(self) -> u64 {
        self.id
    }
}
#[derive(Debug)]
pub struct Incoming {
    pub key: RequestKey,
    pub origin: Origin,
    pub request: rad_wire::Request,
}
struct Output {
    bytes: Vec<u8>,
    offset: usize,
    queued: u64,
}
struct Pending {
    started: bool,
    received: u64,
    operation: String,
}
struct Connection {
    stream: UnixStream,
    origin: Origin,
    wire: rad_wire::Session,
    input: Vec<u8>,
    readable: bool,
    writable: bool,
    output: VecDeque<Output>,
    accepted: u64,
    last_received: u64,
    subscribed: bool,
    notices: BTreeMap<u8, Vec<u8>>,
    hello: bool,
    close_after_flush: bool,
    frame_started: Option<u64>,
}
pub struct Transport {
    editor: UnixListener,
    automation: UnixListener,
    _instance: paths::Instance,
    poll: Poll,
    events: Events,
    connections: BTreeMap<u64, Connection>,
    next: u64,
    ready: VecDeque<Incoming>,
    pending: BTreeMap<RequestKey, Pending>,
    now: u64,
    session: u128,
    listeners_ready: [bool; 2],
    /// A worker-owned source (token `usize::MAX`) fired during the poll inside `drive`.
    worker_edge: bool,
}
impl Transport {
    /// Creates private endpoints; no environment lookup, fallback or session generation.
    pub fn bind(runtime: &Path, session: u128) -> Result<Self, TransportError> {
        if session == 0 {
            return Err(TransportError::InvalidSession);
        }
        let mut instance = paths::Instance::acquire(runtime)?;
        let mut editor = instance.bind("editor.sock")?;
        let mut automation = instance.bind("automation.sock")?;
        let poll = Poll::new().map_err(|_| TransportError::Io)?;
        poll.registry()
            .register(&mut editor, Token(0), Interest::READABLE)
            .map_err(|_| TransportError::Io)?;
        poll.registry()
            .register(&mut automation, Token(1), Interest::READABLE)
            .map_err(|_| TransportError::Io)?;
        Ok(Self {
            editor,
            automation,
            _instance: instance,
            poll,
            events: Events::with_capacity(32),
            connections: BTreeMap::new(),
            next: 2,
            ready: VecDeque::new(),
            pending: BTreeMap::new(),
            now: 0,
            session,
            listeners_ready: [false; 2],
            worker_edge: false,
        })
    }
    /// Deadline-first, bounded I/O turn. Invalid time leaves state and I/O untouched.
    pub fn drive(&mut self, now: u64) -> Result<(), TransportError> {
        if now < self.now {
            return Err(TransportError::TimeReversed);
        }
        now.checked_add(60000).ok_or(TransportError::TimeOverflow)?;
        self.now = now;
        let expired: Vec<_> = self
            .connections
            .iter()
            .filter(|(_, c)| {
                (!c.hello && now.saturating_sub(c.accepted) >= 2000)
                    || c.frame_started
                        .is_some_and(|start| now.saturating_sub(start) >= 2000)
                    || (!c.subscribed && now.saturating_sub(c.last_received) >= 60000)
                    || c.output
                        .front()
                        .is_some_and(|o| now.saturating_sub(o.queued) >= 2000)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.connections.remove(&id);
            self.disconnected(id);
        }
        self.expire_queued();
        self.poll_edges(Duration::ZERO)?;
        for index in 0..2 {
            if self.listeners_ready[index] {
                self.accept_connections(if index == 0 {
                    Origin::Editor
                } else {
                    Origin::Automation
                })?;
            }
        }
        let ids: Vec<_> = self.connections.keys().copied().collect();
        for id in ids {
            let mut c = self.connections.remove(&id).unwrap();
            if self.read(id, &mut c) && self.write(&mut c) {
                self.connections.insert(id, c);
            } else {
                self.disconnected(id);
            }
        }
        Ok(())
    }
    fn accept_connections(&mut self, origin: Origin) -> Result<(), TransportError> {
        let listener = if origin == Origin::Editor {
            &mut self.editor
        } else {
            &mut self.automation
        };
        for _ in 0..16 {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    if self.connections.len() >= 8
                        || (origin == Origin::Automation
                            && self
                                .connections
                                .values()
                                .filter(|c| c.origin == Origin::Automation)
                                .count()
                                >= 6)
                    {
                        continue;
                    }
                    // One reserved token wakes the owner for all trusted worker sources.
                    // The owner drains each worker nonblocking; no socket can share it.
                    if self.next >= usize::MAX as u64 {
                        return Err(TransportError::TimeOverflow);
                    }
                    let id = self.next;
                    self.next = self
                        .next
                        .checked_add(1)
                        .ok_or(TransportError::TimeOverflow)?;
                    self.poll
                        .registry()
                        .register(
                            &mut stream,
                            Token(usize::try_from(id).map_err(|_| TransportError::TimeOverflow)?),
                            Interest::READABLE | Interest::WRITABLE,
                        )
                        .map_err(|_| TransportError::Io)?;
                    self.connections.insert(
                        id,
                        Connection {
                            stream,
                            origin,
                            wire: rad_wire::Session::default(),
                            input: Vec::new(),
                            readable: true,
                            writable: true,
                            output: VecDeque::new(),
                            accepted: self.now,
                            last_received: self.now,
                            subscribed: false,
                            notices: BTreeMap::new(),
                            hello: false,
                            close_after_flush: false,
                            frame_started: None,
                        },
                    );
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.listeners_ready[if origin == Origin::Editor { 0 } else { 1 }] = false;
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(TransportError::Io),
            }
        }
        Ok(())
    }
    fn read(&mut self, id: u64, c: &mut Connection) -> bool {
        for _ in 0..16 {
            if !c.readable || c.close_after_flush {
                break;
            }
            let target = if c.input.len() < 4 {
                4
            } else {
                let length = u32::from_be_bytes(c.input[..4].try_into().unwrap()) as usize;
                if !(1..=65536).contains(&length) {
                    return false;
                }
                4 + length
            };
            let mut buffer = [0u8; 4096];
            let count = (target - c.input.len()).min(buffer.len());
            match c.stream.read(&mut buffer[..count]) {
                Ok(0) => return false,
                Ok(n) => {
                    c.last_received = self.now;
                    if c.input.is_empty() {
                        c.frame_started = Some(self.now);
                    }
                    c.input.extend_from_slice(&buffer[..n]);
                    match rad_wire::read_frame(&c.input) {
                        Ok(Some(frame)) => {
                            let request = match c.wire.accept(frame) {
                                Ok(r) => r,
                                Err(e) => {
                                    let Some(request_id) = e.id else { return false };
                                    let code = match e.code {
                                        rad_wire::ErrorCode::UnsupportedVersion => {
                                            rad_api::ErrorCode::UnsupportedVersion
                                        }
                                        _ => rad_api::ErrorCode::InvalidRequest,
                                    };
                                    // A correlated envelope rejection has not been admitted.
                                    // Recover only an unambiguous operation through the same
                                    // bounded, duplicate-rejecting JSON parser as rad-wire.
                                    let operation = rad_wire::parse_json(frame.payload())
                                        .ok()
                                        .and_then(|value| {
                                            value
                                                .get("op")
                                                .and_then(serde_json::Value::as_str)
                                                .map(str::to_owned)
                                        })
                                        .unwrap_or_default();
                                    if !self.error(c, request_id, code, &operation) {
                                        return false;
                                    }
                                    c.close_after_flush = e.close;
                                    c.input.clear();
                                    c.frame_started = None;
                                    continue;
                                }
                            };
                            c.hello = true;
                            if self.pending.len() >= 32
                                || self.pending.keys().filter(|k| k.connection == id).count() >= 4
                            {
                                if !self.error(
                                    c,
                                    request.id(),
                                    rad_api::ErrorCode::ResourceExhausted,
                                    request.operation(),
                                ) {
                                    return false;
                                }
                                c.input.clear();
                                c.frame_started = None;
                                continue;
                            }
                            let key = RequestKey {
                                session: self.session,
                                connection: id,
                                id: request.id(),
                            };
                            self.pending.insert(
                                key,
                                Pending {
                                    started: false,
                                    received: self.now,
                                    operation: request.operation().into(),
                                },
                            );
                            self.ready.push_back(Incoming {
                                key,
                                origin: c.origin,
                                request,
                            });
                            c.input.clear();
                            c.frame_started = None;
                        }
                        Ok(None) => {}
                        Err(_) => return false,
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    c.readable = false;
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return false,
            }
        }
        true
    }
    /// Transfers one queued request to started in the current `drive` epoch.
    /// Started requests retain a global slot across disconnect until `finish`.
    pub fn take_next(&mut self) -> Option<Incoming> {
        let incoming = self.ready.pop_front()?;
        self.pending.get_mut(&incoming.key)?.started = true;
        Some(incoming)
    }
    fn disconnected(&mut self, id: u64) {
        self.ready.retain(|r| r.key.connection != id);
        self.pending
            .retain(|key, p| key.connection != id || p.started);
    }
    fn write(&mut self, c: &mut Connection) -> bool {
        for _ in 0..16 {
            if !c.writable {
                break;
            }
            if c.output.is_empty()
                && let Some((_, payload)) = c.notices.pop_first()
                && !Self::enqueue(c, &payload, self.now)
            {
                return false;
            }
            let Some(front) = c.output.front_mut() else {
                break;
            };
            match c.stream.write(&front.bytes[front.offset..]) {
                Ok(0) => return false,
                Ok(n) => {
                    front.offset += n;
                    if front.offset == front.bytes.len() {
                        c.output.pop_front();
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    c.writable = false;
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return false,
            }
        }
        !(c.close_after_flush && c.output.is_empty())
    }
}
impl Transport {
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }
}
impl Transport {
    /// Completes a started request once. Payload is authority-encoded, 1..=65536 bytes.
    /// Success releases the slot, NOT proof of delivery: a slow/disconnected peer
    /// may lose the response. Does not cancel or finish any external execution.
    pub fn finish(&mut self, key: RequestKey, payload: &[u8]) -> Result<(), TransportError> {
        if !self.pending.get(&key).is_some_and(|p| p.started) {
            return Err(TransportError::UnknownRequest);
        }
        if payload.is_empty() || payload.len() > 65536 {
            return Err(TransportError::InvalidResponse);
        }
        self.pending.remove(&key);
        if let Some(mut c) = self.connections.remove(&key.connection) {
            if Self::enqueue(&mut c, payload, self.now) {
                self.connections.insert(key.connection, c);
            } else {
                self.disconnected(key.connection);
            }
        }
        Ok(())
    }
}
impl Transport {
    pub fn request_count(&self) -> usize {
        self.pending.len()
    }
}

impl Transport {
    fn error(
        &self,
        c: &mut Connection,
        id: u64,
        code: rad_api::ErrorCode,
        operation: &str,
    ) -> bool {
        let mutation =
            rad_api::Operation::parse(operation).is_some_and(rad_api::Operation::is_mutation);
        let response = rad_api::Response {
            v: rad_api::VERSION,
            id: Some(id.to_string()),
            session_id: Some(format!("{:032x}", self.session)),
            status: rad_api::Status::Error,
            data: rad_api::ResponseData::None,
            error: Some(rad_api::Error {
                code,
                message: "Request rejected".into(),
                details: serde_json::json!({}),
                outcome: mutation.then_some(rad_api::Outcome::NotApplied),
            }),
        };
        let payload = serde_json::to_vec(&response).expect("serializable transport rejection");
        Self::enqueue(c, &payload, self.now)
    }
}

impl Transport {
    fn expire_queued(&mut self) {
        let expired: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| !p.started && self.now.saturating_sub(p.received) >= 5000)
            .map(|(key, _)| *key)
            .collect();
        for key in expired {
            let Some(pending) = self.pending.remove(&key) else {
                continue;
            };
            self.ready.retain(|r| r.key != key);
            if let Some(mut c) = self.connections.remove(&key.connection) {
                if self.error(
                    &mut c,
                    key.id,
                    rad_api::ErrorCode::DeadlineExceeded,
                    &pending.operation,
                ) {
                    self.connections.insert(key.connection, c);
                } else {
                    self.disconnected(key.connection);
                }
            }
        }
    }
}

impl Transport {
    fn enqueue(c: &mut Connection, payload: &[u8], now: u64) -> bool {
        if c.output.len() >= 16
            || c.output.iter().map(|o| o.bytes.len()).sum::<usize>() + payload.len() + 4 > 1048576
        {
            return false;
        }
        let bytes = [(payload.len() as u32).to_be_bytes().as_slice(), payload].concat();
        c.output.push_back(Output {
            bytes,
            offset: 0,
            queued: now,
        });
        c.writable = true;
        true
    }
}
impl Transport {
    /// Earliest absolute deadline; current epoch means retained readiness needs drive.
    pub fn next_deadline(&self) -> Option<u64> {
        let mut next = None;
        let mut include = |time| {
            next = Some(next.map_or(time, |old: u64| old.min(time)));
        };
        if self.listeners_ready.iter().any(|r| *r) {
            include(self.now);
        }
        for c in self.connections.values() {
            if (c.readable && !c.close_after_flush)
                || (c.writable && (!c.output.is_empty() || !c.notices.is_empty()))
            {
                include(self.now);
            }
            if !c.hello {
                include(c.accepted + 2000);
            }
            if let Some(start) = c.frame_started {
                include(start + 2000);
            }
            if !c.subscribed {
                include(c.last_received + 60000);
            }
            if let Some(o) = c.output.front() {
                include(o.queued + 2000);
            }
        }
        for p in self.pending.values() {
            if !p.started {
                include(p.received + 5000);
            }
        }
        next
    }
}
impl Transport {
    fn wait_timeout(&self, requested: Duration) -> Duration {
        self.next_deadline().map_or(requested, |deadline| {
            requested.min(Duration::from_millis(deadline.saturating_sub(self.now)))
        })
    }
    /// IO-owner-only wait, clamped to deadlines. Records edges but performs no IO.
    /// EINTR returns for a fresh clock/drive; spurious readiness is harmless.
    ///
    /// Returns at once when `drive` already consumed a worker edge. Mio reports each edge
    /// once, and that owner may have been pumped before `drive` polled: blocking here
    /// would lose the wakeup, as it did for a shutdown signal.
    pub fn wait_readiness(&mut self, timeout: Duration) -> Result<(), TransportError> {
        if std::mem::take(&mut self.worker_edge) {
            return Ok(());
        }
        self.poll_edges(self.wait_timeout(timeout))?;
        // Every worker owner is pumped on the turn that follows a wait.
        self.worker_edge = false;
        Ok(())
    }
    fn poll_edges(&mut self, timeout: Duration) -> Result<(), TransportError> {
        match self.poll.poll(&mut self.events, Some(timeout)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => return Ok(()),
            Err(_) => return Err(TransportError::Io),
        }
        for e in self.events.iter() {
            let token = e.token().0;
            if token == usize::MAX {
                self.worker_edge = true;
            } else if token < 2 {
                self.listeners_ready[token] = true;
            } else if let Some(c) = self.connections.get_mut(&(token as u64)) {
                c.readable |= e.is_readable() || e.is_read_closed() || e.is_error();
                c.writable |= e.is_writable() || e.is_write_closed() || e.is_error();
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests;

impl Transport {
    /// Single actor wake used by bounded worker completion channels.
    pub fn worker_waker(&self) -> Result<mio::Waker, TransportError> {
        mio::Waker::new(self.poll.registry(), Token(usize::MAX)).map_err(|_| TransportError::Io)
    }
    /// IO from trusted workers shares the actor wait; caller drains it nonblocking.
    pub fn register_worker_io<S: mio::event::Source>(
        &self,
        source: &mut S,
        interest: Interest,
    ) -> Result<(), TransportError> {
        self.poll
            .registry()
            .register(source, Token(usize::MAX), interest)
            .map_err(|_| TransportError::Io)
    }
    pub fn reregister_worker_io<S: mio::event::Source>(
        &self,
        source: &mut S,
        interest: Interest,
    ) -> Result<(), TransportError> {
        self.poll
            .registry()
            .reregister(source, Token(usize::MAX), interest)
            .map_err(|_| TransportError::Io)
    }
    pub fn connection_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.connections.keys().copied()
    }
}

impl Transport {
    // Fixed key space and payload bound; pending frames coalesce before framing.
    fn queue_notice(c: &mut Connection, kind: u8, payload: &[u8]) -> bool {
        if kind >= 8 || payload.is_empty() || payload.len() > 4096 {
            return false;
        }
        c.notices.insert(kind, payload.to_vec());
        c.writable = true;
        true
    }
    /// Dedicated subscription sockets do not expire merely for being quiet.
    pub fn subscribe(&mut self, connection: u64) -> bool {
        let Some(c) = self.connections.get_mut(&connection) else {
            return false;
        };
        c.subscribed = true;
        true
    }
    /// Notice payloads are opaque to transport. At most eight kinds per peer,
    /// plus the one in-flight frame, never replace partially written bytes.
    pub fn notify(
        &mut self,
        connection: u64,
        kind: u8,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        if let Some(c) = self.connections.get_mut(&connection)
            && (!c.subscribed || !Self::queue_notice(c, kind, payload))
        {
            return Err(TransportError::InvalidResponse);
        }
        Ok(())
    }
}
