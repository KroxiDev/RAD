//! One coalesced slot per notice kind, plus connection state. No unbounded queue.
use rad_api::NoticeKind;
use rad_client::connection::{Client, Endpoint, SubscriptionCancel};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
};
#[derive(Default)]
struct State {
    stopped: bool,
    kinds: BTreeSet<NoticeKind>,
    connection: Option<bool>,
    last_connection: Option<bool>,
    cancel: Option<SubscriptionCancel>,
}
pub struct Notices {
    shared: Arc<(Mutex<State>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}
impl Notices {
    pub fn start(runtime: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let source = shared.clone();
        let thread = std::thread::spawn(move || {
            loop {
                if source.0.lock().unwrap().stopped {
                    break;
                }
                let connection =
                    Client::connect(&runtime, Endpoint::Editor).and_then(Client::subscribe);
                if let Ok(mut subscription) = connection
                    && let Ok(cancel) = subscription.cancellation()
                {
                    {
                        let mut state = source.0.lock().unwrap();
                        if state.stopped {
                            break;
                        }
                        state.cancel = Some(cancel);
                        state.connection = Some(true);
                        state.last_connection = Some(true);
                        state.kinds.extend(NoticeKind::ALL);
                    }
                    wake();
                    while let Ok(notice) = subscription.recv() {
                        let mut state = source.0.lock().unwrap();
                        if state.stopped {
                            break;
                        }
                        let changed = state.kinds.insert(notice.kind);
                        drop(state);
                        if changed {
                            wake();
                        }
                    }
                }
                let mut state = source.0.lock().unwrap();
                state.cancel = None;
                if state.stopped {
                    break;
                }
                let changed = state.last_connection != Some(false);
                state.last_connection = Some(false);
                if changed {
                    state.connection = Some(false);
                }
                drop(state);
                if changed {
                    wake();
                }
                // This is transport reconnect backoff, never a state query or GUI timer.
                let state = source.0.lock().unwrap();
                let _ = source
                    .1
                    .wait_timeout_while(state, std::time::Duration::from_secs(1), |s| !s.stopped)
                    .unwrap();
            }
        });
        Self {
            shared,
            thread: Some(thread),
        }
    }
    pub fn take(&self) -> (Option<bool>, BTreeSet<NoticeKind>) {
        let mut state = self.shared.0.lock().unwrap();
        (state.connection.take(), std::mem::take(&mut state.kinds))
    }
}
impl Drop for Notices {
    fn drop(&mut self) {
        {
            let mut state = self.shared.0.lock().unwrap();
            state.stopped = true;
            if let Some(cancel) = &state.cancel {
                cancel.cancel();
            }
        }
        self.shared.1.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
