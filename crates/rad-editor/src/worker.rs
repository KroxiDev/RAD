//! One in-flight job, including an unconsumed response, and at most one waiting command.
//! Never runs on the GUI thread.
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, SyncSender},
};

type Wake = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;
pub struct Worker<J, R> {
    sender: SyncSender<J>,
    receiver: Receiver<R>,
    busy: bool,
    waiting: Option<J>,
    /// A waiting command that could not even be handed to a dead thread.
    unsent: bool,
    /// The thread's death has been reported to the caller.
    reported: bool,
    wake: Wake,
}
/// What a worker has to say this turn.
#[derive(Debug, PartialEq, Eq)]
pub enum Polled<R> {
    /// A result is ready.
    Ready(R),
    /// Nothing yet.
    Pending,
    /// The thread died with a job in flight: that job's outcome is unknown.
    Failed { unsent: bool },
    /// The thread ended with no job in flight.
    Closed { unsent: bool },
}
impl<R> Polled<R> {
    pub fn ready(self) -> Option<R> {
        match self {
            Self::Ready(result) => Some(result),
            _ => None,
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum SubmitError {
    Busy,
    Disconnected,
}
impl<J: Send + 'static, R: Send + 'static> Worker<J, R> {
    pub fn spawn(mut run: impl FnMut(J) -> R + Send + 'static) -> Self {
        let (sender, jobs) = mpsc::sync_channel(1);
        let (results, receiver) = mpsc::sync_channel(1);
        let wake: Wake = Default::default();
        let notify = wake.clone();
        std::thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                if results.send(run(job)).is_err() {
                    break;
                }
                // Released before the call, so a panicking callback poisons nothing.
                let wake = notify.lock().unwrap().clone();
                if let Some(wake) = wake {
                    wake();
                }
            }
        });
        Self {
            sender,
            receiver,
            busy: false,
            waiting: None,
            unsent: false,
            reported: false,
            wake,
        }
    }
    /// Register the GUI wake callback. Also wakes for results produced before registration.
    pub fn set_waker(&self, wake: impl Fn() + Send + Sync + 'static) {
        let wake = Arc::new(wake);
        *self.wake.lock().unwrap() = Some(wake.clone());
        wake();
    }
    pub fn busy(&self) -> bool {
        self.busy
    }
    /// Background work: refused while a job is in flight, so callers simply retry later.
    pub fn submit(&mut self, job: J) -> Result<(), SubmitError> {
        if self.busy {
            return Err(SubmitError::Busy);
        }
        self.sender.try_send(job).map_err(|e| match e {
            mpsc::TrySendError::Full(_) => SubmitError::Busy,
            mpsc::TrySendError::Disconnected(_) => SubmitError::Disconnected,
        })?;
        self.busy = true;
        Ok(())
    }
    /// A user command: runs now, or right after the in-flight job and ahead of any later
    /// `submit`, so background polling neither drops it nor has to disable the controls.
    pub fn enqueue(&mut self, job: J) -> Result<(), SubmitError> {
        if !self.busy {
            return self.submit(job);
        }
        if self.waiting.is_some() {
            return Err(SubmitError::Busy);
        }
        self.waiting = Some(job);
        Ok(())
    }
    /// Takes the result that arrived, or says once that the thread is gone. After `Failed`
    /// or `Closed` the worker is never busy and every submission is refused.
    pub fn poll(&mut self) -> Polled<R> {
        match self.receiver.try_recv() {
            Ok(r) => Polled::Ready(self.consumed(r)),
            Err(mpsc::TryRecvError::Empty) => Polled::Pending,
            Err(mpsc::TryRecvError::Disconnected) => self.gone(),
        }
    }
    /// Blocking variant for non-GUI callers/tests; the app uses poll exclusively.
    pub fn receive(&mut self) -> Polled<R> {
        match self.receiver.recv() {
            Ok(r) => Polled::Ready(self.consumed(r)),
            Err(_) => self.gone(),
        }
    }
    fn consumed(&mut self, result: R) -> R {
        self.busy = false;
        if let Some(job) = self.waiting.take() {
            // A dead thread cannot take it; the next poll reports it as unsent.
            self.unsent = self.submit(job).is_err();
        }
        result
    }
    fn gone(&mut self) -> Polled<R> {
        if std::mem::replace(&mut self.reported, true) {
            return Polled::Pending;
        }
        let unsent = self.waiting.take().is_some() || self.unsent;
        if std::mem::take(&mut self.busy) {
            Polled::Failed { unsent }
        } else {
            Polled::Closed { unsent }
        }
    }
}

/// What a pump of one worker's results learned this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pulse {
    Quiet,
    /// The worker is alive and answered.
    Answered,
    /// The worker's thread is gone and its owner should replace it.
    Lost,
}

/// Recreates a dead worker, at most [`Respawner::LIMIT`] times in a row without an answer
/// in between. Nothing here polls or retries on a timer: the owner calls it on a death
/// it has just seen, or on an explicit user action.
pub struct Respawner<J, R> {
    make: Box<dyn Fn() -> Worker<J, R>>,
    deaths: u8,
    gave_up: bool,
}
impl<J: Send + 'static, R: Send + 'static> Respawner<J, R> {
    pub const LIMIT: u8 = 3;
    pub fn new(make: impl Fn() -> Worker<J, R> + 'static) -> Self {
        Self {
            make: Box::new(make),
            deaths: 0,
            gave_up: false,
        }
    }
    /// A correct answer arrived: the next death starts a new count.
    pub fn answered(&mut self) {
        self.deaths = 0;
    }
    /// Whether the limit was reached and only [`Respawner::revive`] can bring a worker back.
    pub fn gave_up(&self) -> bool {
        self.gave_up
    }
    /// Replaces the dead `worker` unless the limit was reached; returns whether it did.
    pub fn replace(
        &mut self,
        worker: &mut Worker<J, R>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> bool {
        if self.deaths >= Self::LIMIT {
            self.gave_up = true;
            return false;
        }
        self.deaths += 1;
        self.install(worker, wake);
        true
    }
    /// The user asked to reconnect: starts a fresh count and a fresh worker.
    pub fn revive(&mut self, worker: &mut Worker<J, R>, wake: impl Fn() + Send + Sync + 'static) {
        self.deaths = 0;
        self.gave_up = false;
        self.install(worker, wake);
    }
    fn install(&self, worker: &mut Worker<J, R>, wake: impl Fn() + Send + Sync + 'static) {
        *worker = (self.make)();
        worker.set_waker(wake);
    }
}

/// A single replaceable one-shot deadline (search debounce), not a periodic GUI timer.
type TimerState = Arc<(
    Mutex<(bool, Option<std::time::Instant>)>,
    std::sync::Condvar,
)>;
pub struct WakeTimer {
    state: TimerState,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl WakeTimer {
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        let state = Arc::new((
            Mutex::new((false, None::<std::time::Instant>)),
            std::sync::Condvar::new(),
        ));
        let pending = state.clone();
        let thread = std::thread::spawn(move || {
            let (lock, changed) = &*pending;
            let mut state = lock.lock().unwrap();
            loop {
                if state.0 {
                    break;
                }
                match state.1 {
                    None => state = changed.wait(state).unwrap(),
                    Some(deadline) => {
                        if let Some(left) =
                            deadline.checked_duration_since(std::time::Instant::now())
                        {
                            state = changed.wait_timeout(state, left).unwrap().0;
                        } else {
                            state.1 = None;
                            drop(state);
                            wake();
                            state = lock.lock().unwrap();
                        }
                    }
                }
            }
        });
        Self {
            state,
            thread: Some(thread),
        }
    }
    pub fn schedule(&self, after: std::time::Duration) {
        self.state.0.lock().unwrap().1 = Some(std::time::Instant::now() + after);
        self.state.1.notify_one();
    }
}
impl Drop for WakeTimer {
    fn drop(&mut self) {
        self.state.0.lock().unwrap().0 = true;
        self.state.1.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
