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
    wake: Wake,
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
                if let Some(wake) = notify.lock().unwrap().as_ref() {
                    wake();
                }
            }
        });
        Self {
            sender,
            receiver,
            busy: false,
            waiting: None,
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
    pub fn poll(&mut self) -> Option<R> {
        let r = self.receiver.try_recv().ok()?;
        self.consumed();
        Some(r)
    }
    /// Blocking consumption for non-GUI callers/tests; the app uses poll exclusively.
    pub fn receive(&mut self) -> Option<R> {
        let r = self.receiver.recv().ok()?;
        self.consumed();
        Some(r)
    }
    fn consumed(&mut self) {
        self.busy = false;
        if let Some(job) = self.waiting.take() {
            // A dead worker thread loses the command; every later call reports Disconnected.
            let _ = self.submit(job);
        }
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
