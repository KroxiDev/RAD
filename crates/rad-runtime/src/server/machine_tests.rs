//! «Usar el Stream Deck con RAD» (ADR 0006) as a setting of the machine: loaded at start,
//! changed through the client API, and applied to the deck without restarting the runtime.
use super::*;
use rad_api::{DeckState, EventCode};
use rad_deck::{DeckError, Desired, Event};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Instant,
};

/// What a fake deck did and what the test wants it to do next.
#[derive(Default)]
struct PortState {
    events: VecDeque<Event>,
    shutdowns: usize,
    /// The deck reports being stopped only when the test says so.
    stopped: bool,
    dropped: bool,
}
struct Port(Arc<Mutex<PortState>>);
impl crate::display::DeckPort for Port {
    fn next_event(&mut self) -> Option<Event> {
        self.0.lock().unwrap().events.pop_front()
    }
    fn set_desired(&mut self, _: Desired) -> Result<(), DeckError> {
        Ok(())
    }
    fn shutdown(&mut self) {
        self.0.lock().unwrap().shutdowns += 1;
    }
    fn is_stopped(&self) -> bool {
        self.0.lock().unwrap().stopped
    }
}
impl Drop for Port {
    fn drop(&mut self) {
        self.0.lock().unwrap().dropped = true;
    }
}

/// How the runtime is told to open a deck: counts attempts, keeps every port it handed out.
#[derive(Clone, Default)]
struct Factory {
    attempts: Arc<AtomicUsize>,
    fails: Arc<AtomicBool>,
    ports: Arc<Mutex<Vec<Arc<Mutex<PortState>>>>>,
}
impl Factory {
    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
    fn port(&self, index: usize) -> Arc<Mutex<PortState>> {
        self.ports.lock().unwrap()[index].clone()
    }
    fn open(
        &self,
    ) -> impl FnMut(Desired, Arc<mio::Waker>) -> Result<Box<dyn crate::display::DeckPort>, DeckError>
    + use<> {
        let this = self.clone();
        move |_, _| {
            this.attempts.fetch_add(1, Ordering::SeqCst);
            if this.fails.load(Ordering::SeqCst) {
                return Err(DeckError::Unavailable);
            }
            let state = Arc::new(Mutex::new(PortState::default()));
            this.ports.lock().unwrap().push(state.clone());
            Ok(Box::new(Port(state)))
        }
    }
}

struct Machine {
    root: super::contained_tests::Directory,
    server: Server,
    factory: Factory,
}
impl Machine {
    fn data(&self) -> std::path::PathBuf {
        self.root.0.join("data")
    }
    fn file(&self) -> std::path::PathBuf {
        self.data().join("machine.json")
    }
    fn stored(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.file()).unwrap()).unwrap()
    }
}

/// A runtime whose data directory already holds `machine.json` with these bytes, if any,
/// with a deck factory registered the way the binary does.
fn machine(name: &str, session: u128, file: Option<&[u8]>) -> Machine {
    let root = super::contained_tests::Directory(
        std::env::temp_dir().join(format!("rad-machine-{}-{name}", std::process::id())),
    );
    std::fs::create_dir(&root.0).unwrap();
    std::fs::set_permissions(&root.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let data = root.0.join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    if let Some(bytes) = file {
        std::fs::write(data.join("machine.json"), bytes).unwrap();
    }
    let mut server = Server::new(&root.0, &data, NonZeroU128::new(session).unwrap()).unwrap();
    let factory = Factory::default();
    server.attach_deck_with(factory.open()).unwrap();
    Machine {
        root,
        server,
        factory,
    }
}
const ON: &[u8] = br#"{"version":1,"use_stream_deck":true,"start_with_session":false}"#;
const OFF: &[u8] = br#"{"version":1,"use_stream_deck":false,"start_with_session":false}"#;

/// A client connection driven in the same thread as the server.
struct Client(UnixStream, u64);
impl Client {
    fn connect(machine: &mut Machine, socket: &str) -> Self {
        let stream = UnixStream::connect(machine.root.0.join("rad").join(socket)).unwrap();
        stream.set_nonblocking(true).unwrap();
        let mut client = Self(stream, 0);
        let hello = client.ask(machine, "hello", json!({}));
        assert_eq!(hello["status"], "ok", "{hello}");
        client
    }
    fn ask(&mut self, machine: &mut Machine, op: &str, args: Value) -> Value {
        self.1 += 1;
        let id = self.1.to_string();
        let bytes = serde_json::to_vec(&json!({"v":1,"id":id,"op":op,"args":args})).unwrap();
        self.0
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .unwrap();
        self.0.write_all(&bytes).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut received = Vec::new();
        while Instant::now() < deadline {
            machine.server.turn().unwrap();
            let mut chunk = [0; 16384];
            match self.0.read(&mut chunk) {
                Ok(count) => received.extend_from_slice(&chunk[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("{error}"),
            }
            if received.len() >= 4 {
                let length = u32::from_be_bytes(received[..4].try_into().unwrap()) as usize;
                if received.len() >= 4 + length {
                    return serde_json::from_slice(&received[4..4 + length]).unwrap();
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("no answer to {op}");
    }
}

fn settings(use_stream_deck: bool, start_with_session: bool) -> Value {
    json!({"use_stream_deck":use_stream_deck,"start_with_session":start_with_session})
}
fn turns(machine: &mut Machine, count: usize) {
    for _ in 0..count {
        machine.server.turn().unwrap();
    }
}
fn codes(server: &Server) -> Vec<EventCode> {
    server
        .events
        .view()
        .entries
        .iter()
        .map(|e| e.code)
        .collect()
}

#[test]
fn the_deck_is_opened_at_start_only_when_the_setting_is_on() {
    let on = machine("start-on", 1001, Some(ON));
    assert_eq!(on.factory.attempts(), 1);
    assert!(on.server.deck.is_some());
    assert_eq!(on.server.deck_state(), DeckState::Disconnected);

    for (name, session, file) in [("start-off", 1002, Some(OFF)), ("start-new", 1003, None)] {
        let off = machine(name, session, file);
        assert_eq!(off.factory.attempts(), 0, "{name}");
        assert!(off.server.deck.is_none());
        assert_eq!(off.server.deck_state(), DeckState::Disabled);
        // An absent file is the default, not a failure worth reporting.
        assert_eq!(codes(&off.server), [EventCode::Started], "{name}");
    }
}

#[test]
fn an_unreadable_file_means_defaults_an_event_and_the_same_bytes() {
    let garbage: &[u8] = b"{\"version\":99, not json";
    let mut machine = machine("unreadable", 1004, Some(garbage));
    assert_eq!(machine.factory.attempts(), 0);
    assert_eq!(machine.server.deck_state(), DeckState::Disabled);
    let mut editor = Client::connect(&mut machine, "editor.sock");
    let log = editor.ask(&mut machine, "runtime_log", json!({}));
    let entries = log["data"]["entries"].as_array().unwrap();
    let unreadable: Vec<_> = entries
        .iter()
        .filter(|e| e["code"] == "machine_settings_unreadable")
        .collect();
    assert_eq!(unreadable.len(), 1, "{entries:?}");
    assert!(unreadable[0].get("subject").is_none());
    assert_eq!(
        editor.ask(&mut machine, "machine_settings", json!({}))["data"],
        settings(false, false)
    );
    assert_eq!(std::fs::read(machine.file()).unwrap(), garbage);
}

#[test]
fn machine_settings_reports_what_was_loaded_and_is_for_the_editor_only() {
    let mut machine = machine(
        "read",
        1005,
        Some(br#"{"version":1,"use_stream_deck":true,"start_with_session":true}"#),
    );
    let mut editor = Client::connect(&mut machine, "editor.sock");
    let answer = editor.ask(&mut machine, "machine_settings", json!({}));
    assert_eq!(answer["status"], "ok");
    assert_eq!(answer["data"], settings(true, true));

    let mut automation = Client::connect(&mut machine, "automation.sock");
    for (op, args) in [
        ("machine_settings", json!({})),
        ("set_machine_settings", json!({"use_stream_deck":false})),
    ] {
        let denied = automation.ask(&mut machine, op, args);
        assert_eq!(denied["status"], "error", "{op}");
        assert_eq!(denied["error"]["code"], "permission_denied", "{op}");
    }
    // The refused change changed nothing.
    assert_eq!(
        editor.ask(&mut machine, "machine_settings", json!({}))["data"],
        settings(true, true)
    );
    assert_eq!(machine.stored()["use_stream_deck"], true);
}

#[test]
fn set_machine_settings_stores_the_change_and_answers_with_the_resulting_view() {
    let mut machine = machine("set", 1006, None);
    let mut editor = Client::connect(&mut machine, "editor.sock");
    let answer = editor.ask(
        &mut machine,
        "set_machine_settings",
        json!({"start_with_session":true}),
    );
    // Not `applied`: it is no revision of the configuration.
    assert_eq!(answer["status"], "ok");
    assert_eq!(answer["data"], settings(false, true));
    assert_eq!(
        machine.stored(),
        json!({"version":1,"use_stream_deck":false,"start_with_session":true})
    );
    // A field left out keeps what it had.
    let answer = editor.ask(
        &mut machine,
        "set_machine_settings",
        json!({"use_stream_deck":true}),
    );
    assert_eq!(answer["data"], settings(true, true));
    assert_eq!(machine.stored()["start_with_session"], true);
    assert_eq!(machine.stored()["use_stream_deck"], true);

    let empty = editor.ask(&mut machine, "set_machine_settings", json!({}));
    assert_eq!(empty["status"], "error");
    assert_eq!(empty["error"]["code"], "invalid_request");
    assert_eq!(empty["error"]["outcome"], "not_applied");
}

#[test]
fn a_change_that_cannot_be_stored_is_not_applied_anywhere() {
    let mut machine = machine("save-fails", 1007, None);
    let mut editor = Client::connect(&mut machine, "editor.sock");
    // As root a permission would not stop the write; a directory in the way always does.
    std::fs::create_dir(machine.file()).unwrap();
    let failed = editor.ask(
        &mut machine,
        "set_machine_settings",
        json!({"use_stream_deck":true}),
    );
    assert_eq!(failed["status"], "error");
    assert_eq!(failed["error"]["code"], "storage_error");
    assert_eq!(failed["error"]["outcome"], "not_applied");
    assert_eq!(
        editor.ask(&mut machine, "machine_settings", json!({}))["data"],
        settings(false, false)
    );
    turns(&mut machine, 3);
    assert_eq!(machine.factory.attempts(), 0);
    assert_eq!(machine.server.deck_state(), DeckState::Disabled);
}

#[test]
fn turning_the_setting_on_opens_the_deck_once_and_a_failure_is_not_retried() {
    let mut machine = machine("hot-on", 1008, None);
    let mut editor = Client::connect(&mut machine, "editor.sock");
    let on = json!({"use_stream_deck":true});
    let off = json!({"use_stream_deck":false});

    assert_eq!(machine.factory.attempts(), 0);
    editor.ask(&mut machine, "set_machine_settings", on.clone());
    turns(&mut machine, 3);
    assert_eq!(machine.factory.attempts(), 1);
    assert_eq!(machine.server.deck_state(), DeckState::Disconnected);
    machine
        .factory
        .port(0)
        .lock()
        .unwrap()
        .events
        .push_back(Event::Connected);
    turns(&mut machine, 1);
    assert_eq!(machine.server.deck_state(), DeckState::Connected);
    // The same value again writes and opens nothing.
    editor.ask(&mut machine, "set_machine_settings", on.clone());
    turns(&mut machine, 2);
    assert_eq!(machine.factory.attempts(), 1);

    // Off, then on with a deck that cannot be had.
    editor.ask(&mut machine, "set_machine_settings", off.clone());
    machine.factory.port(0).lock().unwrap().stopped = true;
    turns(&mut machine, 2);
    assert_eq!(machine.server.deck_state(), DeckState::Disabled);
    machine.factory.fails.store(true, Ordering::SeqCst);
    editor.ask(&mut machine, "set_machine_settings", on.clone());
    turns(&mut machine, 5);
    assert_eq!(machine.factory.attempts(), 2, "one attempt, no retries");
    assert_eq!(machine.server.deck_state(), DeckState::Inaccessible);
    assert_eq!(
        codes(&machine.server)
            .iter()
            .filter(|c| **c == EventCode::DeckSpawnFailed)
            .count(),
        1
    );

    // Off is Disabled, not Inaccessible; on again tries once more, now with success.
    editor.ask(&mut machine, "set_machine_settings", off);
    turns(&mut machine, 2);
    assert_eq!(machine.server.deck_state(), DeckState::Disabled);
    machine.factory.fails.store(false, Ordering::SeqCst);
    editor.ask(&mut machine, "set_machine_settings", on);
    turns(&mut machine, 3);
    assert_eq!(machine.factory.attempts(), 3);
    assert_eq!(machine.server.deck_state(), DeckState::Disconnected);
}

/// The Tools folder of the `server_deck` scenario: pressing key 2 enters it, and inside it
/// key 0 goes back.
fn with_tools_folder(machine: &mut Machine, editor: &mut Client) {
    let created = editor.ask(
        machine,
        "create_folder",
        json!({"parent_id":"f1","position":2,"name":"Tools"}),
    );
    assert_eq!(created["status"], "applied", "{created}");
}
fn pressed(key: usize) -> [bool; 32] {
    let mut keys = [false; 32];
    keys[key] = true;
    keys
}
fn active(server: &Server) -> Option<String> {
    server
        .actor
        .status(Origin::Editor)
        .active
        .map(|binding| format!("{binding:?}"))
}

#[test]
fn turning_the_setting_off_stops_the_deck_in_order_and_lets_authority_know() {
    let mut machine = machine("hot-off", 1009, Some(ON));
    let mut editor = Client::connect(&mut machine, "editor.sock");
    with_tools_folder(&mut machine, &mut editor);
    let first = machine.factory.port(0);
    let root_folder = active(&machine.server);
    first.lock().unwrap().events.extend([
        Event::Connected,
        Event::Report([false; 32]),
        Event::Report(pressed(2)),
        Event::Report([false; 32]),
    ]);
    turns(&mut machine, 1);
    let inside = active(&machine.server);
    assert_ne!(inside, root_folder, "key 2 entered the folder");

    let answer = editor.ask(
        &mut machine,
        "set_machine_settings",
        json!({"use_stream_deck":false}),
    );
    assert_eq!(answer["data"], settings(false, false));
    assert_eq!(first.lock().unwrap().shutdowns, 1);
    // Until the deck says it stopped it is still there, and still asked once only.
    turns(&mut machine, 4);
    assert_ne!(machine.server.deck_state(), DeckState::Disabled);
    assert!(!first.lock().unwrap().dropped);
    assert_eq!(first.lock().unwrap().shutdowns, 1);

    // It stops without saying anything: the port is let go and the disconnect still happens.
    first.lock().unwrap().stopped = true;
    turns(&mut machine, 2);
    assert_eq!(machine.server.deck_state(), DeckState::Disabled);
    assert!(first.lock().unwrap().dropped);
    assert!(machine.server.deck.is_none());

    // A first report with several keys down is what a deck sends when it connects with them
    // held; Authority ignores it only if it was told the previous input was gone.
    editor.ask(
        &mut machine,
        "set_machine_settings",
        json!({"use_stream_deck":true}),
    );
    turns(&mut machine, 1);
    assert_eq!(machine.factory.attempts(), 2);
    machine
        .factory
        .port(1)
        .lock()
        .unwrap()
        .events
        .push_back(Event::Report({
            let mut keys = pressed(0);
            keys[1] = true;
            keys
        }));
    turns(&mut machine, 2);
    assert_eq!(active(&machine.server), inside);
}

#[test]
fn turning_it_back_on_while_the_deck_is_stopping_waits_for_it_to_finish() {
    let mut machine = machine("reactivate", 1010, Some(ON));
    let mut editor = Client::connect(&mut machine, "editor.sock");
    let first = machine.factory.port(0);
    editor.ask(
        &mut machine,
        "set_machine_settings",
        json!({"use_stream_deck":false}),
    );
    editor.ask(
        &mut machine,
        "set_machine_settings",
        json!({"use_stream_deck":true}),
    );
    turns(&mut machine, 4);
    // Only one deck may be alive: the new one waits for the old one to end.
    assert_eq!(machine.factory.attempts(), 1);
    assert!(!first.lock().unwrap().dropped);

    first.lock().unwrap().events.push_back(Event::Stopped {
        logo_restored: true,
    });
    first.lock().unwrap().stopped = true;
    turns(&mut machine, 2);
    assert!(first.lock().unwrap().dropped);
    assert_eq!(machine.factory.attempts(), 2);
    assert!(machine.server.deck.is_some());
    assert_eq!(machine.server.deck_state(), DeckState::Disconnected);
}

#[test]
fn start_with_session_is_only_stored() {
    let mut machine = machine("session", 1011, Some(ON));
    let mut editor = Client::connect(&mut machine, "editor.sock");
    let port = machine.factory.port(0);
    let answer = editor.ask(
        &mut machine,
        "set_machine_settings",
        json!({"start_with_session":true}),
    );
    assert_eq!(answer["data"], settings(true, true));
    turns(&mut machine, 3);
    assert_eq!(machine.stored()["start_with_session"], true);
    assert_eq!(machine.factory.attempts(), 1);
    assert_eq!(port.lock().unwrap().shutdowns, 0);
    assert!(!port.lock().unwrap().dropped);
    assert!(machine.server.deck.is_some());
}

#[test]
fn a_runtime_that_is_shutting_down_opens_no_deck() {
    let mut machine = machine("closing", 1012, Some(ON));
    let mut editor = Client::connect(&mut machine, "editor.sock");
    let first = machine.factory.port(0);
    for value in [false, true] {
        editor.ask(
            &mut machine,
            "set_machine_settings",
            json!({"use_stream_deck":value}),
        );
    }
    // A new deck is waiting for the old one when the runtime is asked to end.
    machine.server.request_shutdown();
    first.lock().unwrap().stopped = true;
    let end = Instant::now() + Duration::from_secs(3);
    while !machine.server.shutdown_complete() {
        assert!(Instant::now() < end, "the runtime ends once its deck did");
        turns(&mut machine, 1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(machine.factory.attempts(), 1);
}
