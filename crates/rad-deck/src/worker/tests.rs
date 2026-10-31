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
