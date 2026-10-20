use super::*;
use crate::{KeyVisual, protocol};
use std::{
    os::{fd::AsRawFd, unix::net::UnixDatagram},
    sync::Mutex,
    time::{Duration, Instant},
};
static SERIAL: Mutex<()> = Mutex::new(());
#[derive(Default)]
struct State {
    writes: Vec<[u8; 1024]>,
    features: Vec<[u8; 32]>,
    peers: Vec<UnixDatagram>,
    closed: usize,
    online: bool,
    block: bool,
    fail_close: bool,
}
struct Fake {
    state: Arc<Mutex<State>>,
    socket: UnixDatagram,
}
impl Link for Fake {
    fn raw_fd(&self) -> i32 {
        self.socket.as_raw_fd()
    }
    fn read(&mut self) -> Result<Option<[bool; 32]>, DeckError> {
        if !self.state.lock().unwrap().online {
            return Err(DeckError::Absent);
        }
        let mut b = [0; 512];
        match self.socket.recv(&mut b) {
            Ok(n) => protocol::parse(&b[..n]).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            _ => Err(DeckError::Io),
        }
    }
    fn write(&mut self, p: &[u8; 1024]) -> Result<bool, DeckError> {
        let mut s = self.state.lock().unwrap();
        if s.block {
            return Ok(false);
        }
        s.writes.push(*p);
        Ok(true)
    }
    fn feature(&mut self, p: [u8; 32]) -> Result<(), DeckError> {
        self.state.lock().unwrap().features.push(p);
        Ok(())
    }
    fn close(&mut self) -> bool {
        let mut s = self.state.lock().unwrap();
        s.closed += 1;
        !s.fail_close
    }
}
fn desired(brightness: u8, asleep: bool) -> Desired {
    Desired::new(
        std::array::from_fn(|_| KeyVisual::default()),
        brightness,
        asleep,
    )
    .unwrap()
}
fn fixture() -> (Arc<Mutex<State>>, Connector, mio::Poll, Arc<mio::Waker>) {
    let state = Arc::new(Mutex::new(State {
        online: true,
        ..Default::default()
    }));
    let s = state.clone();
    let connect: Connector = Box::new(move || {
        let mut state = s.lock().unwrap();
        if !state.online {
            return Err(DeckError::Absent);
        }
        let (a, b) = UnixDatagram::pair().unwrap();
        a.set_nonblocking(true).unwrap();
        b.set_nonblocking(true).unwrap();
        state.peers.push(b);
        Ok(Box::new(Fake {
            state: s.clone(),
            socket: a,
        }))
    });
    let poll = mio::Poll::new().unwrap();
    let wake = Arc::new(mio::Waker::new(poll.registry(), mio::Token(7)).unwrap());
    (state, connect, poll, wake)
}
fn until(mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(3);
    while !f() {
        assert!(Instant::now() < end, "worker fixture timeout");
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn worker_paints_32_emits_raw_reports_wakes_and_cleans_up() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    until(Deck::can_spawn);
    let (s, c, mut poll, w) = fixture();
    let deck = Deck::spawn_connector(desired(73, false), w, c).unwrap();
    until(|| {
        s.lock()
            .unwrap()
            .writes
            .iter()
            .filter(|p| p[3] == 1)
            .count()
            == 32
    });
    assert_eq!(deck.next_event(), Some(Event::Connected));
    assert!(
        s.lock()
            .unwrap()
            .features
            .contains(&protocol::feature(8, 73))
    );
    let mut b = [0; 512];
    b[..4].copy_from_slice(&[1, 0, 32, 0]);
    b[4] = 1;
    s.lock().unwrap().peers[0].send(&b).unwrap();
    let mut event = None;
    until(|| {
        event = deck.next_event();
        event.is_some()
    });
    assert!(matches!(event,Some(Event::Report(keys)) if keys[0]));
    let mut events = mio::Events::with_capacity(4);
    poll.poll(&mut events, Some(Duration::from_secs(1)))
        .unwrap();
    assert!(!events.is_empty());
    deck.shutdown();
    until(|| deck.is_stopped());
    assert_eq!(s.lock().unwrap().closed, 1);
    assert_eq!(
        deck.next_event(),
        Some(Event::Stopped {
            logo_restored: true
        })
    );
    until(Deck::can_spawn);
}
#[test]
fn shutdown_event_is_not_lost_when_report_mailbox_is_full() {
    let (_s, _c, _poll, w) = fixture();
    let shared = Shared {
        state: Mutex::new(super::State {
            desired: Arc::new(desired(100, false)),
            events: VecDeque::new(),
            stop: false,
            done: false,
        }),
        wake: w,
    };
    for _ in 0..64 {
        assert!(emit(&shared, Event::Report([false; 32])));
    }
    emit(
        &shared,
        Event::Stopped {
            logo_restored: false,
        },
    );
    let s = shared.state.lock().unwrap();
    assert!(s.events.contains(&Event::Stopped {
        logo_restored: false
    }));
    assert!(s.events.contains(&Event::Disconnected(DeckError::Overflow)));
    assert!(s.events.len() <= 64);
}
#[test]
fn latest_coalesces_reconnect_repaints_and_sleep_never_consumes_input() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    until(Deck::can_spawn);
    let (s, c, _poll, w) = fixture();
    s.lock().unwrap().online = false;
    let deck = Deck::spawn_connector(desired(100, false), w.clone(), c).unwrap();
    until(|| deck.next_event() == Some(Event::Disconnected(DeckError::Absent)));
    for brightness in 1..=40 {
        deck.set_desired(desired(brightness, false)).unwrap();
    }
    deck.set_desired(desired(40, true)).unwrap();
    s.lock().unwrap().online = true;
    deck.set_desired(desired(40, true)).unwrap();
    until(|| {
        s.lock()
            .unwrap()
            .writes
            .iter()
            .filter(|p| p[3] == 1)
            .count()
            == 32
    });
    assert!(
        s.lock()
            .unwrap()
            .features
            .contains(&protocol::feature(8, 0))
    );
    assert_eq!(deck.next_event(), Some(Event::Connected));
    let mut report = [0; 512];
    report[..4].copy_from_slice(&[1, 0, 32, 0]);
    report[7] = 1;
    s.lock().unwrap().peers[0].send(&report).unwrap();
    let mut event = None;
    until(|| {
        event = deck.next_event();
        event.is_some()
    });
    assert!(matches!(event,Some(Event::Report(k)) if k[3]));
    deck.set_desired(desired(40, false)).unwrap();
    until(|| {
        s.lock()
            .unwrap()
            .features
            .contains(&protocol::feature(8, 40))
    });
    assert_eq!(
        s.lock()
            .unwrap()
            .writes
            .iter()
            .filter(|p| p[3] == 1)
            .count(),
        32
    );
    s.lock().unwrap().online = false;
    deck.set_desired(desired(40, false)).unwrap();
    until(|| deck.next_event() == Some(Event::Disconnected(DeckError::Absent)));
    s.lock().unwrap().online = true;
    deck.set_desired(desired(40, false)).unwrap();
    until(|| {
        s.lock()
            .unwrap()
            .writes
            .iter()
            .filter(|p| p[3] == 1)
            .count()
            == 64
    });
    assert_eq!(deck.next_event(), Some(Event::Connected));
    assert!(deck.next_event().is_none());
    deck.shutdown();
    until(|| deck.is_stopped());
    assert_eq!(s.lock().unwrap().closed, 2);
}
#[test]
fn blocked_output_keeps_one_worker_and_shutdown_does_not_wait_for_render_queue() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    until(Deck::can_spawn);
    let (s, c, _poll, w) = fixture();
    s.lock().unwrap().block = true;
    let deck = Deck::spawn_connector(desired(100, false), w.clone(), c).unwrap();
    until(|| deck.next_event() == Some(Event::Connected));
    assert!(!Deck::can_spawn());
    let (_other, c, _p, _w) = fixture();
    assert!(matches!(
        Deck::spawn_connector(desired(100, false), w, c),
        Err(DeckError::Unavailable)
    ));
    for n in 0..100 {
        deck.set_desired(desired(n % 100, false)).unwrap();
    }
    assert!(s.lock().unwrap().writes.is_empty());
    deck.shutdown();
    until(|| deck.is_stopped());
    until(Deck::can_spawn);
    assert_eq!(s.lock().unwrap().closed, 1);
    assert_eq!(
        deck.set_desired(desired(100, false)),
        Err(DeckError::Unavailable)
    );
}

#[test]
fn display_update_sends_only_changed_key() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    until(Deck::can_spawn);
    let (s, c, _poll, w) = fixture();
    let start = Instant::now();
    let mut keys = std::array::from_fn(|_| KeyVisual::new("RAD", None, false, true));
    let deck = Deck::spawn_connector(Desired::new(keys.clone(), 70, false).unwrap(), w, c).unwrap();
    until(|| {
        s.lock()
            .unwrap()
            .writes
            .iter()
            .filter(|p| p[3] == 1)
            .count()
            == 32
    });
    eprintln!(
        "synthetic 32 text/generic JPEG renders + fake writes: {:?}",
        start.elapsed()
    );
    s.lock().unwrap().writes.clear();
    keys[17] = KeyVisual::new("Nuevo", None, true, true);
    deck.set_desired(Desired::new(keys, 70, false).unwrap())
        .unwrap();
    until(|| s.lock().unwrap().writes.iter().any(|p| p[3] == 1));
    deck.shutdown();
    until(|| deck.is_stopped());
    let state = s.lock().unwrap();
    assert!(state.writes.iter().all(|p| p[2] == 17));
    assert_eq!(state.writes.iter().filter(|p| p[3] == 1).count(), 1);
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status
            .lines()
            .filter(|s| s.starts_with("VmHWM:") || s.starts_with("VmRSS:"))
        {
            eprintln!("synthetic test process {line}");
        }
    }
}

#[test]
fn cleanup_failure_is_reported_after_disconnect_and_stop() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    until(Deck::can_spawn);
    let (s, c, _poll, w) = fixture();
    let deck = Deck::spawn_connector(desired(70, false), w, c).unwrap();
    until(|| deck.next_event() == Some(Event::Connected));
    {
        let mut state = s.lock().unwrap();
        state.fail_close = true;
        state.online = false;
    }
    deck.set_desired(desired(70, false)).unwrap();
    until(|| deck.next_event() == Some(Event::Disconnected(DeckError::Absent)));
    deck.shutdown();
    until(|| deck.is_stopped());
    assert_eq!(
        deck.next_event(),
        Some(Event::Stopped {
            logo_restored: false
        })
    );
}
