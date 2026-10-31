use crate::{Diagnostic, Error, OptionsOutcome, State, TARGET};
use rad_plugin_contract::*;
use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    os::unix::process::CommandExt,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
};
use zeroize::Zeroize;
fn carries_jpeg(state: &VisualState) -> bool {
    matches!(state, VisualState::Current { content } | VisualState::Stale { content }
        if content.jpeg_hex.is_some())
}
/// A request awaiting its correlated reply. `residual`: the deadline passed and the
/// reply may still arrive; `delivered`: the caller already consumed that timeout.
pub(super) struct Pending<T = Outcome> {
    pub reserved_end: bool,
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
            reserved_end: false,
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
/// Report every request past its deadline as `value`, once. The request stays as a
/// residual (counted, holding its credential revision) until its late reply is drained
/// or `settle` runs; neither reports it again.
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
    pub observed_current: bool,
    pub received_state: bool,
    pub renewals: u64,
    pub credential_revision: Option<u64>,
    pub state: VisualState,
    pub expires: u64,
    pub ttl: u64,
}
pub(super) struct Worker {
    pub holds: BTreeMap<u64, crate::holds::Held>,
    pub credentials: BTreeMap<u64, crate::credentials::Installed>,
    pub last_revision: u64,
    pub manifest: Manifest,
    /// Host API agreed with this plugin; gates every API 2 feature in both directions.
    pub api: u32,
    pub capabilities: Vec<String>,
    pub generation: u64,
    pub state: State,
    pub stopped_normally: bool,
    pub child: Option<Child>,
    input: Option<ChildStdin>,
    output: Option<ChildStdout>,
    error_output: Option<ChildStderr>,
    pub stderr: crate::stderr::Stderr,
    pub protocol_credential_leak: bool,
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
    // The plugin leads its own process group (pgid == pid), so the Host can kill
    // everything the plugin started with one `kill(-pid)`.
    command.env_clear().envs(allowed).arg0(id).process_group(0);
    let parent = std::process::id();
    // The runtime blocks its shutdown signals before spawning threads; a plugin
    // inheriting that mask could only be stopped with SIGKILL.
    // SAFETY: the post-fork closure uses only async-signal-safe libc operations
    // (sigprocmask, prctl, getppid) and constructs an OS error without allocation
    // on failure.
    unsafe {
        command.pre_exec(move || {
            // Linux death signal, a backstop for a runtime that dies without cleanup.
            // It fires when the *thread* that spawned the plugin exits, and it is not
            // inherited by forks, so group kill remains the primary mechanism. The
            // parent check closes the race where the runtime died before the prctl.
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() as u32 != parent {
                return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
            }
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
/// Whether the plugin process (the group leader) has terminated, reaped or not.
/// An unexpected `waitid` error counts as exited so the caller falls back to `try_wait`.
fn leader_exited(child: &Child) -> bool {
    // SAFETY: zeroed siginfo_t is a valid out-parameter; WNOWAIT leaves the child unreaped.
    unsafe {
        let mut info = std::mem::zeroed::<libc::siginfo_t>();
        let r = libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        );
        r != 0 || info.si_pid() != 0
    }
}
/// SIGKILL the plugin's whole process group, then the leader itself (which covers a
/// leader that moved to another group). The group is only signalled while the leader
/// is unreaped: a zombie leader's pid cannot be reused, so `-pid` cannot hit a stranger.
/// A descendant that left the group (setsid/setpgid) is out of reach until the service
/// stops and systemd empties the cgroup (ADR 0013); the Host neither adopts nor reaps
/// grandchildren, whoever inherits them does.
fn kill_group(child: &mut Child) {
    // SAFETY: zeroed siginfo_t is a valid out-parameter; WNOWAIT leaves the child unreaped.
    let unreaped = unsafe {
        let mut info = std::mem::zeroed::<libc::siginfo_t>();
        libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        ) == 0
    };
    if unreaped {
        // SAFETY: plain signal to the group this Host created for the child.
        unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
    }
    let _ = child.kill();
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
            holds: BTreeMap::new(),
            credentials: BTreeMap::new(),
            last_revision: 0,
            capabilities: negotiate_capabilities(
                &manifest.capabilities,
                &SUPPORTED_CAPABILITIES
                    .iter()
                    .map(|s| (*s).into())
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_default(),
            api: manifest.negotiated_api(HOST_API),
            manifest,
            generation,
            state: State::Starting,
            stopped_normally: false,
            child: None,
            input: None,
            output: None,
            error_output: None,
            stderr: Default::default(),
            protocol_credential_leak: false,
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
    /// Spawns the plugin as leader of its own process group, with `PR_SET_PDEATHSIG`
    /// SIGKILL. The death signal fires when the *calling thread* exits, so call this
    /// from a thread that lives as long as the plugins it launches.
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
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| Error::Io)?;
        let input = child.stdin.take().expect("configured stdin pipe");
        let output = child.stdout.take().expect("configured stdout pipe");
        let error_output = child.stderr.take().expect("configured stderr pipe");
        // Only the owned runtime/child are inspected. Prefer the plugin under OOM pressure.
        let oom = std::fs::read_to_string("/proc/self/oom_score_adj")
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .unwrap_or(0);
        let oom_configured = std::fs::write(
            format!("/proc/{}/oom_score_adj", child.id()),
            (oom + 100).clamp(500, 1000).to_string(),
        )
        .is_ok();
        let configured = nonblocking(input.as_raw_fd())
            .and_then(|_| nonblocking(output.as_raw_fd()))
            .and_then(|_| nonblocking(error_output.as_raw_fd()))
            .is_ok();
        let exit = open_exit(child.id());
        let configured = configured && exit.is_some() && oom_configured;
        let mut w = Self {
            holds: BTreeMap::new(),
            credentials: BTreeMap::new(),
            last_revision: 0,
            capabilities: negotiate_capabilities(
                &manifest.capabilities,
                &SUPPORTED_CAPABILITIES
                    .iter()
                    .map(|s| (*s).into())
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_default(),
            api: manifest.negotiated_api(HOST_API),
            manifest,
            generation,
            state: State::Starting,
            stopped_normally: false,
            child: Some(child),
            input: Some(input),
            output: Some(output),
            error_output: Some(error_output),
            stderr: Default::default(),
            protocol_credential_leak: false,
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
            capabilities: w.capabilities.clone(),
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
        if let Some(stderr) = &self.error_output {
            sources.push((stderr.as_fd(), false));
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
        self.send_with_limit(message, 32)
    }
    pub fn send_reserved_end(&mut self, message: Message) -> Result<Correlation, Error> {
        self.send_with_limit(message, 32 + crate::MAX_PLUGIN_HOLDS)
    }
    fn send_with_limit(&mut self, message: Message, limit: usize) -> Result<Correlation, Error> {
        if !message.permitted(&self.capabilities) {
            return Err(Error::Invalid);
        }
        if self.tx.len() >= limit {
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
            && self.holds.is_empty()
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
        self.error_output.take();
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
            kill_group(child);
        }
    }
    fn receive(&mut self, envelope: Envelope, now: u64) {
        // Latch before projection/coalescing: a later clean visual must not hide a leak.
        if !self.protocol_credential_leak
            && !self.credentials.is_empty()
            && let Ok(json) = serde_json::to_string(&envelope)
        {
            let json = zeroize::Zeroizing::new(json);
            self.protocol_credential_leak = self
                .credentials
                .values()
                .filter_map(|c| c.values.as_ref())
                .flat_map(|c| c.as_slice())
                .filter(|v| !v.value.is_empty())
                .any(|v| {
                    let encoded = zeroize::Zeroizing::new(
                        serde_json::to_string(&v.value).expect("string serialization"),
                    );
                    json.contains(&encoded[1..encoded.len() - 1])
                });
        }

        if envelope.correlation.generation != self.generation
            || !envelope.message.permitted(&self.capabilities)
        {
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
                capabilities,
            } if self.state == State::Starting
                && envelope.correlation.request == 1
                && plugin_id == self.manifest.id
                && plugin_version == self.manifest.version
                && capabilities == self.capabilities =>
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
            Message::Choices { list }
                if self.state == State::Ready
                    && rad_plugin_contract::supports(
                        &self.capabilities,
                        rad_plugin_contract::DYNAMIC_OPTIONS,
                    ) =>
            {
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
            Message::Visual {
                state,
                valid_for_ms,
            } if self.state == State::Ready
                && supports(&self.capabilities, VISUAL_FRESHNESS)
                && (rad_plugin_contract::supports(
                    &self.capabilities,
                    rad_plugin_contract::JPEG,
                ) || !carries_jpeg(&state)) =>
            {
                if let Some(v) = self.visuals.get_mut(&envelope.correlation.request) {
                    v.received_state = true;
                    v.observed_current = matches!(state, VisualState::Current { .. });
                    v.state = state;
                    v.expires = now.saturating_add(valid_for_ms);
                    v.ttl = valid_for_ms;
                } // A queued visual after unsubscribe is harmless and never an action result.
            }
            Message::VisualRenew { valid_for_ms }
                if self.state == State::Ready && supports(&self.capabilities, VISUAL_FRESHNESS) =>
            {
                if let Some(v) = self.visuals.get_mut(&envelope.correlation.request) {
                    if !v.received_state {
                        self.fail(Diagnostic::Protocol);
                        return;
                    }
                    v.renewals = v.renewals.saturating_add(1);
                    if v.observed_current
                        && let VisualState::Stale { content } = &v.state
                    {
                        v.state = VisualState::Current {
                            content: content.clone(),
                        };
                    }
                    v.expires = now.saturating_add(valid_for_ms);
                    v.ttl = valid_for_ms;
                }
            }
            _ => self.fail(Diagnostic::Protocol),
        }
    }
    pub fn pump(&mut self, now: u64) {
        self.immediate = false;
        for _ in 0..4 {
            let Some(output) = &mut self.error_output else {
                break;
            };
            let mut bytes = [0u8; 4096];
            match output.read(&mut bytes) {
                Ok(0) => {
                    self.error_output.take();
                    break;
                }
                Ok(n) => {
                    let secrets: Vec<_> = self
                        .credentials
                        .values()
                        .filter_map(|c| c.values.as_ref())
                        .flat_map(|c| c.as_slice().iter().map(|v| v.value.as_str()))
                        .collect();
                    for line in self.stderr.ingest(&bytes[..n], now, &secrets) {
                        crate::stderr::journal(&self.manifest.id, &line);
                    }
                    bytes.zeroize();
                    self.immediate = true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.error_output.take();
                    break;
                }
            }
        }
        if self.state == State::Starting && now >= self.hello_deadline {
            self.fail(Diagnostic::Deadline);
        }
        if self.state == State::Ready && self.credential_deadline().is_some_and(|d| now >= d) {
            self.fail(Diagnostic::Deadline);
        }
        let timed_out = Outcome::Failure {
            code: FailureCode::TimedOut,
        };
        if expire(&mut self.pending, now, timed_out) {
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
        // Kill the group before reaping the leader: while it is a zombie its pid is not
        // reusable, so what the plugin left behind dies even if it exited by itself.
        if let Some(child) = &mut self.child
            && leader_exited(child)
            && {
                kill_group(child);
                matches!(child.try_wait(), Ok(Some(_)))
            }
        {
            self.fail(Diagnostic::Exited);
            self.child.take();
            self.exit.take();
            self.immediate = false;
            for _ in 0..4 {
                let Some(output) = &mut self.error_output else {
                    break;
                };
                let mut bytes = [0u8; 4096];
                match output.read(&mut bytes) {
                    Ok(0) => {
                        self.error_output.take();
                        break;
                    }
                    Ok(n) => {
                        let secrets: Vec<_> = self
                            .credentials
                            .values()
                            .filter_map(|c| c.values.as_ref())
                            .flat_map(|c| c.as_slice().iter().map(|v| v.value.as_str()))
                            .collect();
                        for line in self.stderr.ingest(&bytes[..n], now, &secrets) {
                            crate::stderr::journal(&self.manifest.id, &line);
                        }
                        bytes.zeroize();
                        self.immediate = true;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => {
                        self.error_output.take();
                        break;
                    }
                }
            }
            self.reap_retry = None;
            self.state = State::Reaped;
            self.holds.clear();
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
            kill_group(child);
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
        for (host_min, host_max, negotiated) in [(1, 1, 1), (1, 2, 1), (1, 9, 1)] {
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

    fn waiting(extended: bool) -> Worker {
        let mut manifest: Manifest =
            serde_json::from_slice(include_bytes!("../fixture/manifest-v1.json")).unwrap();
        if extended {
            manifest
                .capabilities
                .extend(["dynamic_options".into(), "jpeg".into()]);
        }
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
            valid_for_ms: 1000,
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
    fn choices_and_jpeg_require_negotiated_capabilities() {
        let mut old = waiting(false);
        receive(&mut old, 5, Message::Choices { list: list() }, 0);
        assert_eq!(old.state, State::Failed, "choices without capability");
        let mut old = waiting(false);
        receive(&mut old, 7, jpeg(), 0);
        assert_eq!(old.state, State::Failed, "jpeg without capability");

        let mut new = waiting(true);
        new.visuals.insert(
            7,
            Visual {
                observed_current: false,
                received_state: false,
                renewals: 0,
                credential_revision: None,
                state: VisualState::Unavailable,
                expires: 1000,
                ttl: 1000,
            },
        );
        receive(&mut new, 7, jpeg(), 0);
        assert_eq!(new.state, State::Ready, "jpeg with capability");
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
        manifest
            .capabilities
            .extend(["credentials".into(), "dynamic_options".into()]);
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
        assert_eq!(
            host.take_completion(),
            Some((
                Correlation {
                    generation: 1,
                    request: 100
                },
                Outcome::Failure {
                    code: FailureCode::TimedOut
                }
            ))
        );
        assert!(host.take_completion().is_none());
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
        assert!(host.take_completion().is_none(), "drained, not reported");
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
    fn reaping_forgets_a_reported_timeout_and_frees_its_credential_revision() {
        let mut table: Table = Table::new();
        table.insert(1, Pending::with_credentials(10, Some(1)));
        assert!(expire(&mut table, 10, Outcome::Success));
        assert_eq!(take(&mut table), Some((1, Outcome::Success)));
        assert!(table[&1].residual && table[&1].credential_revision == Some(1));
        settle(
            &mut table,
            Outcome::Failure {
                code: FailureCode::Failed,
            },
        );
        assert!(table.is_empty(), "no second report, nothing retained");
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
