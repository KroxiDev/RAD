use rad_client::connection::{Client, Endpoint};
use rad_deck::{DeckError, Desired, Event, KeyVisual, TextPlacement};
use rad_runtime::{display::DeckPort, server::Server};
use serde_json::json;
use std::{
    collections::VecDeque,
    num::NonZeroU128,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
#[derive(Default)]
struct State {
    events: VecDeque<Event>,
    desired: Option<Desired>,
    stopped: bool,
}
struct Port(Arc<Mutex<State>>);
impl DeckPort for Port {
    fn next_event(&mut self) -> Option<Event> {
        self.0.lock().unwrap().events.pop_front()
    }
    fn set_desired(&mut self, d: Desired) -> Result<(), DeckError> {
        self.0.lock().unwrap().desired = Some(d);
        Ok(())
    }
    fn shutdown(&mut self) {
        self.0.lock().unwrap().stopped = true;
    }
    fn is_stopped(&self) -> bool {
        self.0.lock().unwrap().stopped
    }
}
struct Directory(std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn physical_reports_navigate_and_reconnect_repaints_without_replaying() {
    let root =
        Directory(std::env::temp_dir().join(format!("rad-server-deck-{}", std::process::id())));
    std::fs::create_dir(&root.0).unwrap();
    std::fs::set_permissions(&root.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut server = Server::new(
        &root.0,
        &root.0.join("data"),
        NonZeroU128::new(421).unwrap(),
    )
    .unwrap();
    let state = Arc::new(Mutex::new(State::default()));
    server
        .attach_deck_port(Box::new(Port(state.clone())))
        .unwrap();
    let path = root.0.clone();
    let (send, receive) = mpsc::sync_channel(1);
    let client = std::thread::spawn(move || {
        let mut c = Client::connect(&path, Endpoint::Editor).unwrap();
        assert!(
            c.request(
                "create_folder",
                json!({"parent_id":"f1","position":2,"name":"Tools"})
            )
            .unwrap()
            .error
            .is_none()
        );
        send.send(()).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    while receive.try_recv().is_err() {
        server.turn().unwrap();
        assert!(Instant::now() < deadline);
        server.wait(Duration::from_millis(10)).unwrap();
    }
    client.join().unwrap();
    let mut keys = std::array::from_fn(|_| KeyVisual::default());
    keys[2] = KeyVisual::new("Tools", None, false, false).text_at(TextPlacement::Middle);
    server.turn().unwrap();
    assert_eq!(
        state.lock().unwrap().desired,
        Some(Desired::new(keys.clone(), 100, false).unwrap())
    );
    let mut pressed = [false; 32];
    pressed[2] = true;
    state.lock().unwrap().events.extend([
        Event::Connected,
        Event::Report([false; 32]),
        Event::Report(pressed),
    ]);
    server.turn().unwrap();
    let mut inside = std::array::from_fn(|_| KeyVisual::default());
    inside[0] = KeyVisual::new("Return", None, false, false).text_at(TextPlacement::Middle);
    assert_eq!(
        state.lock().unwrap().desired,
        Some(Desired::new(inside.clone(), 100, false).unwrap())
    );
    state.lock().unwrap().events.extend([
        Event::Disconnected(DeckError::Absent),
        Event::Connected,
        Event::Report([false; 32]),
    ]);
    server.turn().unwrap();
    assert_eq!(
        state.lock().unwrap().desired,
        Some(Desired::new(inside, 100, false).unwrap())
    );
    let mut back = [false; 32];
    back[0] = true;
    state.lock().unwrap().events.push_back(Event::Report(back));
    server.turn().unwrap();
    assert_eq!(
        state.lock().unwrap().desired,
        Some(Desired::new(keys, 100, false).unwrap())
    );
    server.request_shutdown();
    let end = Instant::now() + Duration::from_secs(2);
    while !server.shutdown_complete() {
        server.turn().unwrap();
        assert!(
            Instant::now() < end,
            "shutdown must wait for owned workers then finish"
        );
        server.wait(Duration::from_millis(10)).unwrap();
    }
    assert!(
        state.lock().unwrap().stopped,
        "full runtime exit must ask deck cleanup, unlike editor closure"
    );
}
