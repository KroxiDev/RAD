use crate::{Diagnostic, Error, OptionsOutcome, State, TARGET};
use rad_plugin_contract::*;
use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    os::unix::process::CommandExt,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};
use zeroize::Zeroize;
fn carries_jpeg(state: &VisualState) -> bool {
    matches!(state, VisualState::Current { content } | VisualState::Stale { content }
        if content.jpeg_hex.is_some())
}
/// A request awaiting its correlated reply. `residual`: the deadline passed and the
/// reply may still arrive; `delivered`: the caller already consumed that timeout.
pub(super) struct Pending<T = Outcome> {
    pub credential_revision: Option<u64>,
    pub deadline: u64,
    pub outcome: Option<T>,
    pub residual: bool,
    pub delivered: bool,
}
impl<T> Pending<T> {
    #[cfg(test)]
    pub fn new(deadline: u64) -> Self {
        Self::with_credentials(deadline, None)
    }
    pub fn with_credentials(deadline: u64, credential_revision: Option<u64>) -> Self {
        Self {
            deadline,
            credential_revision,
            outcome: None,
            residual: false,
            delivered: false,
        }
    }
}
pub(super) type Table<T = Outcome> = BTreeMap<u64, Pending<T>>;
/// Complete `request` with its reply. False when no reply is due: unknown request,
/// repeated reply, or one that missed its deadline without being a drained residual.
fn answer<T>(table: &mut Table<T>, request: u64, now: u64, value: T) -> bool {
    let Some(p) = table.get_mut(&request) else {
        return false;
    };
    if p.residual {
        // The late reply of an already reported timeout: drained, never reported again.
        p.residual = false;
        p.credential_revision = None;
        if p.delivered {
            table.remove(&request);
        }
        true
    } else if p.outcome.is_none() && now < p.deadline {
        p.credential_revision = None;
        p.outcome = Some(value);
        true
    } else {
        false
    }
}
/// Report every request past its deadline as `value`; its reply may still arrive later.
fn expire<T: Clone>(table: &mut Table<T>, now: u64, value: T) -> bool {
    let mut expired = false;
    for p in table.values_mut() {
        if p.outcome.is_none() && now >= p.deadline {
            p.outcome = Some(value.clone());
            p.residual = true;
            expired = true;
        }
    }
    expired
}
/// The process is gone, so no reply can arrive: finish what is waiting as `value`
/// and forget timeouts that were already reported.
fn settle<T: Clone>(table: &mut Table<T>, value: T) {
    table.retain(|_, p| !p.delivered);
    for p in table.values_mut() {
        p.residual = false;
        p.credential_revision = None;
        if p.outcome.is_none() {
            p.outcome = Some(value.clone());
        }
    }
}
/// Hand out one finished entry. A residual stays, marked delivered, until its late reply.
pub(super) fn take<T: Clone>(table: &mut Table<T>) -> Option<(u64, T)> {
    let request = table
        .iter()
        .find_map(|(r, p)| (!p.delivered && p.outcome.is_some()).then_some(*r))?;
    let p = table.get_mut(&request).expect("entry found above");
    if p.residual {
        p.delivered = true;
        p.outcome.clone().map(|outcome| (request, outcome))
    } else {
        table
            .remove(&request)?
            .outcome
            .map(|outcome| (request, outcome))
    }
}
pub(super) struct Visual {
    pub credential_revision: Option<u64>,
    pub state: VisualState,
    pub expires: u64,
    pub ttl: u64,
}
pub(super) struct Worker {
    pub credentials: BTreeMap<u64, crate::credentials::Installed>,
    pub last_revision: u64,
    pub manifest: Manifest,
    /// Host API agreed with this plugin; gates every API 2 feature in both directions.
    pub api: u32,
    pub generation: u64,
    pub state: State,
    pub stopped_normally: bool,
    pub child: Option<Child>,
    input: Option<ChildStdin>,
    output: Option<ChildStdout>,
    exit: Option<OwnedFd>,
    pub immediate: bool,
    reap_retry: Option<u64>,
    rx: Vec<u8>,
    tx: VecDeque<Vec<u8>>,
    offset: usize,
    next: u64,
    pub hello_deadline: u64,
    pub launch_pending: bool,
    pub pending: Table,
    pub queries: Table<OptionsOutcome>,
    pub visuals: BTreeMap<u64, Visual>,
    pub unsubscribes: VecDeque<Correlation>,
    pub diagnostics: VecDeque<Diagnostic>,
}
fn nonblocking(fd: i32) -> std::io::Result<()> {
    // SAFETY: borrowed live pipe fd; no ownership transfer or pointer arguments.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
fn configure_command(command: &mut Command, id: &str) {
    // Explicit caller values override inherited values, including explicit removals.
    let allowed: Vec<_> = ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"]
        .into_iter()
        .filter_map(|key| {
            let value = match command.get_envs().find(|(name, _)| *name == key) {
                Some((_, value)) => value.map(|v| v.to_owned()),
                None => std::env::var_os(key),
            };
            value.map(|value| (key, value))
        })
        .collect();
    command.env_clear().envs(allowed).arg0(id);
    // The runtime blocks its shutdown signals before spawning threads; a plugin
    // inheriting that mask could only be stopped with SIGKILL.
    // SAFETY: the post-fork closure uses only async-signal-safe libc signal
    // operations and constructs an OS error without allocation on failure.
    unsafe {
        command.pre_exec(|| {
            let mut signals = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut signals);
            libc::sigaddset(&mut signals, libc::SIGINT);
            libc::sigaddset(&mut signals, libc::SIGTERM);
            if libc::sigprocmask(libc::SIG_UNBLOCK, &signals, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
fn process_exit(pid: u32) -> Option<OwnedFd> {
    // SAFETY: caller retains this unreaped child; syscall returns a new owned fd.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        None
    } else {
        // SAFETY: successful syscall returned an exclusive valid descriptor.
        Some(unsafe { OwnedFd::from_raw_fd(fd as i32) })
    }
}
impl Worker {
    pub fn reserved(manifest: Manifest, generation: u64, deadline: u64) -> Self {
        Self {
            credentials: BTreeMap::new(),
            last_revision: 0,
            api: manifest.negotiated_api(HOST_API),
            manifest,
            generation,
            state: State::Starting,
            stopped_normally: false,
            child: None,
            input: None,
            output: None,
            exit: None,
            immediate: false,
            reap_retry: None,
            rx: Vec::new(),
            tx: VecDeque::new(),
            offset: 0,
            next: 1,
            hello_deadline: deadline,
            launch_pending: true,
            pending: BTreeMap::new(),
            queries: BTreeMap::new(),
            visuals: BTreeMap::new(),
            unsubscribes: VecDeque::new(),
            diagnostics: VecDeque::new(),
        }
    }
    pub fn spawn(
        command: Command,
        manifest: Manifest,
        generation: u64,
        deadline: u64,
    ) -> Result<Self, Error> {
        Self::spawn_with_exit(command, manifest, generation, deadline, process_exit)
    }
    fn spawn_with_exit(
        mut command: Command,
        manifest: Manifest,
        generation: u64,
        deadline: u64,
        open_exit: fn(u32) -> Option<OwnedFd>,
    ) -> Result<Self, Error> {
        configure_command(&mut command, &manifest.id);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Error::Io)?;
        let input = child.stdin.take().expect("configured stdin pipe");
        let output = child.stdout.take().expect("configured stdout pipe");
        let configured = nonblocking(input.as_raw_fd())
            .and_then(|_| nonblocking(output.as_raw_fd()))
            .is_ok();
        let exit = open_exit(child.id());
        let configured = configured && exit.is_some();
        let mut w = Self {
            credentials: BTreeMap::new(),
            last_revision: 0,
            api: manifest.negotiated_api(HOST_API),
            manifest,
            generation,
            state: State::Starting,
            stopped_normally: false,
            child: Some(child),
            input: Some(input),
            output: Some(output),
            exit,
            immediate: true,
            reap_retry: None,
            rx: Vec::new(),
            tx: VecDeque::new(),
            offset: 0,
            next: 1,
            hello_deadline: deadline,
            launch_pending: false,
            pending: BTreeMap::new(),
            queries: BTreeMap::new(),
            visuals: BTreeMap::new(),
            unsubscribes: VecDeque::new(),
            diagnostics: VecDeque::new(),
        };
        if !configured {
            w.fail(Diagnostic::Pipe);
            return Ok(w);
        }
        let hello = Message::Hello {
            host_api: w.api,
            target: TARGET.into(),
            plugin_id: w.manifest.id.clone(),
            plugin_version: w.manifest.version,
        };
        if w.send(hello).is_err() {
            w.fail(Diagnostic::Protocol);
        }
        Ok(w)
    }
    pub fn io_sources(&self) -> Vec<(BorrowedFd<'_>, bool)> {
        let mut sources = Vec::new();
        if let Some(output) = &self.output {
            sources.push((output.as_fd(), false));
        }
        if let Some(input) = &self.input {
            sources.push((input.as_fd(), true));
        }
        if let Some(exit) = &self.exit {
            sources.push((exit.as_fd(), false));
        }
        sources
    }
    pub fn next_deadline(&self, now: u64) -> Option<u64> {
        let immediate = self.immediate;
        immediate
            .then_some(now)
            .into_iter()
            .chain(self.reap_retry)
            .chain(
                (self.state == State::Ready)
                    .then(|| self.credential_deadline())
                    .flatten(),
            )
            .chain((self.state == State::Starting).then_some(self.hello_deadline))
            .chain(
                self.pending
                    .values()
                    .filter(|p| p.outcome.is_none())
                    .map(|p| p.deadline),
            )
            .chain(
                self.queries
                    .values()
                    .filter(|p| p.outcome.is_none())
                    .map(|p| p.deadline),
            )
            .chain(
                self.visuals
                    .values()
                    .filter(|v| matches!(v.state, VisualState::Current { .. }))
                    .map(|v| v.expires),
            )
            .min()
    }
    pub fn send(&mut self, message: Message) -> Result<Correlation, Error> {
        if self.tx.len() >= 32 {
            return Err(Error::Capacity);
        }
        let next = self.next.checked_add(1).ok_or(Error::Exhausted)?;
        let correlation = Correlation {
            generation: self.generation,
            request: self.next,
        };
        let bytes = encode(&Envelope {
            protocol: 1,
            correlation,
            message,
        })
        .map_err(|_| Error::Invalid)?;
        self.tx.push_back(bytes);
        self.immediate = true;
        self.next = next;
        Ok(correlation)
    }
    fn record(&mut self, why: Diagnostic) {
        if self.diagnostics.len() == 16 {
            self.diagnostics.pop_front();
        }
        self.diagnostics.push_back(why);
    }
    pub fn stop(&mut self) {
        if matches!(self.state, State::Failed | State::Reaped) {
            return;
        }
        self.stopped_normally = true;
        self.fail(Diagnostic::Exited);
    }
    pub fn idle(&self) -> bool {
        !self.launch_pending
            && self.pending.is_empty()
            && self.queries.is_empty()
            && self.visuals.is_empty()
            && self.unsubscribes.is_empty()
            && self.tx.is_empty()
            && self.credential_deadline().is_none()
    }
    pub fn fail(&mut self, why: Diagnostic) {
        if matches!(self.state, State::Failed | State::Reaped) {
            return;
        }
        self.state = State::Failed;
        if !self.stopped_normally {
            self.record(why);
        }
        self.input.take();
        self.output.take();
        for bytes in &mut self.tx {
            bytes.zeroize();
        }
        self.tx.clear();
        self.unsubscribes.clear();
        self.rx.zeroize();
        self.rx.clear();
        for v in self.visuals.values_mut() {
            v.state = VisualState::Unavailable;
        }
        // A list is not tied to execution state, so it does not wait for the reap.
        // Failure closes admission, but kill is not proof that credential users stopped.
        for p in self.queries.values_mut() {
            if p.outcome.is_none() {
                p.outcome = Some(OptionsOutcome::Choices(ChoiceList::Unavailable));
            }
            p.residual = true;
        }
        if let Some(child) = &mut self.child {
            let _ = child.kill();
        }
    }
    fn receive(&mut self, envelope: Envelope, now: u64) {
        if envelope.correlation.generation != self.generation {
            self.fail(Diagnostic::Protocol);
            return;
        }
        match envelope.message {
            Message::CredentialsApplied { revision } => {
                self.credential_ack(envelope.correlation, revision, false, now)
            }
            Message::CredentialsReleased { revision } => {
                self.credential_ack(envelope.correlation, revision, true, now)
            }
            Message::Ready {
                plugin_id,
                plugin_version,
            } if self.state == State::Starting
                && envelope.correlation.request == 1
                && plugin_id == self.manifest.id
                && plugin_version == self.manifest.version =>
            {
                self.state = State::Ready
            }
            Message::Result { outcome } if self.state == State::Ready => {
                if !answer(
                    &mut self.pending,
                    envelope.correlation.request,
                    now,
                    outcome,
                ) {
                    self.fail(Diagnostic::Protocol);
                }
            }
            // API 1 plugins never get a query, so any `choices` from them is unsolicited.
            Message::Choices { list } if self.state == State::Ready && self.api >= 2 => {
                let outcome = OptionsOutcome::Choices(list);
                if !answer(
                    &mut self.queries,
                    envelope.correlation.request,
                    now,
                    outcome,
                ) {
                    self.fail(Diagnostic::Protocol);
                }
            }
            // JPEG frames exist from API 2; an API 1 plugin sending one broke the contract.
            Message::Visual { state }
                if self.state == State::Ready && (self.api >= 2 || !carries_jpeg(&state)) =>
            {
                if let Some(v) = self.visuals.get_mut(&envelope.correlation.request) {
                    v.state = state;
                    v.expires = now.saturating_add(v.ttl);
                } // A queued visual after unsubscribe is harmless and never an action result.
            }
            _ => self.fail(Diagnostic::Protocol),
        }
    }
    pub fn pump(&mut self, now: u64) {
        self.immediate = false;
        if self.state == State::Starting && now >= self.hello_deadline {
            self.fail(Diagnostic::Deadline);
        }
        if self.state == State::Ready && self.credential_deadline().is_some_and(|d| now >= d) {
            self.fail(Diagnostic::Deadline);
        }
        let failed = Outcome::Failure {
            code: FailureCode::Failed,
        };
        if expire(&mut self.pending, now, failed) {
            self.record(Diagnostic::Deadline);
        }
        // A late list is harmless: unlike a late action it records nothing and never kills.
        expire(&mut self.queries, now, OptionsOutcome::TimedOut);
        if self.pending.values().filter(|p| p.residual).count() >= crate::MAX_PLUGIN_JOBS {
            self.fail(Diagnostic::Deadline);
        }
        for v in self.visuals.values_mut() {
            if now >= v.expires
                && let VisualState::Current { content } = &v.state
            {
                v.state = VisualState::Stale {
                    content: content.clone(),
                };
            }
        }
        while let Some(subscription) = self.unsubscribes.front().copied() {
            match self.send(Message::Unsubscribe { subscription }) {
                Ok(_) => {
                    self.unsubscribes.pop_front();
                }
                Err(Error::Capacity) => break,
                Err(_) => {
                    self.fail(Diagnostic::Protocol);
                    break;
                }
            }
        }
        let mut write_blocked = false;
        // Hard per-turn budgets; a flooding worker cannot own the loop.
        for _ in 0..8 {
            let Some(bytes) = self.tx.front() else {
                break;
            };
            let Some(input) = &mut self.input else {
                break;
            };
            match input.write(&bytes[self.offset..]) {
                Ok(0) => {
                    self.fail(Diagnostic::Pipe);
                    break;
                }
                Ok(n) => {
                    self.offset += n;
                    if self.offset == bytes.len() {
                        if let Some(mut bytes) = self.tx.pop_front() {
                            bytes.zeroize();
                        }
                        self.offset = 0;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    write_blocked = true;
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.fail(Diagnostic::Pipe);
                    break;
                }
            }
        }
        self.immediate |= !write_blocked && (!self.tx.is_empty() || !self.unsubscribes.is_empty());
        let mut read_continuation = false;
        for _ in 0..16 {
            read_continuation = false;
            match decode(&self.rx) {
                Ok(Some((message, used))) => {
                    self.rx[..used].zeroize();
                    self.rx.drain(..used);
                    self.receive(message, now);
                    read_continuation = true;
                    continue;
                }
                Err(_) => {
                    self.fail(Diagnostic::Protocol);
                    break;
                }
                Ok(None) => {}
            }
            if self.rx.len() >= 65540 {
                self.fail(Diagnostic::Backpressure);
                break;
            }
            let Some(output) = &mut self.output else {
                break;
            };
            let mut buffer = [0; 4096];
            let remaining = (65540 - self.rx.len()).min(buffer.len());
            match output.read(&mut buffer[..remaining]) {
                Ok(0) => {
                    self.fail(Diagnostic::Exited);
                    break;
                }
                Ok(n) => {
                    self.rx.extend_from_slice(&buffer[..n]);
                    buffer.zeroize();
                    read_continuation = true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.fail(Diagnostic::Pipe);
                    break;
                }
            }
        }
        self.immediate |= read_continuation;
        // Only unavailable pidfd monitoring uses a bounded exceptional reap retry.
        self.reap_retry =
            (self.child.is_some() && self.exit.is_none()).then_some(now.saturating_add(100));
        if let Some(child) = &mut self.child
            && matches!(child.try_wait(), Ok(Some(_)))
        {
            self.fail(Diagnostic::Exited);
            self.child.take();
            self.exit.take();
            self.immediate = false;
            self.reap_retry = None;
            self.state = State::Reaped;
            self.credentials.clear();
            for v in self.visuals.values_mut() {
                v.credential_revision = None;
            }
            settle(
                &mut self.queries,
                OptionsOutcome::Choices(ChoiceList::Unavailable),
            );
            settle(
                &mut self.pending,
                Outcome::Failure {
                    code: FailureCode::Failed,
                },
            );
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        for bytes in &mut self.tx {
            bytes.zeroize();
        }
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.try_wait();
        }
    }
}

#[cfg(test)]
mod exit_watch_tests {
    use super::*;
    #[test]
    fn missing_pidfd_fails_closed_and_retries_reap_without_busy_spin() {
        let manifest =
            serde_json::from_slice(include_bytes!("../fixture/manifest-v1.json")).unwrap();
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        let mut worker = Worker::spawn_with_exit(command, manifest, 1, 5000, |_| None).unwrap();
        assert_eq!(worker.state, State::Failed);
        assert!(worker.child.is_some(), "kill is not reap");
        assert_eq!(worker.diagnostics.back(), Some(&Diagnostic::Pipe));
        // Simulate the scheduled retry epoch before polling; killed child remains owned.
        worker.immediate = false;
        worker.reap_retry = Some(100);
        assert_eq!(worker.next_deadline(0), Some(100));
        let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut now = 100;
        while worker.child.is_some() {
            worker.pump(now);
            assert!(std::time::Instant::now() < end);
            if worker.child.is_some() {
                assert_eq!(worker.next_deadline(now), Some(now + 100));
                now += 100;
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        assert_eq!(worker.next_deadline(now), None);
    }
}

#[cfg(test)]
mod negotiation_tests {
    use super::*;

    fn spawn(host_min: u32, host_max: u32) -> Worker {
        let mut manifest: Manifest =
            serde_json::from_slice(include_bytes!("../fixture/manifest-v1.json")).unwrap();
        (manifest.host_min, manifest.host_max) = (host_min, host_max);
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        Worker::spawn(command, manifest, 1, 5000).unwrap()
    }

    #[test]
    fn hello_carries_the_api_negotiated_with_the_manifest_not_the_newest() {
        for (host_min, host_max, negotiated) in [(1, 1, 1), (1, 2, 2), (2, 2, 2), (1, 9, 3)] {
            let worker = spawn(host_min, host_max);
            let hello = worker.tx.front().expect("Hello is queued at spawn");
            let Ok(Some((envelope, _))) = decode(hello) else {
                panic!("queued frame decodes");
            };
            assert!(
                matches!(envelope.message, Message::Hello { host_api, .. } if host_api == negotiated),
                "range {host_min}..={host_max}: {:?}",
                envelope.message
            );
        }
    }
}

#[cfg(test)]
mod options_protocol_tests {
    use super::*;

    fn waiting(api: u32) -> Worker {
        let mut manifest: Manifest =
            serde_json::from_slice(include_bytes!("../fixture/manifest-v1.json")).unwrap();
        (manifest.host_min, manifest.host_max) = (api, api);
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        let mut worker = Worker::spawn(command, manifest, 1, 5000).unwrap();
        worker.state = State::Ready;
        worker.queries.insert(5, Pending::new(100));
        worker.pending.insert(6, Pending::new(100));
        worker
    }
    fn receive(worker: &mut Worker, request: u64, message: Message, now: u64) {
        let correlation = Correlation {
            generation: 1,
            request,
        };
        worker.receive(
            Envelope {
                protocol: 1,
                correlation,
                message,
            },
            now,
        );
    }
    fn list() -> ChoiceList {
        ChoiceList::Available {
            options: vec![ChoiceOption {
                value: "a".into(),
                label: "A".into(),
            }],
        }
    }
    fn jpeg() -> Message {
        Message::Visual {
            state: VisualState::Current {
                content: VisualContent {
                    text: None,
                    png_hex: None,
                    jpeg_hex: Some("ffd8ffd9".into()),
                    color: None,
                },
            },
        }
    }

    #[test]
    fn replies_that_match_no_waiting_query_are_protocol_violations() {
        let outcome = Outcome::Success;
        let options = Message::Options {
            credential_revision: None,
            scope: OptionsScope::Action,
            definition: "pick".into(),
            field: "group".into(),
            context: Parameters {
                schema_version: 1,
                values: serde_json::Map::new(),
            },
        };
        for (case, request, message) in [
            ("unknown correlation", 9, Message::Choices { list: list() }),
            (
                "invocation correlation",
                6,
                Message::Choices { list: list() },
            ),
            ("result to a query", 5, Message::Result { outcome }),
            ("host message sent by the plugin", 5, options),
        ] {
            let mut worker = waiting(2);
            receive(&mut worker, request, message, 0);
            assert_eq!(worker.state, State::Failed, "{case}");
        }
    }

    #[test]
    fn a_query_is_answered_once_and_a_late_reply_is_drained_once() {
        let mut worker = waiting(2);
        receive(&mut worker, 5, Message::Choices { list: list() }, 0);
        assert_eq!(worker.state, State::Ready);
        assert!(
            worker.pending[&6].outcome.is_none(),
            "the invocation is untouched"
        );
        receive(&mut worker, 5, Message::Choices { list: list() }, 0);
        assert_eq!(worker.state, State::Failed, "second answer");

        let mut worker = waiting(2);
        worker.pump(100); // past the deadline: reported as a timeout, reply still due
        assert_eq!(
            take(&mut worker.queries),
            Some((5, OptionsOutcome::TimedOut))
        );
        receive(&mut worker, 5, Message::Choices { list: list() }, 100);
        assert_eq!(worker.state, State::Ready, "the late reply is drained");
        assert!(worker.queries.is_empty() && take(&mut worker.queries).is_none());
        receive(&mut worker, 5, Message::Choices { list: list() }, 100);
        assert_eq!(worker.state, State::Failed, "drained replies do not repeat");
    }

    #[test]
    fn a_late_reply_drained_before_the_timeout_is_taken_leaves_the_timeout_to_report() {
        let mut worker = waiting(2);
        worker.pump(100); // expired, not yet taken by the caller
        receive(&mut worker, 5, Message::Choices { list: list() }, 100);
        assert_eq!(worker.state, State::Ready, "the late reply is drained");
        assert_eq!(
            take(&mut worker.queries),
            Some((5, OptionsOutcome::TimedOut)),
            "the timeout is still reported"
        );
        assert!(take(&mut worker.queries).is_none() && worker.queries.is_empty());
    }

    #[test]
    fn choices_and_jpeg_belong_to_api_two_only() {
        let mut old = waiting(1);
        receive(&mut old, 5, Message::Choices { list: list() }, 0);
        assert_eq!(old.state, State::Failed, "choices at API 1");
        let mut old = waiting(1);
        receive(&mut old, 7, jpeg(), 0);
        assert_eq!(old.state, State::Failed, "jpeg at API 1");

        let mut new = waiting(2);
        new.visuals.insert(
            7,
            Visual {
                credential_revision: None,
                state: VisualState::Unavailable,
                expires: 1000,
                ttl: 1000,
            },
        );
        receive(&mut new, 7, jpeg(), 0);
        assert_eq!(new.state, State::Ready, "jpeg at API 2");
        assert!(matches!(new.visuals[&7].state, VisualState::Current { .. }));
    }
}

#[cfg(test)]
mod credential_tests {
    use super::*;
    use crate::{CredentialState, Host};
    fn ready() -> Host {
        let mut manifest: Manifest =
            serde_json::from_slice(include_bytes!("../fixture/manifest-v1.json")).unwrap();
        manifest.host_min = 3;
        manifest.host_max = 3;
        manifest.actions[0].credential_slots = vec!["password".into()];
        let mut w = Worker::reserved(manifest.clone(), 1, 5000);
        w.state = State::Ready;
        w.next = 2;
        let mut host = Host::default();
        host.workers.insert(manifest.id, w);
        host
    }
    fn values(value: &str) -> CredentialValues {
        CredentialValues::new(vec![CredentialValue {
            slot: "password".into(),
            value: value.into(),
        }])
        .unwrap()
    }
    fn reply(host: &mut Host, c: Correlation, message: Message, now: u64) {
        host.workers.values_mut().next().unwrap().receive(
            Envelope {
                protocol: 1,
                correlation: c,
                message,
            },
            now,
        );
    }
    #[test]
    fn credentials_need_correlated_ack_before_use_and_release() {
        let mut host = ready();
        let id = host.workers.keys().next().unwrap().clone();
        let c = host
            .install_credentials(&id, 1, 1, values("CANARY"))
            .unwrap();
        assert_eq!(
            host.credentials_state(&id, 1, 1),
            Some(CredentialState::Installing)
        );
        assert!(host.workers[&id].check_credentials(Some(1)).is_err());
        reply(&mut host, c, Message::CredentialsApplied { revision: 1 }, 0);
        assert!(host.workers[&id].check_credentials(Some(1)).is_ok());
        assert!(host.workers[&id].check_credentials(None).is_err());
        let c = host.release_credentials(&id, 1, 1).unwrap();
        assert!(host.workers[&id].check_credentials(Some(1)).is_err());
        reply(
            &mut host,
            c,
            Message::CredentialsReleased { revision: 1 },
            1,
        );
        assert_eq!(host.take_credentials_released(), Some((id.clone(), 1, 1)));
        assert_eq!(host.credentials_state(&id, 1, 1), None);
        assert!(
            host.install_credentials(&id, 1, 1, values("again"))
                .is_err()
        );
    }
    #[test]
    fn unknown_duplicate_wrong_revision_direction_and_deadline_acks_fail_closed() {
        for kind in 0..6 {
            let mut host = ready();
            let id = host.workers.keys().next().unwrap().clone();
            let c = host
                .install_credentials(&id, 1, 1, values("CANARY"))
                .unwrap();
            match kind {
                0 => reply(
                    &mut host,
                    Correlation {
                        request: c.request + 1,
                        ..c
                    },
                    Message::CredentialsApplied { revision: 1 },
                    0,
                ),
                1 => reply(&mut host, c, Message::CredentialsApplied { revision: 2 }, 0),
                2 => reply(
                    &mut host,
                    c,
                    Message::CredentialsReleased { revision: 1 },
                    0,
                ),
                3 => reply(
                    &mut host,
                    c,
                    Message::Credentials {
                        revision: 1,
                        values: vec![],
                    },
                    0,
                ),
                4 => reply(
                    &mut host,
                    c,
                    Message::CredentialsApplied { revision: 1 },
                    2000,
                ),
                _ => {
                    reply(&mut host, c, Message::CredentialsApplied { revision: 1 }, 0);
                    reply(&mut host, c, Message::CredentialsApplied { revision: 1 }, 1);
                }
            }
            assert_eq!(host.state(&id), Some(State::Failed), "case {kind}");
            assert!(
                host.workers[&id].credentials[&1].values.is_some(),
                "failed is not reaped"
            );
            assert!(!format!("{:?}", host.diagnostics(&id)).contains("CANARY"));
        }
    }
    #[test]
    fn logical_timeouts_retain_credentials_until_the_real_late_reply() {
        let mut host = ready();
        let id = host.workers.keys().next().unwrap().clone();
        let c = host
            .install_credentials(&id, 1, 1, values("CANARY"))
            .unwrap();
        reply(&mut host, c, Message::CredentialsApplied { revision: 1 }, 0);
        let w = host.workers.get_mut(&id).unwrap();
        w.pending
            .insert(100, Pending::with_credentials(10, Some(1)));
        w.queries
            .insert(101, Pending::with_credentials(10, Some(1)));
        host.pump(10).unwrap();
        host.take_completion();
        host.take_options();
        assert_eq!(host.release_credentials(&id, 1, 1), Err(Error::Residual));
        reply(
            &mut host,
            Correlation {
                generation: 1,
                request: 100,
            },
            Message::Result {
                outcome: Outcome::Success,
            },
            11,
        );
        assert_eq!(host.release_credentials(&id, 1, 1), Err(Error::Residual));
        reply(
            &mut host,
            Correlation {
                generation: 1,
                request: 101,
            },
            Message::Choices {
                list: ChoiceList::Unavailable,
            },
            11,
        );
        assert!(host.release_credentials(&id, 1, 1).is_ok());
    }
    #[test]
    fn install_and_release_deadlines_are_scheduled_and_revision_storage_is_bounded() {
        let mut host = ready();
        let id = host.workers.keys().next().unwrap().clone();
        let c = host
            .install_credentials(&id, 1, 1, values("CANARY"))
            .unwrap();
        host.workers.get_mut(&id).unwrap().tx.clear();
        host.workers.get_mut(&id).unwrap().immediate = false;
        assert_eq!(host.next_poll(), Some(2000));
        host.pump(2000).unwrap();
        assert_eq!(host.state(&id), Some(State::Failed));
        reply(
            &mut host,
            c,
            Message::CredentialsApplied { revision: 1 },
            2001,
        );
        assert_eq!(host.state(&id), Some(State::Failed));
        let mut host = ready();
        let id = host.workers.keys().next().unwrap().clone();
        for r in 1..=200 {
            let c = host
                .install_credentials(&id, 1, r, values("canary"))
                .unwrap();
            reply(&mut host, c, Message::CredentialsApplied { revision: r }, 0);
            let c = host.release_credentials(&id, 1, r).unwrap();
            reply(
                &mut host,
                c,
                Message::CredentialsReleased { revision: r },
                0,
            );
            assert!(host.take_credentials_released().is_some());
            host.workers.get_mut(&id).unwrap().tx.clear();
        }
        for r in 201..=267 {
            let c = host
                .install_credentials(&id, 1, r, values("canary"))
                .unwrap();
            reply(&mut host, c, Message::CredentialsApplied { revision: r }, 0);
            host.workers.get_mut(&id).unwrap().tx.clear();
        }
        assert_eq!(
            host.install_credentials(&id, 1, 268, values("canary")),
            Err(Error::Capacity)
        );
        let c = host.release_credentials(&id, 1, 201).unwrap();
        host.workers.get_mut(&id).unwrap().tx.clear();
        host.workers.get_mut(&id).unwrap().immediate = false;
        assert_eq!(host.next_poll(), Some(2000));
        host.pump(2000).unwrap();
        assert_eq!(host.state(&id), Some(State::Failed));
        reply(
            &mut host,
            c,
            Message::CredentialsReleased { revision: 201 },
            2001,
        );
        assert!(host.take_credentials_released().is_none());
    }
    #[test]
    fn options_occupancy_matches_admission_through_results_timeouts_and_late_replies() {
        let mut host = ready();
        let id = host.workers.keys().next().unwrap().clone();
        let c = host
            .install_credentials(&id, 1, 1, values("correct"))
            .unwrap();
        reply(&mut host, c, Message::CredentialsApplied { revision: 1 }, 0);
        let action = host.workers[&id].manifest.actions[0].id.clone();
        host.workers.get_mut(&id).unwrap().manifest.actions[0]
            .schema
            .fields
            .push(Field {
                id: "pick".into(),
                label: "Pick".into(),
                required: false,
                kind: FieldKind::DynamicChoice { depends_on: vec![] },
            });
        let query = crate::OptionsQuery {
            scope: OptionsScope::Action,
            definition: action,
            field: "pick".into(),
            context: Parameters {
                schema_version: 1,
                values: Default::default(),
            },
        };
        assert_eq!(host.options_in_use(&id, 1), 0);
        let first = host
            .options_with_credentials(&id, 1, query.clone(), 10, Some(1))
            .unwrap();
        let second = host
            .options_with_credentials(&id, 1, query.clone(), 10, Some(1))
            .unwrap();
        assert_eq!(host.options_in_use(&id, 1), 2);
        assert_eq!(host.options_in_use(&id, 2), 0);
        assert_eq!(host.options_in_use("missing", 1), 0);
        host.pump(10).unwrap();
        assert!(matches!(
            host.take_options(),
            Some((_, crate::OptionsOutcome::TimedOut))
        ));
        assert!(matches!(
            host.take_options(),
            Some((_, crate::OptionsOutcome::TimedOut))
        ));
        assert_eq!(
            host.options_in_use(&id, 1),
            2,
            "timeouts are still residual"
        );
        assert_eq!(
            host.options_with_credentials(&id, 1, query.clone(), 20, Some(1)),
            Err(Error::Capacity)
        );
        reply(
            &mut host,
            first,
            Message::Choices {
                list: ChoiceList::Unavailable,
            },
            11,
        );
        assert_eq!(
            host.options_in_use(&id, 1),
            1,
            "late reply drains one real slot"
        );
        let third = host
            .options_with_credentials(&id, 1, query, 20, Some(1))
            .unwrap();
        assert_eq!(host.options_in_use(&id, 1), 2);
        reply(
            &mut host,
            third,
            Message::Choices {
                list: ChoiceList::Unavailable,
            },
            12,
        );
        assert_eq!(
            host.options_in_use(&id, 1),
            2,
            "completed but not consumed still occupies a slot"
        );
        assert_eq!(
            host.take_options(),
            Some((
                third,
                crate::OptionsOutcome::Choices(ChoiceList::Unavailable)
            ))
        );
        assert_eq!(host.options_in_use(&id, 1), 1);
        reply(
            &mut host,
            second,
            Message::Choices {
                list: ChoiceList::Unavailable,
            },
            12,
        );
        assert_eq!(host.options_in_use(&id, 1), 0);
    }
}
