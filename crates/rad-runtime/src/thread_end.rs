//! A worker thread's end, published before the runtime loop is woken.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Held for a worker thread's whole body. Dropping it, on return or unwind, first records
/// the end and then wakes the loop. `JoinHandle::is_finished` cannot serve here: it turns
/// true only after the thread has dropped everything, this guard included, so a loop
/// woken by the guard could still read "running" and go back to sleep for good.
pub(crate) struct EndGuard {
    ended: Arc<AtomicBool>,
    wake: Box<dyn Fn() + Send>,
}
impl Drop for EndGuard {
    fn drop(&mut self) {
        self.ended.store(true, Ordering::Release);
        (self.wake)();
    }
}
/// Whether the thread holding the matching guard has ended.
#[derive(Clone)]
pub(crate) struct Ended(Arc<AtomicBool>);
impl Ended {
    pub fn get(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    #[cfg(test)]
    pub fn already() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }
}
pub(crate) fn end_signal(wake: impl Fn() + Send + 'static) -> (EndGuard, Ended) {
    let ended = Arc::new(AtomicBool::new(false));
    (
        EndGuard {
            ended: ended.clone(),
            wake: Box::new(wake),
        },
        Ended(ended),
    )
}
