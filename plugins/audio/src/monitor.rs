use crate::{
    Pactl, Snapshot, Timing,
    server::{self, Failure},
};
use std::{
    io::Read,
    panic::{AssertUnwindSafe, catch_unwind},
    process::{Child, ChildStdout},
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread,
    time::{Duration, Instant},
};

const MAX_LINE: usize = 256;

pub(crate) type Publish = Box<dyn Fn(u64, Result<Snapshot, Failure>) + Send + Sync + 'static>;

const MIN_RETRY_WAIT: Duration = Duration::from_millis(1);

struct Backoff {
    first: Duration,
    most: Duration,
    next: Duration,
}

impl Backoff {
    fn new(timing: &Timing) -> Self {
        let first = timing.retry_wait.max(MIN_RETRY_WAIT);
        Self {
            first,
            most: timing.retry_max.max(first),
            next: first,
        }
    }

    fn next(&mut self) -> Duration {
        let wait = self.next;
        self.next = wait.saturating_mul(2).min(self.most);
        wait
    }

    fn reset(&mut self) {
        self.next = self.first;
    }
}

struct Shared {
    pactl: Pactl,
    timing: Timing,
    publish: Publish,
    control: Control,
}

pub(crate) struct Monitor {
    shared: Arc<Shared>,
}

impl Monitor {
    pub(crate) fn new(pactl: Pactl, timing: Timing, publish: Publish) -> Self {
        Self {
            shared: Arc::new(Shared {
                pactl,
                timing,
                publish,
                control: Control::default(),
            }),
        }
    }

    pub(crate) fn start(&self) -> Option<u64> {
        let control = &self.shared.control;
        let mut state = control.lock();
        if !state.wanted {
            state.wanted = true;
            state.epoch += 1;
            control.wake.notify_all();
        }
        if !state.running {
            state.running = true;
            let shared = Arc::clone(&self.shared);
            let spawned = thread::Builder::new()
                .name("audio-monitor".into())
                .spawn(move || thread_main(&shared));
            if spawned.is_err() {
                state.running = false;
                state.wanted = false;
                return None;
            }
        }
        Some(state.epoch)
    }

    pub(crate) fn refresh(&self) {
        self.shared.control.event();
    }

    pub(crate) fn stop(&self) {
        self.shared.control.lock().wanted = false;
        self.shared.control.wake.notify_all();
    }
}

#[derive(Default)]
struct Control {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    wanted: bool,
    epoch: u64,
    running: bool,
    ended: bool,
    pending: Option<(Instant, Instant)>,
}

enum Wake {
    Stop,
    Events,
    Ended,
    Retry,
}

impl Control {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn event(&self) {
        let now = Instant::now();
        let mut state = self.lock();
        let first = state.pending.map_or(now, |(first, _)| first);
        state.pending = Some((first, now));
        self.wake.notify_all();
    }

    fn end(&self) {
        self.lock().ended = true;
        self.wake.notify_all();
    }
}

struct Lease<'a> {
    control: &'a Control,
    epoch: u64,
}

impl Lease<'_> {
    fn over(&self, state: &State) -> bool {
        !state.wanted || state.epoch != self.epoch
    }

    fn is_over(&self) -> bool {
        self.over(&self.control.lock())
    }

    fn reset(&self) {
        let mut state = self.control.lock();
        state.ended = false;
        state.pending = None;
    }

    fn wait(&self, retry_at: Option<Instant>) -> Wake {
        let mut state = self.control.lock();
        loop {
            if self.over(&state) {
                return Wake::Stop;
            }
            if state.pending.is_some() {
                return Wake::Events;
            }
            if state.ended {
                return Wake::Ended;
            }
            let Some(retry_at) = retry_at else {
                state = self
                    .control
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            };
            let left = retry_at.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Wake::Retry;
            }
            state = self
                .control
                .wake
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn settle(&self, quiet: Duration, most: Duration) -> bool {
        let mut state = self.control.lock();
        loop {
            if self.over(&state) {
                return false;
            }
            let Some((first, last)) = state.pending else {
                return true;
            };
            let end = (last + quiet).min(first + most);
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() || state.ended {
                state.pending = None;
                return true;
            }
            state = self
                .control
                .wake
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn sleep(&self, duration: Duration) -> bool {
        let end = Instant::now().checked_add(duration);
        let mut state = self.control.lock();
        while !self.over(&state) {
            let Some(end) = end else {
                state = self
                    .control
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            };
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            state = self
                .control
                .wake
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        false
    }
}

fn thread_main(shared: &Arc<Shared>) {
    let mut backoff = Backoff::new(&shared.timing);
    loop {
        let epoch = {
            let mut state = shared.control.lock();
            if !state.wanted {
                state.running = false;
                return;
            }
            state.epoch
        };
        let lease = Lease {
            control: &shared.control,
            epoch,
        };
        match catch_unwind(AssertUnwindSafe(|| watch(shared, &lease, &mut backoff))) {
            Ok(()) => backoff.reset(),
            Err(_) => {
                lease.sleep(backoff.next());
            }
        }
    }
}

struct EventStream {
    child: Child,
    reader: Option<thread::JoinHandle<()>>,
}

impl Drop for EventStream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn watch(shared: &Arc<Shared>, lease: &Lease<'_>, backoff: &mut Backoff) {
    let Shared {
        pactl,
        timing,
        publish,
        ..
    } = &**shared;
    let publish = |picture| publish(lease.epoch, picture);
    while !lease.is_over() {
        lease.reset();
        let mut scheduled = None;
        if let Ok((child, stdout)) = pactl.subscribe() {
            let mut stream = EventStream {
                child,
                reader: None,
            };
            let reader_shared = Arc::clone(shared);
            stream.reader = thread::Builder::new()
                .name("audio-events".into())
                .spawn(move || read_events(stdout, &reader_shared.control))
                .ok();
            if stream.reader.is_some() {
                scheduled = follow(pactl, timing, lease, &publish, backoff);
            }
        }
        if lease.is_over() {
            return;
        }
        publish(Err(Failure::Unavailable));
        if !lease.sleep(scheduled.unwrap_or_else(|| backoff.next())) {
            return;
        }
    }
}

fn refresh(pactl: &Pactl, timing: &Timing) -> Result<Snapshot, Failure> {
    server::snapshot(pactl, Instant::now() + timing.refresh_deadline)
}

fn follow(
    pactl: &Pactl,
    timing: &Timing,
    lease: &Lease<'_>,
    publish: &dyn Fn(Result<Snapshot, Failure>),
    backoff: &mut Backoff,
) -> Option<Duration> {
    let mut retry = None::<(Instant, Duration)>;
    let mut retry_at = Some(Instant::now());
    loop {
        match lease.wait(retry_at) {
            Wake::Stop => return None,
            Wake::Ended => return retry.map(|(_, wait)| wait),
            Wake::Retry => {}
            Wake::Events => {
                if !lease.settle(timing.settle, timing.max_settle) {
                    return None;
                }
            }
        }
        let picture = refresh(pactl, timing);
        let worked = picture.is_ok();
        if worked {
            retry = None;
        } else {
            let wait = backoff.next();
            retry = Some((Instant::now() + wait, wait));
        }
        retry_at = Some(retry.map_or_else(|| Instant::now() + timing.recheck, |(at, _)| at));
        publish(picture);
        if worked {
            backoff.reset();
        }
    }
}

fn read_events(mut stdout: ChildStdout, control: &Control) {
    let mut line = Vec::new();
    let mut too_long = false;
    let mut chunk = [0u8; 1024];
    loop {
        match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                for &byte in &chunk[..n] {
                    if byte == b'\n' {
                        if !too_long && is_device_event(&line) {
                            control.event();
                        }
                        line.clear();
                        too_long = false;
                    } else if line.len() < MAX_LINE {
                        line.push(byte);
                    } else {
                        too_long = true;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    control.end();
}

fn is_device_event(line: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(line) else {
        return false;
    };
    let Some(rest) = text.strip_prefix("Event '") else {
        return false;
    };
    let Some((_, after)) = rest.split_once("' on ") else {
        return false;
    };
    matches!(
        after.split([' ', '#']).next(),
        Some("sink" | "source" | "server")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Dir, alive, all_gone, emit_event, fake, gone, pids};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn wait_until(within: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + within;
        while !condition() {
            if Instant::now() >= end {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
        true
    }

    #[test]
    fn the_wait_still_grows_when_the_first_one_is_zero() {
        let timing = Timing::default()
            .retry_wait(Duration::ZERO)
            .retry_max(Duration::from_millis(50));
        let mut backoff = Backoff::new(&timing);
        let waits: Vec<_> = (0..10).map(|_| backoff.next()).collect();
        assert!(waits[0] > Duration::ZERO, "{waits:?}");
        assert!(
            waits.windows(2).all(|w| w[0] <= w[1]),
            "never shorter: {waits:?}"
        );
        assert!(waits[3] > waits[0], "it grows: {waits:?}");
        assert_eq!(
            waits[9],
            Duration::from_millis(50),
            "up to the limit: {waits:?}"
        );
    }

    #[test]
    fn the_wait_doubles_to_the_limit_and_starts_over_on_reset() {
        let timing = Timing::default()
            .retry_wait(Duration::from_millis(30))
            .retry_max(Duration::from_millis(200));
        let mut backoff = Backoff::new(&timing);
        let ms = |d: Duration| d.as_millis();
        let waits: Vec<_> = (0..6).map(|_| ms(backoff.next())).collect();
        assert_eq!(waits, [30, 60, 120, 200, 200, 200]);
        backoff.reset();
        assert_eq!(ms(backoff.next()), 30);
        let timing = timing.retry_max(Duration::from_millis(5));
        let mut backoff = Backoff::new(&timing);
        assert_eq!((ms(backoff.next()), ms(backoff.next())), (30, 30));
    }

    fn always_panicking(dir: &Dir, timing: Timing) -> (Monitor, Arc<Mutex<Vec<Instant>>>) {
        let pactl = Pactl::new(fake(), Some(dir.0.clone().into_os_string()));
        let panics = Arc::new(Mutex::new(Vec::new()));
        let publish: Publish = {
            let panics = Arc::clone(&panics);
            Box::new(move |_, _| {
                panics.lock().unwrap().push(Instant::now());
                panic!("injected panic in a watcher");
            })
        };
        (Monitor::new(pactl, timing, publish), panics)
    }

    fn stop_and_check_no_child_is_left(monitor: &Monitor, dir: &Dir) {
        monitor.stop();
        assert!(
            wait_until(Duration::from_secs(3), || all_gone(dir, "subscribe-start ")),
            "event children left behind (running or zombie): {:?}",
            pids(dir, "subscribe-start ")
        );
    }

    fn panics_so_far(panics: &Mutex<Vec<Instant>>) -> usize {
        panics.lock().unwrap().len()
    }

    fn gaps(panics: &Mutex<Vec<Instant>>) -> Vec<Duration> {
        let times = panics.lock().unwrap().clone();
        times.windows(2).map(|pair| pair[1] - pair[0]).collect()
    }

    #[test]
    fn a_wait_too_long_to_double_stays_as_long_instead_of_overflowing() {
        let timing = Timing::default().retry_wait(Duration::MAX);
        let mut backoff = Backoff::new(&timing);
        assert_eq!(backoff.next(), Duration::MAX);
        assert_eq!(backoff.next(), Duration::MAX);
    }

    #[test]
    fn an_absurdly_long_restart_wait_lasts_until_the_stop_and_the_thread_ends_cleanly() {
        let dir = Dir::with_devices("panic-huge-wait");
        let timing = Timing::default().retry_wait(Duration::MAX);
        let (monitor, panics) = always_panicking(&dir, timing);
        monitor.start().unwrap();
        assert!(wait_until(Duration::from_secs(5), || panics_so_far(
            &panics
        ) >= 1));
        stop_and_check_no_child_is_left(&monitor, &dir);
        assert!(
            wait_until(Duration::from_secs(3), || !monitor
                .shared
                .control
                .lock()
                .running),
            "the monitor thread is gone but believes it is running"
        );
    }

    #[test]
    fn a_watcher_that_keeps_panicking_is_restarted_only_after_a_wait() {
        let dir = Dir::with_devices("panic-rhythm");
        let wait = Duration::from_millis(200);
        let timing = Timing::default().retry_wait(wait).retry_max(wait);
        let (monitor, panics) = always_panicking(&dir, timing);
        monitor.start().unwrap();
        assert!(
            wait_until(Duration::from_secs(5), || panics_so_far(&panics) >= 4),
            "{} panics: the monitor gave up",
            panics_so_far(&panics)
        );
        stop_and_check_no_child_is_left(&monitor, &dir);
        for gap in gaps(&panics) {
            assert!(gap >= wait, "restarted after {gap:?}, wait is {wait:?}");
        }
    }

    #[test]
    fn the_wait_between_restarts_grows_while_the_panics_keep_coming() {
        let dir = Dir::with_devices("panic-backoff");
        let timing = Timing::default()
            .retry_wait(Duration::from_millis(100))
            .retry_max(Duration::from_millis(400));
        let (monitor, panics) = always_panicking(&dir, timing);
        monitor.start().unwrap();
        assert!(
            wait_until(Duration::from_secs(5), || panics_so_far(&panics) >= 4),
            "{} panics: the monitor gave up",
            panics_so_far(&panics)
        );
        stop_and_check_no_child_is_left(&monitor, &dir);
        let least = [100, 200, 400].map(Duration::from_millis);
        for (gap, least) in gaps(&panics).into_iter().zip(least) {
            assert!(
                gap >= least,
                "restarted after {gap:?}, at least {least:?} expected"
            );
        }
    }

    #[test]
    fn a_session_that_works_starts_the_waits_after_a_panic_over() {
        let dir = Dir::with_devices("panic-reset");
        let pactl = Pactl::new(fake(), Some(dir.0.clone().into_os_string()));
        let timing = Timing::default()
            .settle(Duration::from_millis(10))
            .retry_wait(Duration::from_millis(200))
            .retry_max(Duration::from_millis(800));
        let failing = Arc::new(AtomicBool::new(true));
        let panics = Arc::new(Mutex::new(Vec::new()));
        let pictures = Arc::new(AtomicUsize::new(0));
        let publish: Publish = {
            let (failing, panics, pictures) = (
                Arc::clone(&failing),
                Arc::clone(&panics),
                Arc::clone(&pictures),
            );
            Box::new(move |_, picture| {
                if failing.load(Ordering::SeqCst) {
                    panics.lock().unwrap().push(Instant::now());
                    panic!("injected panic in a watcher");
                }
                if picture.is_ok() {
                    pictures.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        let monitor = Monitor::new(pactl, timing, publish);
        monitor.start().unwrap();
        assert!(wait_until(Duration::from_secs(5), || panics_so_far(
            &panics
        ) >= 2));
        failing.store(false, Ordering::SeqCst);
        assert!(
            wait_until(Duration::from_secs(5), || pictures.load(Ordering::SeqCst)
                >= 1),
            "the monitor never recovered"
        );
        failing.store(true, Ordering::SeqCst);
        emit_event(&dir, "Event 'change' on sink #1");
        assert!(wait_until(Duration::from_secs(5), || panics_so_far(
            &panics
        ) >= 4));
        stop_and_check_no_child_is_left(&monitor, &dir);
        let after_the_good_session = gaps(&panics)[2];
        assert!(
            after_the_good_session < Duration::from_millis(700),
            "waited {after_the_good_session:?}: the wait did not start over"
        );
    }

    #[test]
    fn a_panic_in_a_watcher_does_not_end_the_watching_nor_leak_the_event_child() {
        let dir = Dir::with_devices("panic");
        let pactl = Pactl::new(fake(), Some(dir.0.clone().into_os_string()));
        let timing = Timing::default()
            .retry_wait(Duration::from_millis(20))
            .retry_max(Duration::from_millis(40));
        let calls = Arc::new(AtomicUsize::new(0));
        let pictures = Arc::new(AtomicUsize::new(0));
        let publish: Publish = {
            let (calls, pictures) = (Arc::clone(&calls), Arc::clone(&pictures));
            Box::new(move |_, picture| {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("injected panic in a watcher");
                }
                if picture.is_ok() {
                    pictures.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        let monitor = Monitor::new(pactl, timing, publish);
        monitor.start().unwrap();
        assert!(
            wait_until(Duration::from_secs(3), || pictures.load(Ordering::SeqCst)
                >= 1),
            "no picture after the panic: the monitor gave up"
        );
        let panicked_session = pids(&dir, "subscribe-start ")[0];
        assert!(
            gone(panicked_session),
            "the first event child was left behind"
        );
        assert!(
            wait_until(Duration::from_secs(2), || alive(&dir, "subscribe-start ")
                == 1),
            "{} event children running",
            alive(&dir, "subscribe-start ")
        );
        monitor.stop();
        assert!(wait_until(Duration::from_secs(3), || alive(
            &dir,
            "subscribe-start "
        ) == 0));
    }

    #[test]
    fn stopping_ends_the_event_child_and_waits_for_it() {
        let dir = Dir::with_devices("stop-reap");
        let pactl = Pactl::new(fake(), Some(dir.0.clone().into_os_string()));
        let pictures = Arc::new(AtomicUsize::new(0));
        let publish: Publish = {
            let pictures = Arc::clone(&pictures);
            Box::new(move |_, picture| {
                if picture.is_ok() {
                    pictures.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        let monitor = Monitor::new(pactl, Timing::default(), publish);
        monitor.start().unwrap();
        assert!(
            wait_until(Duration::from_secs(3), || pictures.load(Ordering::SeqCst)
                >= 1
                && alive(&dir, "subscribe-start ") == 1),
            "the monitor never got going"
        );
        monitor.stop();
        assert!(
            wait_until(Duration::from_secs(3), || all_gone(
                &dir,
                "subscribe-start "
            )),
            "the event child outlived the stop (running or zombie): {:?}",
            pids(&dir, "subscribe-start ")
        );
    }
}
