use std::{
    sync::{Condvar, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

enum Pending {
    Nothing,
    Changes(Instant, Instant),
    Now,
}

struct State {
    stopped_since: Option<Instant>,
    pending: Pending,
}

pub(crate) struct Wake {
    state: Mutex<State>,
    wake: Condvar,
}

impl Wake {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(State {
                stopped_since: None,
                pending: Pending::Nothing,
            }),
            wake: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn changed(&self) {
        let now = Instant::now();
        let mut state = self.lock();
        state.pending = match state.pending {
            Pending::Nothing => Pending::Changes(now, now),
            Pending::Changes(first, _) => Pending::Changes(first, now),
            Pending::Now => Pending::Now,
        };
        self.wake.notify_all();
    }

    pub(crate) fn now(&self) {
        self.lock().pending = Pending::Now;
        self.wake.notify_all();
    }

    pub(crate) fn stop(&self) {
        self.lock().stopped_since.get_or_insert_with(Instant::now);
        self.wake.notify_all();
    }

    pub(crate) fn resume(&self) {
        self.lock().stopped_since = None;
    }

    pub(crate) fn wait(
        &self,
        quiet: Duration,
        most: Duration,
        idle: Duration,
        linger: Duration,
    ) -> bool {
        let idle_ends = Instant::now() + idle;
        let mut state = self.lock();
        loop {
            let now = Instant::now();
            let until_stop = match state.stopped_since {
                Some(since) if now >= since + linger => return false,
                Some(since) => Some((since + linger).saturating_duration_since(now)),
                None => None,
            };
            let until_look = match state.pending {
                Pending::Nothing => {
                    let left = idle_ends.saturating_duration_since(now);
                    if left.is_zero() {
                        return true;
                    }
                    left
                }
                Pending::Now => {
                    state.pending = Pending::Nothing;
                    return true;
                }
                Pending::Changes(first, last) => {
                    let end = (last + quiet).min(first + most);
                    let left = end.saturating_duration_since(now);
                    if left.is_zero() {
                        state.pending = Pending::Nothing;
                        return true;
                    }
                    left
                }
            };
            let wait = until_stop.map_or(until_look, |stop| stop.min(until_look));
            state = self
                .wake
                .wait_timeout(state, wait)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    pub(crate) fn sleep(&self, duration: Duration) -> bool {
        let end = Instant::now() + duration;
        let mut state = self.lock();
        while state.stopped_since.is_none() {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            state = self
                .wake
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        false
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.lock().stopped_since.is_some()
    }
}
