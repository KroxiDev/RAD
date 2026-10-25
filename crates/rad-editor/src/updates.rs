//! Screen-scoped installation client. No network work starts without `check`.
use crate::worker::{Polled, Worker};
use rad_update::{Cancellation, Candidate, CheckResult, Prepared, Progress};
use std::sync::{Arc, Mutex};

/// Public installation seam. Tests exercise it without network, systemd, or root.
pub trait Backend: Send + Sync + 'static {
    fn check(&self, cancel: &Cancellation) -> Result<CheckResult, String>;
    fn prepare(
        &self,
        candidate: &Candidate,
        cancel: &Cancellation,
        progress: &Progress,
    ) -> Result<Prepared, String>;
    fn handoff(&self, prepared: &Prepared) -> Result<(), String>;
    fn discard(&self, prepared: &Prepared);
    fn outcome(&self) -> Result<Option<rad_update::coordinator::OperationResult>, String> {
        Ok(None)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Verification {
    #[default]
    Unchecked,
    Checking,
    Failed(String),
    Current,
    Available {
        candidate: Box<Candidate>,
        managed: bool,
    },
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Installation {
    #[default]
    Idle,
    Confirming,
    Preparing,
    Ready,
    HandingOff,
    Failed(String),
    Cancelled,
    Relayed,
}

type Wake = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;
fn notify(wake: &Wake) {
    let callback = wake.lock().unwrap().clone();
    if let Some(callback) = callback {
        callback();
    }
}

/// One controller survives visits. Its slot includes cancelled but still-live work.
pub struct Controller {
    worker: Worker<Work, Envelope>,
    verification: Verification,
    installation: Installation,
    staged: Option<Box<Staged>>,
    generation: u64,
    cancel: Cancellation,
    visible: bool,
    cancelled_pending: bool,
    progress: Progress,
    wake: Wake,
    outcome: Option<Result<Option<rad_update::coordinator::OperationResult>, String>>,
}
struct Work {
    generation: u64,
    job: Job,
}
enum Job {
    Check(Cancellation),
    Prepare {
        candidate: Box<Candidate>,
        cancel: Cancellation,
        progress: Progress,
    },
    Handoff(Box<Staged>),
    Outcome,
}
struct Envelope {
    generation: u64,
    installing: bool,
    reply: Reply,
}
enum Reply {
    Checked(Result<CheckResult, String>),
    Prepared(Result<Box<Staged>, String>),
    HandedOff(Result<(), String>),
    Panicked,
    Outcome(Result<Option<rad_update::coordinator::OperationResult>, String>),
}
/// The reply itself owns cleanup, including when its receiver or screen disappeared.
struct Staged {
    prepared: Prepared,
    backend: Arc<dyn Backend>,
    transferred: bool,
}
impl Staged {
    fn handoff(mut self: Box<Self>) -> Result<(), String> {
        let result = self.backend.handoff(&self.prepared);
        self.transferred = result.is_ok();
        result
    }
}
impl Drop for Staged {
    fn drop(&mut self) {
        if !self.transferred {
            self.backend.discard(&self.prepared);
        }
    }
}
impl Controller {
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        let worker = Worker::spawn(move |work: Work| {
            let installing = matches!(work.job, Job::Prepare { .. } | Job::Handoff(_));
            let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match work.job {
                Job::Check(cancel) => Reply::Checked(backend.check(&cancel)),
                Job::Prepare {
                    candidate,
                    cancel,
                    progress,
                } => Reply::Prepared(backend.prepare(&candidate, &cancel, &progress).map(
                    |prepared| {
                        Box::new(Staged {
                            prepared,
                            backend: backend.clone(),
                            transferred: false,
                        })
                    },
                )),
                Job::Handoff(staged) => Reply::HandedOff(staged.handoff()),
                Job::Outcome => Reply::Outcome(backend.outcome()),
            }))
            .unwrap_or(Reply::Panicked);
            Envelope {
                generation: work.generation,
                installing,
                reply,
            }
        });
        Self {
            worker,
            verification: Verification::Unchecked,
            installation: Installation::Idle,
            staged: None,
            generation: 0,
            cancel: Cancellation::default(),
            visible: false,
            cancelled_pending: false,
            progress: Progress::default(),
            wake: Default::default(),
            outcome: None,
        }
    }
    pub fn set_waker(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self.wake.lock().unwrap() = Some(Arc::new(wake));
        let wake = self.wake.clone();
        self.worker.set_waker(move || notify(&wake));
    }
    pub fn enter(&mut self) {
        self.visible = true;
    }
    /// False only after the coordinator owns the operation; leaving cannot cancel pacman.
    pub fn leave(&mut self) -> bool {
        if self.relaying() {
            return false;
        }
        self.visible = false;
        self.invalidate();
        self.installation = Installation::Idle;
        true
    }
    fn invalidate(&mut self) {
        self.cancelled_pending = self.worker.busy();
        self.cancel.cancel();
        self.generation = self.generation.wrapping_add(1);
        self.verification = Verification::Unchecked;
        self.staged = None;
        self.outcome = None;
    }
    pub fn busy(&self) -> bool {
        self.worker.busy()
    }
    pub fn cancelling(&self) -> bool {
        self.cancelled_pending && self.busy()
    }
    pub fn relaying(&self) -> bool {
        matches!(
            self.installation,
            Installation::HandingOff | Installation::Relayed
        )
    }
    pub fn outcome(
        &self,
    ) -> Option<&Result<Option<rad_update::coordinator::OperationResult>, String>> {
        self.outcome.as_ref()
    }
    pub fn read_outcome(&mut self) -> bool {
        if !self.visible || self.busy() || self.relaying() {
            return false;
        }
        self.submit(Job::Outcome)
    }
    pub fn verification(&self) -> &Verification {
        &self.verification
    }
    pub fn installation(&self) -> &Installation {
        &self.installation
    }
    pub fn percent(&self) -> Option<u8> {
        self.progress.percent()
    }
    pub fn check(&mut self) -> bool {
        if !self.visible || self.busy() || self.relaying() {
            return false;
        }
        self.invalidate();
        self.installation = Installation::Idle;
        self.verification = Verification::Checking;
        self.cancel = Cancellation::default();
        self.submit(Job::Check(self.cancel.clone()))
    }
    pub fn confirm(&mut self) -> bool {
        if !self.visible
            || self.busy()
            || !matches!(
                self.verification,
                Verification::Available { managed: true, .. }
            )
            || self.relaying()
        {
            return false;
        }
        self.installation = Installation::Confirming;
        true
    }
    pub fn cancel_confirmation(&mut self) {
        if self.installation == Installation::Confirming {
            self.installation = Installation::Idle;
        }
    }
    pub fn cancel(&mut self) {
        if self.relaying() {
            return;
        }
        self.invalidate();
        self.installation = Installation::Cancelled;
    }
    pub fn prepare(&mut self) -> bool {
        if !self.visible || self.busy() || self.installation != Installation::Confirming {
            return false;
        }
        let Verification::Available {
            candidate,
            managed: true,
        } = &self.verification
        else {
            return false;
        };
        let candidate = candidate.clone();
        let wake = self.wake.clone();
        self.progress = Progress::new(move || notify(&wake));
        let progress = self.progress.clone();
        self.installation = Installation::Preparing;
        self.cancel = Cancellation::default();
        self.submit(Job::Prepare {
            candidate,
            cancel: self.cancel.clone(),
            progress,
        })
    }
    /// Called only after the UI rechecks that no drafts or commands are pending.
    pub fn handoff(&mut self) -> bool {
        if !self.visible || self.busy() || self.installation != Installation::Ready {
            return false;
        }
        let Some(staged) = self.staged.take() else {
            return false;
        };
        self.installation = Installation::HandingOff;
        self.submit(Job::Handoff(staged))
    }
    fn submit(&mut self, job: Job) -> bool {
        let installing = matches!(job, Job::Prepare { .. } | Job::Handoff(_));
        if self
            .worker
            .submit(Work {
                generation: self.generation,
                job,
            })
            .is_err()
        {
            self.failed(installing, "update worker unavailable".into());
            return false;
        }
        true
    }
    fn failed(&mut self, installing: bool, error: String) {
        self.invalidate();
        if installing {
            self.installation = Installation::Failed(error);
        } else {
            self.verification = Verification::Failed(error);
        }
    }
    pub fn poll(&mut self) {
        let result = match self.worker.poll() {
            Polled::Pending => return,
            Polled::Failed { .. } | Polled::Closed { .. } => {
                self.failed(
                    self.installation != Installation::Idle,
                    "update worker stopped".into(),
                );
                return;
            }
            Polled::Ready(result) => result,
        };
        self.cancelled_pending = false;
        if !self.visible || result.generation != self.generation {
            return;
        }
        let installing = result.installing;
        let result = result.reply;
        match result {
            Reply::Checked(result) => {
                self.verification = match result {
                    Ok(CheckResult::NoUpdate) => Verification::Current,
                    Ok(CheckResult::Available { candidate, managed }) => {
                        Verification::Available { candidate, managed }
                    }
                    Err(error) => Verification::Failed(error),
                };
            }
            Reply::Prepared(Err(error)) | Reply::HandedOff(Err(error)) => self.failed(true, error),
            Reply::Prepared(Ok(staged)) => {
                self.staged = Some(staged);
                self.installation = Installation::Ready;
            }
            Reply::HandedOff(Ok(())) => {
                self.verification = Verification::Unchecked;
                self.installation = Installation::Relayed;
            }
            Reply::Outcome(result) => self.outcome = Some(result),
            Reply::Panicked => self.failed(installing, "update worker stopped".into()),
        }
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Production backend. Construction is inert; only its explicit operations do I/O.
pub struct SystemBackend;
impl Backend for SystemBackend {
    fn check(&self, cancel: &Cancellation) -> Result<CheckResult, String> {
        rad_update::check(cancel).map_err(|e| e.to_string())
    }
    fn prepare(
        &self,
        candidate: &Candidate,
        cancel: &Cancellation,
        progress: &Progress,
    ) -> Result<Prepared, String> {
        rad_update::prepare(candidate, cancel, progress).map_err(|e| e.to_string())
    }
    fn handoff(&self, prepared: &Prepared) -> Result<(), String> {
        rad_update::coordinator::handoff(prepared).map_err(|e| e.to_string())
    }
    fn discard(&self, prepared: &Prepared) {
        let _ = rad_update::discard_prepared(prepared);
    }
    fn outcome(&self) -> Result<Option<rad_update::coordinator::OperationResult>, String> {
        rad_update::coordinator::operation_result().map_err(|e| e.to_string())
    }
}
impl Default for Controller {
    fn default() -> Self {
        Self::new(Arc::new(SystemBackend))
    }
}
