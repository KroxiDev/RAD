//! One private vault owner. Filesystem calls and fsync never run on the serving loop.
use rad_credentials::{Error, State, Status, Vault};
use rad_plugin_contract::{CredentialValue, CredentialValues};
use rad_protocol::CredentialOperation;
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
};
#[derive(Clone)]
pub(crate) struct Binding {
    pub plugin: String,
    pub generation: u64,
    pub slots: Vec<String>,
}
pub(crate) enum Task {
    Request(CredentialOperation, Vec<Binding>),
    Refresh(Vec<Binding>),
    Reconcile(Vec<Binding>),
}
pub(crate) struct Map {
    pub binding: Binding,
    pub values: Result<CredentialValues, Error>,
}
pub(crate) struct Finished {
    pub state: State,
    pub result: Option<Result<Status, Error>>,
    pub maps: Vec<Map>,
}
pub(crate) enum Event {
    Opened(State),
    Finished(Finished),
}
pub(crate) struct Worker {
    tx: Option<mpsc::SyncSender<Task>>,
    rx: mpsc::Receiver<Event>,
    thread: thread::JoinHandle<()>,
    ended: crate::thread_end::Ended,
    busy: bool,
    pub state: Option<State>,
}
impl Worker {
    #[cfg(test)]
    pub(crate) fn controlled() -> (Self, mpsc::Receiver<Task>, mpsc::SyncSender<Event>) {
        let (tx, tasks) = mpsc::sync_channel(1);
        let (results, rx) = mpsc::sync_channel(1);
        let thread = thread::spawn(|| {});
        while !thread.is_finished() {
            thread::yield_now();
        }
        (
            Self {
                tx: Some(tx),
                rx,
                thread,
                ended: crate::thread_end::Ended::already(),
                busy: false,
                state: Some(State::Ready),
            },
            tasks,
            results,
        )
    }

    pub fn spawn(root: PathBuf, wake: Arc<mio::Waker>) -> Result<Self, Error> {
        let (tx, tasks) = mpsc::sync_channel::<Task>(1);
        let (results, rx) = mpsc::sync_channel(1);
        let exit_wake = wake.clone();
        let (end, ended) = crate::thread_end::end_signal(move || {
            let _ = exit_wake.wake();
        });
        let thread = thread::Builder::new()
            .name("rad-credentials".into())
            .spawn(move || {
                let _end = end;
                let mut vault = Vault::open(&root);
                let state = vault.as_ref().map_or(State::Unknown, Vault::state);
                if results.send(Event::Opened(state)).is_err() {
                    return;
                }
                let _ = wake.wake();
                while let Ok(task) = tasks.recv() {
                    let mut result = None;
                    let bindings = match task {
                        Task::Request(operation, bindings) => {
                            // A new explicit request gets one recovery attempt, never a replay
                            // of the request whose durability was uncertain.
                            if let Ok(vault) = &mut vault {
                                if vault.state() == State::Unknown {
                                    let _ = vault.reconcile();
                                }
                            } else {
                                vault = Vault::open(&root);
                            }
                            result = Some(match &mut vault {
                                Ok(vault) => match operation {
                                    CredentialOperation::Status { plugin_id, slot } => {
                                        vault.status(&plugin_id, &slot)
                                    }
                                    CredentialOperation::Replace { plugin_id, secret } => {
                                        rad_credentials::Secret::new(secret.value.clone())
                                            .and_then(|value| {
                                                vault.replace(&plugin_id, &secret.slot, value)
                                            })
                                            .map(|()| {
                                                vault
                                                    .status(&plugin_id, &secret.slot)
                                                    .unwrap_or(Status::Unavailable)
                                            })
                                    }
                                    CredentialOperation::Remove { plugin_id, slot } => {
                                        vault.remove(&plugin_id, &slot).map(|()| {
                                            vault
                                                .status(&plugin_id, &slot)
                                                .unwrap_or(Status::Unavailable)
                                        })
                                    }
                                },
                                Err(error) => Err(*error),
                            });
                            bindings
                        }
                        Task::Refresh(bindings) => bindings,
                        Task::Reconcile(bindings) => {
                            if let Ok(vault) = &mut vault {
                                let _ = vault.reconcile();
                            } else {
                                vault = Vault::open(&root);
                            }
                            bindings
                        }
                    };
                    let state = vault.as_ref().map_or(State::Unknown, Vault::state);
                    let maps = bindings
                        .into_iter()
                        .map(|binding| {
                            let values = match &vault {
                                Ok(vault) => vault
                                    .resolve_declared(
                                        &binding.plugin,
                                        &binding
                                            .slots
                                            .iter()
                                            .map(String::as_str)
                                            .collect::<Vec<_>>(),
                                    )
                                    .and_then(|secrets| {
                                        CredentialValues::new(
                                            secrets
                                                .iter()
                                                .map(|(slot, value)| CredentialValue {
                                                    slot: slot.into(),
                                                    value: value.expose().into(),
                                                })
                                                .collect(),
                                        )
                                        .map_err(|_| Error::Unavailable)
                                    }),
                                Err(error) => Err(*error),
                            };
                            Map { binding, values }
                        })
                        .collect();
                    if results
                        .send(Event::Finished(Finished {
                            state,
                            result,
                            maps,
                        }))
                        .is_err()
                    {
                        break;
                    }
                    let _ = wake.wake();
                }
            })
            .map_err(|_| Error::Unavailable)?;
        Ok(Self {
            tx: Some(tx),
            rx,
            thread,
            ended,
            busy: false,
            state: None,
        })
    }
    pub fn submit(&mut self, task: Task) -> Result<(), Error> {
        if self.busy {
            return Err(Error::Busy);
        }
        self.tx
            .as_ref()
            .ok_or(Error::Unavailable)?
            .try_send(task)
            .map_err(|_| Error::Unavailable)?;
        self.busy = true;
        Ok(())
    }
    pub fn available(&self) -> bool {
        !self.busy && self.tx.is_some() && !self.thread.is_finished()
    }
    pub fn poll(&mut self) -> Option<Event> {
        let event = match self.rx.try_recv() {
            Ok(event) => event,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.tx.take()?;
                self.busy = false;
                self.state = Some(State::Unknown);
                return Some(Event::Finished(Finished {
                    state: State::Unknown,
                    result: Some(Err(Error::Uncertain)),
                    maps: vec![],
                }));
            }
        };
        match &event {
            Event::Opened(state) => self.state = Some(*state),
            Event::Finished(done) => {
                self.state = Some(done.state);
                self.busy = false;
            }
        }
        Some(event)
    }
    pub fn shutdown(&mut self) {
        self.tx.take();
    }
    pub fn stopped(&self) -> bool {
        self.ended.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lost_worker_resolves_pending_work_as_unknown_once() {
        let (tx, _tasks) = mpsc::sync_channel(1);
        let (results, rx) = mpsc::sync_channel(1);
        drop(results);
        let thread = thread::spawn(|| {});
        while !thread.is_finished() {
            thread::yield_now();
        }
        let mut worker = Worker {
            tx: Some(tx),
            rx,
            thread,
            ended: crate::thread_end::Ended::already(),
            busy: true,
            state: Some(State::Ready),
        };
        let Some(Event::Finished(done)) = worker.poll() else {
            panic!("lost worker left a request pending forever");
        };
        assert_eq!(done.state, State::Unknown);
        assert_eq!(done.result, Some(Err(Error::Uncertain)));
        assert!(!worker.available());
        assert!(worker.poll().is_none());
    }
}
