//! One in-flight job, including an unconsumed response, and at most one waiting command.
//! Never runs on the GUI thread.
use std::sync::mpsc::{self, Receiver, SyncSender};
pub struct Worker<J, R> {
    sender: SyncSender<J>,
    receiver: Receiver<R>,
    busy: bool,
    waiting: Option<J>,
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
        std::thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                if results.send(run(job)).is_err() {
                    break;
                }
            }
        });
        Self {
            sender,
            receiver,
            busy: false,
            waiting: None,
        }
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
