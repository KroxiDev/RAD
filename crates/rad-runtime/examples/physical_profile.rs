//! Explicitly opted-in XL instrument. --dry-run NEVER opens hardware.
//! Report dequeue -> expected Desired publication is NOT HID/contact/pixel latency.
use rad_client::connection::{Client, Endpoint};
use rad_deck::{Deck, DeckError, Desired, Event, KeyVisual, TextPlacement};
use rad_runtime::{display::DeckPort, lifecycle::ShutdownSignals, server::Server};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    num::NonZeroU128,
    os::{fd::AsRawFd, unix::fs::DirBuilderExt, unix::net::UnixStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;
const BRIGHTNESS: u8 = 25;
const NAVIGATION_SAMPLES: usize = 12;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn emit(value: Value) -> Result<()> {
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &value).map_err(|e| e.to_string())?;
    writeln!(output)
        .and_then(|_| output.flush())
        .map_err(|e| e.to_string())
}
fn notify(socket: &mut UnixStream) -> Result<()> {
    match socket.write(&[1]) {
        Ok(1) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
        other => Err(format!("instrument wake failed: {other:?}")),
    }
}
fn drain(socket: &mut UnixStream) -> Result<()> {
    let mut bytes = [0; 128];
    loop {
        match socket.read(&mut bytes) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct View {
    child: bool,
    asleep: bool,
}
struct Expected(Vec<(View, Desired)>);
impl Expected {
    fn new() -> Self {
        let mut values = Vec::new();
        for child in [false, true] {
            for asleep in [false, true] {
                let mut keys = std::array::from_fn(|_| KeyVisual::default());
                keys[0] =
                    KeyVisual::new(if child { "VOLVER" } else { "ENTRAR" }, None, false, false)
                        .text_at(TextPlacement::Middle);
                keys[2] = KeyVisual::new("", None, false, false)
                    .sleep_icon(true)
                    .text_at(TextPlacement::Bottom);
                values.push((
                    View { child, asleep },
                    Desired::new(keys, BRIGHTNESS, asleep).unwrap(),
                ));
            }
        }
        Self(values)
    }
    fn view(&self, desired: &Desired) -> Option<View> {
        self.0
            .iter()
            .find_map(|(view, value)| (value == desired).then_some(*view))
    }
    fn initial(&self) -> Desired {
        self.0[0].1.clone()
    }
}
struct Pending {
    at: Instant,
    expected: View,
    operation: &'static str,
    key: usize,
    ambiguous: bool,
}
#[derive(Default)]
struct Observed {
    phase: Option<&'static str>,
    view: Option<View>,
    connected: bool,
    pressed: [bool; 32],
    pending: Option<Pending>,
    navigation: usize,
    sleeps: usize,
    wakes: usize,
    discarded: usize,
    logo_restored: Option<bool>,
    error: Option<String>,
    events: VecDeque<Value>,
}
struct Shared {
    state: Mutex<Observed>,
    changed: Condvar,
    cancel: AtomicBool,
    synthetic: bool,
}
impl Shared {
    fn new(synthetic: bool) -> Self {
        Self {
            state: Mutex::new(Observed::default()),
            changed: Condvar::new(),
            cancel: AtomicBool::new(false),
            synthetic,
        }
    }
    fn fail(&self, error: String) {
        lock(&self.state).error.get_or_insert(error);
        self.changed.notify_all();
    }
    fn cancel(&self) {
        // Synchronize the condition change with wait_timeout's unlock-and-wait;
        // an atomic flag plus an unlocked notification could lose the wake.
        let _state = lock(&self.state);
        self.cancel.store(true, Ordering::Release);
        self.changed.notify_all();
    }
    fn push(state: &mut Observed, value: Value) {
        if state.events.len() < 4096 {
            state.events.push_back(value);
        } else {
            state
                .error
                .get_or_insert("instrument event budget exhausted".into());
        }
    }
    fn flush(&self) -> Result<()> {
        let events = std::mem::take(&mut lock(&self.state).events);
        for event in events {
            emit(event)?;
        }
        Ok(())
    }
    fn wait(&self, duration: Duration, predicate: impl Fn(&Observed) -> bool) -> Result<bool> {
        let deadline = Instant::now() + duration;
        let mut state = lock(&self.state);
        loop {
            if let Some(error) = &state.error {
                return Err(error.clone());
            }
            if self.cancel.load(Ordering::Acquire) {
                return Err("instrument cancelled".into());
            }
            if predicate(&state) {
                return Ok(true);
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Ok(false);
            };
            let waited = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = waited.0;
        }
    }
    fn human_complete(state: &Observed) -> bool {
        state.navigation >= NAVIGATION_SAMPLES
            && state.sleeps >= 1
            && state.wakes >= 1
            && !state.pressed.iter().any(|down| *down)
            && state.view.is_some_and(|view| !view.asleep)
    }
}

#[derive(Default)]
struct Fake {
    events: Mutex<VecDeque<Event>>,
    stopped: AtomicBool,
}
enum Backend {
    Real(Deck),
    Fake(Arc<Fake>),
}
impl Backend {
    fn next(&self) -> Option<Event> {
        match self {
            Self::Real(deck) => deck.next_event(),
            Self::Fake(fake) => lock(&fake.events).pop_front(),
        }
    }
    fn publish(&self, desired: Desired) -> std::result::Result<(), DeckError> {
        match self {
            Self::Real(deck) => deck.set_desired(desired),
            Self::Fake(_) => Ok(()),
        }
    }
    fn shutdown(&self) {
        match self {
            Self::Real(deck) => deck.shutdown(),
            Self::Fake(fake) => {
                if !fake.stopped.swap(true, Ordering::AcqRel) {
                    lock(&fake.events).push_back(Event::Stopped {
                        logo_restored: true,
                    });
                }
            }
        }
    }
    fn stopped(&self) -> bool {
        match self {
            Self::Real(deck) => deck.is_stopped(),
            Self::Fake(fake) => fake.stopped.load(Ordering::Acquire),
        }
    }
}
struct Port {
    backend: Backend,
    shared: Arc<Shared>,
    expected: Arc<Expected>,
}
impl Port {
    fn note(&self, event: &Event, dequeued: Instant) {
        let mut state = lock(&self.shared.state);
        match event {
            Event::Connected => {
                state.connected = true;
                state.pending = None;
                state.pressed = [false; 32];
                Shared::push(
                    &mut state,
                    json!({"event":"deck_connected","synthetic":self.shared.synthetic}),
                );
            }
            Event::Disconnected(error) => {
                state.connected = false;
                state.pending = None;
                state.error = Some(format!("deck disconnected: {error:?}"));
                Shared::push(
                    &mut state,
                    json!({"event":"deck_disconnected","error":format!("{error:?}")}),
                );
            }
            Event::Stopped { logo_restored } => {
                state.logo_restored = Some(*logo_restored);
                Shared::push(
                    &mut state,
                    json!({"event":"deck_stopped","logo_restored":logo_restored,"synthetic":self.shared.synthetic}),
                );
            }
            Event::Report(keys) => {
                let rising: Vec<_> = keys
                    .iter()
                    .enumerate()
                    .filter_map(|(key, down)| (*down && !state.pressed[key]).then_some(key))
                    .collect();
                if let Some(pending) = &mut state.pending {
                    pending.ambiguous = true;
                }
                if state.phase == Some("human") && !rising.is_empty() {
                    if rising.len() == 1
                        && let Some(view) = state.view
                    {
                        let key = rising[0];
                        let transition = if view.asleep {
                            Some((
                                "wake",
                                View {
                                    asleep: false,
                                    ..view
                                },
                            ))
                        } else if key == 0 {
                            Some((
                                "navigation",
                                View {
                                    child: !view.child,
                                    asleep: false,
                                },
                            ))
                        } else if key == 2 {
                            Some((
                                "sleep",
                                View {
                                    asleep: true,
                                    ..view
                                },
                            ))
                        } else {
                            None
                        };
                        if let Some((operation, expected)) = transition {
                            let ambiguous = state.pending.is_some()
                                || keys.iter().filter(|down| **down).count() != 1;
                            state.pending = Some(Pending {
                                at: dequeued,
                                expected,
                                operation,
                                key,
                                ambiguous,
                            });
                        }
                    } else {
                        state.discarded += 1;
                        state.pending = None;
                    }
                }
                state.pressed = *keys;
            }
        }
        self.shared.changed.notify_all();
    }
}
impl DeckPort for Port {
    fn next_event(&mut self) -> Option<Event> {
        let event = self.backend.next()?;
        self.note(&event, Instant::now());
        Some(event)
    }
    fn set_desired(&mut self, desired: Desired) -> std::result::Result<(), DeckError> {
        let Some(view) = self.expected.view(&desired) else {
            self.shared
                .fail("unexpected projection; not forwarded to hardware".into());
            return Err(DeckError::InvalidDesired);
        };
        self.backend.publish(desired)?;
        let published = Instant::now();
        let mut state = lock(&self.shared.state);
        if let Some(pending) = state.pending.take() {
            if pending.expected == view && !pending.ambiguous {
                match pending.operation {
                    "navigation" => state.navigation += 1,
                    "sleep" => state.sleeps += 1,
                    "wake" => state.wakes += 1,
                    _ => unreachable!(),
                }
                Shared::push(
                    &mut state,
                    json!({"event":"sample","phase":"human","operation":pending.operation,
                    "ns":published.duration_since(pending.at).as_nanos(),"trigger_key":pending.key,
                    "trigger_edge":"rising","synthetic":self.shared.synthetic,"boundary":"report_dequeue_to_expected_desired_forwarded"}),
                );
            } else {
                state.discarded += 1;
                Shared::push(
                    &mut state,
                    json!({"event":"discarded","reason":"coalesced_reports_or_unexpected_projection"}),
                );
            }
        }
        state.view = Some(view);
        self.shared.changed.notify_all();
        Ok(())
    }
    fn shutdown(&mut self) {
        self.backend.shutdown();
    }
    fn is_stopped(&self) -> bool {
        let stopped = self.backend.stopped();
        if stopped {
            // Server stops pumping deck events once shutting down. Preserve the real
            // worker's stop evidence here without issuing any extra device operation.
            while let Some(event) = self.backend.next() {
                self.note(&event, Instant::now());
            }
        }
        stopped
    }
}

struct Relay {
    read: UnixStream,
    write: UnixStream,
    wake: Arc<mio::Waker>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Relay {
    fn new(shared: Arc<Shared>) -> Result<Self> {
        let mut poll = mio::Poll::new().map_err(|e| e.to_string())?;
        let wake =
            Arc::new(mio::Waker::new(poll.registry(), mio::Token(0)).map_err(|e| e.to_string())?);
        let (read, write) = UnixStream::pair().map_err(|e| e.to_string())?;
        read.set_nonblocking(true).map_err(|e| e.to_string())?;
        write.set_nonblocking(true).map_err(|e| e.to_string())?;
        let mut writer = write.try_clone().map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = thread::spawn(move || {
            let mut events = mio::Events::with_capacity(8);
            loop {
                match poll.poll(&mut events, None) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        shared.fail(format!("relay poll: {e}"));
                        let _ = notify(&mut writer);
                        break;
                    }
                }
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                if let Err(e) = notify(&mut writer) {
                    shared.fail(e);
                    break;
                }
            }
        });
        Ok(Self {
            read,
            write,
            wake,
            stop,
            thread: Some(thread),
        })
    }
    fn stop(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::Release);
        self.wake.wake().map_err(|e| e.to_string())?;
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| "relay panicked")?;
        }
        Ok(())
    }
}
impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

enum Action {
    Attach,
    Begin(&'static str),
    End,
    Finish(Option<String>),
}
struct Control {
    action: Action,
    ack: mpsc::SyncSender<Result<()>>,
}
struct Signals {
    commands: mpsc::Sender<Control>,
    socket: UnixStream,
    shared: Arc<Shared>,
}
impl Signals {
    fn send(&mut self, action: Action) -> Result<()> {
        if self.shared.cancel.load(Ordering::Acquire) {
            return Err("instrument cancelled".into());
        }
        let (ack, reply) = mpsc::sync_channel(0);
        self.commands
            .send(Control { action, ack })
            .map_err(|_| "control closed")?;
        notify(&mut self.socket)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.shared.cancel.load(Ordering::Acquire) {
                return Err("instrument cancelled".into());
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("control acknowledgement deadline")?;
            match reply.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err("control closed".into()),
            }
        }
    }
    fn finish(&mut self, error: Option<String>) {
        if self.shared.cancel.load(Ordering::Acquire) {
            return;
        }
        // Last notification never waits for an ACK: cancellation may already have
        // moved the authority loop into cleanup, where no commands are consumed.
        let (ack, _) = mpsc::sync_channel(0);
        if self
            .commands
            .send(Control {
                action: Action::Finish(error),
                ack,
            })
            .is_ok()
        {
            let _ = notify(&mut self.socket);
        }
    }
}
#[derive(Default)]
struct Turns {
    count: u64,
    total: u128,
    maximum: u128,
}
impl Turns {
    fn record(&mut self, ns: u128) {
        self.count += 1;
        self.total += ns;
        self.maximum = self.maximum.max(ns);
    }
}
struct Phase {
    name: &'static str,
    started: Instant,
    turns: Turns,
}
fn end_phase(phase: &mut Option<Phase>, shared: &Shared) -> Result<()> {
    if let Some(phase) = phase.take() {
        lock(&shared.state).phase = None;
        emit(json!({"event":"phase_end","phase":phase.name}))?;
        emit(
            json!({"event":"turn_summary","phase":phase.name,"count":phase.turns.count,
            "total_ns":phase.turns.total,"max_ns":phase.turns.maximum,"wall_ns":phase.started.elapsed().as_nanos()}),
        )?;
    }
    Ok(())
}

fn call(client: &mut Client, operation: &str, args: Value) -> Result<Value> {
    let response = client
        .request_until(operation, args, Instant::now() + Duration::from_secs(1))
        .map_err(|e| format!("{operation}: {e:?}"))?;
    if response.error.is_some() {
        return Err(format!("{operation}: {:?}", response.error));
    }
    serde_json::to_value(response.data).map_err(|e| e.to_string())
}
fn execute(client: &mut Client, id: &str) -> Result<u128> {
    let started = Instant::now();
    let receipt = call(client, "run", json!({"id":id}))?;
    let deadline = started + Duration::from_secs(5);
    loop {
        let result = call(
            client,
            "result",
            json!({"execution_id":receipt["execution_id"]}),
        )?;
        if result["state"] == "succeeded" {
            return Ok(started.elapsed().as_nanos());
        }
        if !matches!(result["state"].as_str(), Some("running" | "waiting")) {
            return Err(format!("execution failed: {result}"));
        }
        if Instant::now() > deadline {
            return Err("execution deadline".into());
        }
    }
}
fn slot(client: &mut Client, folder: &str, position: usize) -> Result<String> {
    call(client, "get", json!({"id":folder}))?["fields"]["slots"][position]["id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "missing seeded slot".into())
}
fn appearance(client: &mut Client, id: &str, text: &str, position: &str) -> Result<()> {
    call(
        client,
        "edit",
        json!({"changes":[{"kind":"set","id":id,"field":"appearance",
        "value":{"text":text,"image":null,"text_position":position}}]}),
    )?;
    Ok(())
}
struct Seed {
    enter: String,
    back: String,
    root: String,
    child: String,
}
fn seed(client: &mut Client) -> Result<Seed> {
    call(
        client,
        "set_settings",
        json!({"changes":{"brightness_percent":BRIGHTNESS,"sleep_ms":"0","press_threshold_ms":"0"}}),
    )?;
    let root = call(client, "status", json!({}))?["active_folder_id"]
        .as_str()
        .ok_or("missing root")?
        .to_owned();
    call(
        client,
        "create_folder",
        json!({"parent_id":root,"position":0,"name":"ENTRAR"}),
    )?;
    let enter = slot(client, &root, 0)?;
    appearance(client, &enter, "ENTRAR", "middle")?;
    execute(client, &enter)?;
    let child = call(client, "status", json!({}))?["active_folder_id"]
        .as_str()
        .ok_or("missing child")?
        .to_owned();
    let back = slot(client, &child, 0)?;
    appearance(client, &back, "VOLVER", "middle")?;
    for folder in [&child, &root] {
        call(
            client,
            "create_button",
            json!({"folder_id":folder,"position":2,"name":"LUNA"}),
        )?;
        let button = slot(client, folder, 2)?;
        call(
            client,
            "add_action",
            json!({"button_id":button,"type":"sleep","parameters":null}),
        )?;
        appearance(client, &button, "", "bottom")?;
    }
    execute(client, &back)?;
    Ok(Seed {
        enter,
        back,
        root,
        child,
    })
}
fn fake_press(
    fake: &Fake,
    wake: &mio::Waker,
    shared: &Shared,
    key: usize,
    operation: &'static str,
) -> Result<()> {
    let before = {
        let s = lock(&shared.state);
        match operation {
            "navigation" => s.navigation,
            "sleep" => s.sleeps,
            _ => s.wakes,
        }
    };
    let mut keys = [false; 32];
    keys[key] = true;
    lock(&fake.events).push_back(Event::Report(keys));
    wake.wake().map_err(|e| e.to_string())?;
    let accepted = shared.wait(Duration::from_secs(3), |s| match operation {
        "navigation" => s.navigation > before,
        "sleep" => s.sleeps > before,
        _ => s.wakes > before,
    })?;
    if !accepted {
        return Err("synthetic transition did not reach expected Desired".into());
    }
    pause(shared, Duration::from_millis(30))?;
    lock(&fake.events).push_back(Event::Report([false; 32]));
    wake.wake().map_err(|e| e.to_string())?;
    if !shared.wait(Duration::from_secs(3), |s| {
        !s.pressed.iter().any(|down| *down)
    })? {
        return Err("synthetic release deadline".into());
    }
    pause(shared, Duration::from_millis(30))?;
    Ok(())
}
fn pause(shared: &Shared, duration: Duration) -> Result<()> {
    // A timed condition wait does not wake the authority loop, and cancellation
    // interrupts it immediately rather than leaving a sleeping client to join.
    let _ = shared.wait(duration, |_| false)?;
    Ok(())
}
fn scenario(
    root: &Path,
    shared: &Shared,
    fake: &Fake,
    wake: &mio::Waker,
    signals: &mut Signals,
) -> Result<bool> {
    let mut client = Client::connect(&root.join("runtime"), Endpoint::Editor)
        .map_err(|e| format!("connect: {e:?}"))?;
    let seed = seed(&mut client)?;
    signals.send(Action::Attach)?;
    if !shared.wait(Duration::from_secs(10), |s| {
        s.connected
            && s.view
                == Some(View {
                    child: false,
                    asleep: false,
                })
    })? {
        return Err("deck connection/projection deadline".into());
    }
    // This settling delay is not an upload acknowledgement.
    pause(shared, Duration::from_secs(2))?;
    signals.send(Action::Begin("idle"))?;
    pause(
        shared,
        Duration::from_secs(if shared.synthetic { 1 } else { 20 }),
    )?;
    signals.send(Action::End)?;
    // Keep the already-connected peer below Transport's 60s idle deadline. This
    // avoids a new ten-second hello handshake after hardware has been attached.
    signals.send(Action::Begin("auto"))?;
    for iteration in 0..12 {
        let (button, folder) = if iteration % 2 == 0 {
            (&seed.enter, &seed.child)
        } else {
            (&seed.back, &seed.root)
        };
        let ns = execute(&mut client, button)?;
        if call(&mut client, "status", json!({}))?["active_folder_id"] != *folder {
            return Err("IPC navigation folder mismatch".into());
        }
        emit(
            json!({"event":"sample","phase":"auto","operation":"ipc_navigation_complete_observed","ns":ns,"iteration":iteration,"synthetic":shared.synthetic}),
        )?;
        pause(
            shared,
            Duration::from_millis(if shared.synthetic { 30 } else { 350 }),
        )?;
    }
    signals.send(Action::End)?;
    drop(client);
    signals.send(Action::Begin("human"))?;
    emit(
        json!({"event":"ready_human","navigation_required":NAVIGATION_SAMPLES,"sleep_required":1,"wake_required":1,
        "navigation_key":0,"sleep_key":2,"human_timeout_s":300,"synthetic":shared.synthetic}),
    )?;
    if shared.synthetic {
        for _ in 0..NAVIGATION_SAMPLES {
            fake_press(fake, wake, shared, 0, "navigation")?;
        }
        fake_press(fake, wake, shared, 2, "sleep")?;
        fake_press(fake, wake, shared, 2, "wake")?;
    }
    let complete = shared.wait(Duration::from_secs(300), Shared::human_complete)?;
    signals.send(Action::End)?;
    Ok(complete)
}

fn private_dir(path: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|e| e.to_string())
}
fn root_path(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let allowed = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/performance")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let parent = path
        .parent()
        .ok_or("missing parent")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !parent.starts_with(&allowed) || path.file_name().is_none() {
        return Err("root must be new under target/performance".into());
    }
    Ok(parent.join(path.file_name().unwrap()))
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let physical = match args.as_slice() {
        [_,mode] if mode == "--dry-run" => false,
        [_,mode,consent] if mode == "--physical" && consent == "--exclusive-authorized" => true,
        _ => return Err("usage: physical_profile NEW_ROOT --dry-run | NEW_ROOT --physical --exclusive-authorized".into()),
    };
    let root = root_path(&args[0])?;
    private_dir(&root)?;
    for name in [
        "runtime", "home", "data", "system", "config", "cache", "tmp",
    ] {
        private_dir(&root.join(name))?;
    }
    let shutdown = ShutdownSignals::install().map_err(|e| format!("shutdown signals: {e:?}"))?;
    let mut server = Server::new(
        &root.join("runtime"),
        &root.join("store"),
        NonZeroU128::new(0x_5048_5953).unwrap(),
    )
    .map_err(|e| format!("server: {e:?}"))?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let bin = executable
        .parent()
        .and_then(Path::parent)
        .ok_or("example location")?;
    let mut helper = Command::new(bin.join("rad-launcher-helper"));
    helper
        .env_clear()
        .env("HOME", root.join("home"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_DATA_DIRS", root.join("system"))
        .env("XDG_CONFIG_DIRS", root.join("system"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("TMPDIR", root.join("tmp"))
        .env("PATH", "/nonexistent-rad-private-bin")
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent-rad-private-bus",
        );
    server
        .attach_launcher(
            rad_launcher::Launcher::spawn(helper).map_err(|e| format!("helper: {e:?}"))?,
        )
        .map_err(|e| format!("attach helper: {e:?}"))?;
    let shared = Arc::new(Shared::new(!physical));
    let expected = Arc::new(Expected::new());
    let fake = Arc::new(Fake::default());
    let mut relay = Relay::new(shared.clone())?;
    server
        .register_worker_io(
            &mut mio::unix::SourceFd(&relay.read.as_raw_fd()),
            mio::Interest::READABLE,
        )
        .map_err(|e| format!("register relay: {e:?}"))?;
    server
        .register_worker_io(
            &mut mio::unix::SourceFd(&shutdown.as_raw_fd()),
            mio::Interest::READABLE,
        )
        .map_err(|e| format!("register signals: {e:?}"))?;
    emit(
        json!({"event":"metadata","mode":if physical {"physical"} else {"dry_run"},"pid":std::process::id(),
        "brightness_percent":BRIGHTNESS,"idle_seconds":if physical {20} else {1},"client":"in-process thread included",
        "limits":["public Report has no HID timestamp","relay thread added before Report dequeue","sample includes wrapper matching overhead",
        "Desired forwarding is not render/USB/pixel acknowledgement","no original framebuffer or brightness recovery","shutdown restores device logo and configured25percent"]}),
    )?;
    let (commands, received) = mpsc::channel();
    let mut signals = Signals {
        commands,
        socket: relay.write.try_clone().map_err(|e| e.to_string())?,
        shared: shared.clone(),
    };
    let scenario_shared = shared.clone();
    let scenario_fake = fake.clone();
    let scenario_wake = relay.wake.clone();
    let client = thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scenario(
                &root,
                &scenario_shared,
                &scenario_fake,
                &scenario_wake,
                &mut signals,
            )
        }))
        .unwrap_or_else(|_| Err("scenario panicked".into()));
        signals.finish(result.as_ref().err().cloned());
        result
    });
    let deadline = Instant::now() + Duration::from_secs(540);
    let mut phase: Option<Phase> = None;
    let mut finishing = false;
    let drive = (|| -> Result<()> {
        while !finishing {
            drain(&mut relay.read)?;
            if shutdown
                .take_shutdown()
                .map_err(|e| format!("shutdown read: {e:?}"))?
            {
                return Err("interrupted by shutdown signal".into());
            }
            while let Ok(control) = received.try_recv() {
                let result = match control.action {
                    Action::Attach => {
                        let backend = if physical {
                            Backend::Real(
                                Deck::spawn(
                                    rad_deck::DeviceConfig::xl(),
                                    expected.initial(),
                                    relay.wake.clone(),
                                )
                                .map_err(|e| format!("deck spawn: {e:?}"))?,
                            )
                        } else {
                            lock(&fake.events)
                                .extend([Event::Connected, Event::Report([false; 32])]);
                            relay.wake.wake().map_err(|e| e.to_string())?;
                            Backend::Fake(fake.clone())
                        };
                        server
                            .attach_deck_port(Box::new(Port {
                                backend,
                                shared: shared.clone(),
                                expected: expected.clone(),
                            }))
                            .map_err(|e| format!("attach port: {e:?}"))
                    }
                    Action::Begin(name) => {
                        if phase.is_some() {
                            Err("phase already running".into())
                        } else {
                            lock(&shared.state).phase = Some(name);
                            phase = Some(Phase {
                                name,
                                started: Instant::now(),
                                turns: Turns::default(),
                            });
                            emit(json!({"event":"phase_begin","phase":name}))
                        }
                    }
                    Action::End => end_phase(&mut phase, &shared),
                    Action::Finish(error) => {
                        finishing = true;
                        if let Some(error) = error {
                            Err(error)
                        } else {
                            Ok(())
                        }
                    }
                };
                let _ = control.ack.send(result.clone());
                result?;
            }
            let started = Instant::now();
            server.turn().map_err(|e| format!("server turn: {e:?}"))?;
            if let Some(phase) = &mut phase {
                phase.turns.record(started.elapsed().as_nanos());
            }
            shared.flush()?;
            if let Some(error) = &lock(&shared.state).error {
                return Err(error.clone());
            }
            if !finishing {
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .ok_or("absolute instrument deadline")?;
                server
                    .wait(remaining)
                    .map_err(|e| format!("server wait: {e:?}"))?;
            }
        }
        Ok(())
    })();
    shared.cancel();
    let _ = end_phase(&mut phase, &shared);
    server.request_shutdown();
    let grace = Instant::now() + Duration::from_secs(5);
    let cleanup = (|| -> Result<()> {
        while !server.shutdown_complete() {
            drain(&mut relay.read)?;
            server.turn().map_err(|e| format!("shutdown turn: {e:?}"))?;
            shared.flush()?;
            let remaining = grace
                .checked_duration_since(Instant::now())
                .ok_or("shutdown deadline")?;
            if !server.shutdown_complete() {
                server
                    .wait(remaining)
                    .map_err(|e| format!("shutdown wait: {e:?}"))?;
            }
        }
        shared.flush()?;
        Ok(())
    })();
    let relay_result = relay.stop();
    let client_result = client
        .join()
        .map_err(|_| "scenario join panic".to_string())
        .and_then(|r| r);
    let state = lock(&shared.state);
    let human_complete =
        client_result.as_ref().is_ok_and(|complete| *complete) && Shared::human_complete(&state);
    let clean = cleanup.is_ok() && relay_result.is_ok() && state.logo_restored == Some(true);
    let failure = drive
        .err()
        .or_else(|| cleanup.err())
        .or_else(|| relay_result.err())
        .or_else(|| client_result.err())
        .or_else(|| state.error.clone());
    let _ = emit(
        json!({"event":"complete","mode":if physical {"physical"} else {"dry_run"},"clean_shutdown":clean,"human_complete":human_complete,
        "navigation_samples":state.navigation,"sleep_samples":state.sleeps,"wake_samples":state.wakes,"discarded":state.discarded,
        "human_counts":{"navigation":state.navigation,"sleep":state.sleeps,"wake":state.wakes,
            "all_released":!state.pressed.iter().any(|down|*down)},
        "logo_restored":state.logo_restored==Some(true),"synthetic":shared.synthetic,"error":failure}),
    );
    if let Some(error) = failure {
        Err(error)
    } else if !clean {
        Err("cleanup or logo restoration not confirmed".into())
    } else {
        Ok(())
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("physical_profile: {error}");
        std::process::exit(1);
    }
}
