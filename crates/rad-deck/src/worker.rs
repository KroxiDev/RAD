use crate::{DeckError, Desired, DeviceConfig, KeyVisual, hidraw::Link, protocol, render};
use std::{
    collections::VecDeque,
    os::{fd::AsRawFd, unix::net::UnixDatagram},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Connected,
    Report([bool; 32]),
    Disconnected(DeckError),
    Stopped { logo_restored: bool },
}
type Connector = Box<dyn FnMut() -> Result<Box<dyn Link>, DeckError> + Send>;
static LIVE: AtomicBool = AtomicBool::new(false);
struct State {
    desired: Arc<Desired>,
    events: VecDeque<Event>,
    stop: bool,
    done: bool,
}
struct Shared {
    state: Mutex<State>,
    wake: Arc<mio::Waker>,
}
pub struct Deck {
    shared: Arc<Shared>,
    control: UnixDatagram,
}
impl Deck {
    pub fn spawn(
        _config: DeviceConfig,
        initial: Desired,
        wake: Arc<mio::Waker>,
    ) -> Result<Self, DeckError> {
        Self::spawn_connector(
            initial,
            wake,
            Box::new(|| {
                crate::hidraw::connect_at(
                    std::path::Path::new("/sys"),
                    std::path::Path::new("/dev"),
                )
            }),
        )
    }
    fn spawn_connector(
        initial: Desired,
        wake: Arc<mio::Waker>,
        connector: Connector,
    ) -> Result<Self, DeckError> {
        if LIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(DeckError::Unavailable);
        }
        let result = (|| {
            let (control, receiver) = UnixDatagram::pair().map_err(|_| DeckError::Io)?;
            control.set_nonblocking(true).map_err(|_| DeckError::Io)?;
            receiver.set_nonblocking(true).map_err(|_| DeckError::Io)?;
            let shared = Arc::new(Shared {
                state: Mutex::new(State {
                    desired: Arc::new(initial),
                    events: VecDeque::new(),
                    stop: false,
                    done: false,
                }),
                wake,
            });
            let thread_shared = shared.clone();
            std::thread::Builder::new()
                .name("rad-deck".into())
                .spawn(move || run(thread_shared, receiver, connector))
                .map_err(|_| DeckError::Io)?;
            Ok(Self { shared, control })
        })();
        if result.is_err() {
            LIVE.store(false, Ordering::Release);
        }
        result
    }
    pub fn set_desired(&self, d: Desired) -> Result<(), DeckError> {
        let old = {
            let mut s = self
                .shared
                .state
                .lock()
                .map_err(|_| DeckError::Unavailable)?;
            if s.stop || s.done {
                return Err(DeckError::Unavailable);
            }
            std::mem::replace(&mut s.desired, Arc::new(d))
        };
        drop(old);
        self.notify()
    }
    fn notify(&self) -> Result<(), DeckError> {
        match self.control.send(&[1]) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            _ => Err(DeckError::Io),
        }
    }
    pub fn next_event(&self) -> Option<Event> {
        self.shared.state.lock().ok()?.events.pop_front()
    }
    pub fn shutdown(&self) {
        if let Ok(mut s) = self.shared.state.lock() {
            s.stop = true;
        }
        let _ = self.notify();
    }
    pub fn is_stopped(&self) -> bool {
        self.shared.state.lock().is_ok_and(|s| s.done)
    }
    pub fn can_spawn() -> bool {
        !LIVE.load(Ordering::Acquire)
    }
}
impl Drop for Deck {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn emit(shared: &Shared, event: Event) -> bool {
    let Ok(mut s) = shared.state.lock() else {
        return false;
    };
    let accepted = s.events.len() < 64;
    if !accepted {
        s.events.clear();
        s.events.push_back(Event::Disconnected(DeckError::Overflow));
        if matches!(event, Event::Stopped { .. }) {
            s.events.push_back(event);
        }
    } else {
        s.events.push_back(event);
    }
    drop(s);
    let _ = shared.wake.wake();
    accepted
}
struct Upload {
    key: usize,
    visual: KeyVisual,
    packets: Vec<[u8; 1024]>,
    next: usize,
}
fn wait(control: &UnixDatagram, link: Option<&dyn Link>, timeout: i32) -> Result<(), DeckError> {
    let mut fds = [
        libc::pollfd {
            fd: control.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: link.map_or(-1, Link::raw_fd),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    // SAFETY: two initialized pollfd values live for the duration of poll; fds remain owned.
    let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
    if n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
        return Err(DeckError::Io);
    }
    let mut b = [0; 1];
    for _ in 0..64 {
        if control.recv(&mut b).is_err() {
            break;
        }
    }
    Ok(())
}
fn run(shared: Arc<Shared>, control: UnixDatagram, mut connect: Connector) {
    // If a panic/kernel block prevents orderly completion LIVE stays set: no replacement.
    let mut link: Option<Box<dyn Link>> = None;
    let mut sent: [Option<KeyVisual>; 32] = std::array::from_fn(|_| None);
    let mut brightness = None;
    let mut upload: Option<Upload> = None;
    let mut last_error = None;
    let mut logo = true;
    loop {
        let (desired, stop) = match shared.state.lock() {
            Ok(s) => (s.desired.clone(), s.stop),
            Err(_) => break,
        };
        if stop {
            if let Some(mut device) = link.take() {
                let bright = device
                    .feature(protocol::feature(8, desired.brightness as i32))
                    .is_ok();
                logo = device.close() && bright;
            }
            break;
        }
        if link.is_none() {
            match connect() {
                Ok(device) => {
                    link = Some(device);
                    sent = std::array::from_fn(|_| None);
                    brightness = None;
                    last_error = None;
                    if !emit(&shared, Event::Connected) {
                        if let Some(mut d) = link.take() {
                            logo = d.close();
                        }
                        continue;
                    }
                }
                Err(e) => {
                    if last_error != Some(e) {
                        emit(&shared, Event::Disconnected(e));
                        last_error = Some(e);
                    }
                    if wait(&control, None, 500).is_err() {
                        break;
                    }
                    continue;
                }
            }
        }
        let device = link.as_mut().unwrap();
        let result = (|| -> Result<bool, DeckError> {
            for _ in 0..32 {
                match device.read()? {
                    Some(keys) => {
                        if !emit(&shared, Event::Report(keys)) {
                            return Err(DeckError::Overflow);
                        }
                    }
                    None => break,
                }
            }
            let wanted = if desired.asleep {
                0
            } else {
                desired.brightness
            };
            if brightness != Some(wanted) {
                device.feature(protocol::feature(8, wanted as i32))?;
                brightness = Some(wanted);
            }
            if upload.is_none() {
                if let Some(key) = (0..32).find(|i| sent[*i].as_ref() != Some(&desired.keys[*i])) {
                    let visual = desired.keys[key].clone();
                    let jpeg = render::jpeg(&visual)?;
                    upload = Some(Upload {
                        key,
                        visual,
                        packets: protocol::packets(key as u8, &jpeg)?,
                        next: 0,
                    });
                } else {
                    return Ok(false);
                }
            }
            let current = upload.as_mut().unwrap();
            if !device.write(&current.packets[current.next])? {
                wait(&control, Some(device.as_ref()), 5)?;
                return Ok(true);
            }
            current.next += 1;
            if current.next == current.packets.len() {
                let complete = upload.take().unwrap();
                sent[complete.key] = Some(complete.visual);
            }
            Ok(true)
        })();
        match result {
            Ok(true) => continue,
            Ok(false) => {
                if wait(&control, Some(device.as_ref()), -1).is_err() {
                    break;
                }
            }
            Err(e) => {
                if let Some(mut d) = link.take() {
                    logo = d.close();
                }
                upload = None;
                last_error = Some(e);
                if e != DeckError::Overflow {
                    emit(&shared, Event::Disconnected(e));
                }
                if wait(&control, None, 500).is_err() {
                    break;
                }
            }
        }
    }
    if let Some(mut d) = link.take() {
        logo = d.close();
    }
    emit(
        &shared,
        Event::Stopped {
            logo_restored: logo,
        },
    );
    if let Ok(mut s) = shared.state.lock() {
        s.done = true;
    }
    LIVE.store(false, Ordering::Release);
    let _ = shared.wake.wake();
}
#[cfg(test)]
mod tests;
