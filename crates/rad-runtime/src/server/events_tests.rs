//! What the runtime says about itself: the events it records, the line each one leaves for
//! journald and the Editor-only `runtime_log` read (ADR 0011).
use super::{
    contained_tests::{Directory, runtime},
    *,
};
use rad_api::EventCode;
use std::{
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    sync::Mutex,
};

fn codes(server: &Server) -> Vec<EventCode> {
    server
        .events
        .view()
        .entries
        .into_iter()
        .map(|event| event.code)
        .collect()
}

/// The server's events with their stderr lines captured instead of printed.
fn capture_lines(server: &mut Server) -> Arc<Mutex<Vec<String>>> {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    server.events =
        crate::events::Events::with(move |line| sink.lock().unwrap().push(line.to_owned()), || 1);
    lines
}

fn connect(root: &Directory, endpoint: &str) -> UnixStream {
    let socket = UnixStream::connect(root.0.join("rad").join(endpoint)).unwrap();
    socket.set_nonblocking(true).unwrap();
    socket
}
fn send(socket: &mut UnixStream, id: u32, operation: &str) {
    let request =
        serde_json::to_vec(&json!({"v":1,"id":id.to_string(),"op":operation,"args":{}})).unwrap();
    socket
        .write_all(&(request.len() as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&request).unwrap();
}
/// The next whole frame, with the server turning as a real loop would.
fn answer(server: &mut Server, socket: &mut UnixStream) -> Value {
    let end = std::time::Instant::now() + Duration::from_secs(3);
    let mut bytes = Vec::new();
    loop {
        server.turn().unwrap();
        let mut chunk = [0; 8192];
        match socket.read(&mut chunk) {
            Ok(0) => panic!("the server closed the connection"),
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("{error}"),
        }
        if bytes.len() >= 4 {
            let length = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
            if bytes.len() >= 4 + length {
                return serde_json::from_slice(&bytes[4..4 + length]).unwrap();
            }
        }
        assert!(std::time::Instant::now() < end, "no answer from the server");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn ask(server: &mut Server, socket: &mut UnixStream, id: u32, operation: &str) -> Value {
    send(socket, id, operation);
    answer(server, socket)
}

#[test]
fn a_new_runtime_records_that_it_started() {
    let (_root, server) = runtime("log-started", 910);
    assert_eq!(codes(&server), [EventCode::Started]);
}

#[test]
fn an_unexpected_exit_before_this_start_is_recorded_with_its_reason() {
    let root =
        Directory(std::env::temp_dir().join(format!("rad-events-previous-{}", std::process::id())));
    let data = root.0.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::set_permissions(&root.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    crate::exit::record(&data, crate::exit::Reason::Storage, 5).unwrap();
    let server = Server::new(&root.0, &data, NonZeroU128::new(911).unwrap()).unwrap();
    let events = server.events.view().entries;
    assert_eq!(
        events.iter().map(|e| e.code).collect::<Vec<_>>(),
        [EventCode::Started, EventCode::PreviousExit]
    );
    assert_eq!(events[1].subject.as_deref(), Some("storage"));
}

#[test]
fn every_event_leaves_one_stable_line_for_the_journal() {
    let (_root, mut server) = runtime("log-lines", 912);
    let lines = capture_lines(&mut server);
    server.effects(vec![Effect::DisplayClockOverflow]).unwrap();
    // The line that used to be printed apart is now the event's own, once.
    assert_eq!(
        *lines.lock().unwrap(),
        ["rad-runtime: display_clock_overflow"]
    );
}

#[test]
fn the_editor_reads_the_recorded_events_without_consuming_or_adding_any() {
    let (root, mut server) = runtime("log-read", 913);
    server.effects(vec![Effect::DisplayClockOverflow]).unwrap();
    let mut editor = connect(&root, "editor.sock");
    assert_eq!(ask(&mut server, &mut editor, 1, "hello")["status"], "ok");
    let first = ask(&mut server, &mut editor, 2, "runtime_log");
    assert_eq!(first["status"], "ok", "{first}");
    let entries = first["data"]["entries"].as_array().unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|e| e["code"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["started", "display_clock_overflow"]
    );
    // Integers are decimal strings; a subject is left out when there is none.
    assert_eq!(entries[0]["seq"], "1");
    assert_eq!(entries[1]["seq"], "2");
    assert!(
        entries[0]["at_unix_ms"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .is_ok()
    );
    assert!(entries[0].get("subject").is_none());
    assert_eq!(first["data"]["dropped"], "0");
    // Asking is neither a mutation nor an event.
    let second = ask(&mut server, &mut editor, 3, "runtime_log");
    assert_eq!(second["data"], first["data"]);
}

#[test]
fn automation_cannot_read_the_runtime_log() {
    let (root, mut server) = runtime("log-denied", 914);
    let mut automation = connect(&root, "automation.sock");
    ask(&mut server, &mut automation, 1, "hello");
    let denied = ask(&mut server, &mut automation, 2, "runtime_log");
    assert_eq!(denied["status"], "error");
    assert_eq!(denied["error"]["code"], "permission_denied");
    assert!(denied["data"].is_null() || denied.get("data").is_none());
}

#[test]
fn shutting_down_is_recorded_once() {
    let (_root, mut server) = runtime("log-shutdown", 915);
    server.request_shutdown();
    server.request_shutdown();
    assert_eq!(
        codes(&server),
        [EventCode::Started, EventCode::ShutdownRequested]
    );
}

struct Port(Arc<Mutex<VecDeque<rad_deck::Event>>>);
impl crate::display::DeckPort for Port {
    fn next_event(&mut self) -> Option<rad_deck::Event> {
        self.0.lock().unwrap().pop_front()
    }
    fn set_desired(&mut self, _: rad_deck::Desired) -> Result<(), rad_deck::DeckError> {
        Ok(())
    }
    fn shutdown(&mut self) {}
    fn is_stopped(&self) -> bool {
        true
    }
}

#[test]
fn a_change_in_the_deck_is_recorded_once_and_a_repeat_is_not() {
    use rad_deck::{DeckError, Event};
    let (_root, mut server) = runtime("log-deck", 916);
    let events = Arc::new(Mutex::new(VecDeque::new()));
    server
        .attach_deck_port(Box::new(Port(events.clone())))
        .unwrap();
    for (event, expected) in [
        (Event::Connected, Some(EventCode::DeckConnected)),
        (Event::Connected, None),
        (
            Event::Disconnected(DeckError::Absent),
            Some(EventCode::DeckDisconnected),
        ),
        (Event::Disconnected(DeckError::Absent), None),
        (
            Event::Disconnected(DeckError::Io),
            Some(EventCode::DeckInaccessible),
        ),
        (Event::Disconnected(DeckError::Io), None),
        (Event::Connected, Some(EventCode::DeckConnected)),
    ] {
        let before = codes(&server).len();
        events.lock().unwrap().push_back(event.clone());
        server.turn().unwrap();
        assert_eq!(
            codes(&server)[before..].to_vec(),
            expected.into_iter().collect::<Vec<_>>(),
            "{event:?}"
        );
    }
}

fn uncertain_server(name: &str, session: u128) -> (Directory, Server, rad_authority::CommitId) {
    use rad_core::configuration::{Edit, SettingsData};
    let (root, mut server) = runtime(name, session);
    let ticket = server.actor.reserve(7, Origin::Editor).unwrap();
    let effects = server.actor.submit(
        ticket,
        vec![Edit::SetSettings(SettingsData {
            brightness_percent: 33,
            ..SettingsData::default()
        })],
    );
    let id = effects
        .iter()
        .find_map(|e| match e {
            Effect::Persist { id, .. } => Some(*id),
            _ => None,
        })
        .unwrap();
    let unknown = server.actor.complete(id, rad_authority::Outcome::Uncertain);
    server.effects(unknown).unwrap();
    (root, server, id)
}

#[test]
fn storage_going_uncertain_and_coming_back_is_recorded() {
    let (_root, mut server, _) = uncertain_server("log-storage", 917);
    assert_eq!(
        codes(&server),
        [EventCode::Started, EventCode::StorageUncertain]
    );
    let reconcile = server.reconcile_once();
    let id = reconcile
        .iter()
        .find_map(|e| match e {
            Effect::Reconcile { id } => Some(*id),
            _ => None,
        })
        .unwrap();
    let applied = server.actor.complete(
        id,
        rad_authority::Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    server.effects(applied).unwrap();
    assert_eq!(
        codes(&server),
        [
            EventCode::Started,
            EventCode::StorageUncertain,
            EventCode::StorageReady
        ]
    );
}

#[test]
fn storage_that_needs_recovery_is_recorded() {
    let (_root, mut server, _) = uncertain_server("log-recovery", 918);
    let id = server
        .reconcile_once()
        .iter()
        .find_map(|e| match e {
            Effect::Reconcile { id } => Some(*id),
            _ => None,
        })
        .unwrap();
    let effects = server
        .actor
        .complete(id, rad_authority::Outcome::RecoveryRequired);
    server.effects(effects).unwrap();
    assert_eq!(
        codes(&server).last(),
        Some(&EventCode::StorageRecoveryRequired)
    );
}
