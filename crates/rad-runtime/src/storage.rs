//! Single Store/transfer owner. Only metadata and owned DTOs cross back to the actor.
//! In recovery mode there is no Store yet: the worker then only serves transfers.
use crate::transfers::{self, Metadata, Transfers};
use rad_core::configuration::{Candidate, ConfigurationData};
use rad_store::{CommitOutcome, ReplaceError, Store, StoreError};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerError {
    Busy,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobId(u64);
pub enum Job {
    Commit(Candidate),
    ReadRevision(i64),
    Reconcile,
    Collect(Vec<i64>),
    Transfer {
        context: transfers::Context,
        operation: transfers::Operation,
    },
    CommitMedia {
        candidate: Candidate,
        connection: u64,
        transfer_id: String,
    },
    ReleaseMedia {
        transfer_id: String,
    },
    /// Recovery mode: the base at `path` is replaced by a new one holding the verified
    /// media of the transfer, and the damaged file stays as evidence.
    ReplaceFromMedia {
        path: PathBuf,
        stamp: u64,
        candidate: Candidate,
        connection: u64,
        transfer_id: String,
    },
}
pub enum ResultValue {
    Commit(CommitOutcome),
    Collected {
        outcome: CommitOutcome,
        registry: Option<rad_core::identity::IdentityRegistry>,
    },
    Revision(Result<ConfigurationData, StoreError>),
    Transfer(Result<transfers::Output, transfers::Error>),
    Released(Result<(), transfers::Error>),
    Replaced(Result<(), ReplaceError>),
}
struct Completion {
    id: JobId,
    result: ResultValue,
    metadata: Option<Metadata>,
    released: Vec<String>,
}
pub struct StorageWorker {
    sender: SyncSender<(JobId, Job)>,
    receiver: Receiver<Completion>,
    pending: Option<JobId>,
    next: u64,
    metadata: Option<Metadata>,
    released: Vec<String>,
    thread: JoinHandle<()>,
}
struct WakeOnExit(Arc<mio::Waker>);

/// Explicit per-test ownership; never selected from an inherited environment.
#[cfg(all(test, feature = "fixture-tests"))]
pub(crate) mod fixture {
    use super::*;
    use std::{
        cell::RefCell,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    #[derive(Clone, Copy)]
    pub(crate) enum Fault {
        UncertainReturn,
        LostAfterCommit,
    }
    pub(crate) struct Hook {
        pub fault: Fault,
        pub commits: Arc<AtomicUsize>,
        pub entered: SyncSender<CommitOutcome>,
        pub release: Receiver<()>,
        pub ended: SyncSender<()>,
        pub used: bool,
    }
    thread_local! { static NEXT: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
    pub(crate) struct Control {
        pub entered: Receiver<CommitOutcome>,
        pub ended: Receiver<()>,
        pub commits: Arc<AtomicUsize>,
        release: Option<SyncSender<()>>,
    }
    impl Control {
        pub fn install(fault: Fault) -> Self {
            let (entered, received) = mpsc::sync_channel(1);
            let (release, permit) = mpsc::sync_channel(1);
            let (ended, stopped) = mpsc::sync_channel(1);
            let commits = Arc::new(AtomicUsize::new(0));
            NEXT.with(|next| {
                assert!(
                    next.borrow().is_none(),
                    "one explicit worker fixture per spawn"
                );
                *next.borrow_mut() = Some(Hook {
                    fault,
                    commits: commits.clone(),
                    entered,
                    release: permit,
                    ended,
                    used: false,
                });
            });
            Self {
                entered: received,
                ended: stopped,
                commits,
                release: Some(release),
            }
        }
        pub fn release(&mut self) {
            if let Some(release) = self.release.take() {
                let _ = release.try_send(());
            }
        }
        pub fn count(&self) -> usize {
            self.commits.load(Ordering::Acquire)
        }
        pub fn resources_dropped(&self) {
            self.ended.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }
    impl Drop for Control {
        fn drop(&mut self) {
            self.release();
            NEXT.with(|next| {
                next.borrow_mut().take();
            });
        }
    }
    pub(crate) fn take() -> Option<Hook> {
        NEXT.with(|next| next.borrow_mut().take())
    }
}
impl Drop for WakeOnExit {
    fn drop(&mut self) {
        let _ = self.0.wake();
    }
}
impl StorageWorker {
    pub fn spawn(store: Store, wake: Arc<mio::Waker>) -> Result<Self, WorkerError> {
        Self::start(Some(store), None, wake)
    }
    pub fn with_transfers(
        store: Store,
        transfers: Transfers,
        wake: Arc<mio::Waker>,
    ) -> Result<Self, WorkerError> {
        Self::start(Some(store), Some(transfers), wake)
    }
    /// Recovery mode: transfers without a Store.
    pub fn transfers_only(
        transfers: Transfers,
        wake: Arc<mio::Waker>,
    ) -> Result<Self, WorkerError> {
        Self::start(None, Some(transfers), wake)
    }
    fn start(
        mut store: Option<Store>,
        mut transfers: Option<Transfers>,
        wake: Arc<mio::Waker>,
    ) -> Result<Self, WorkerError> {
        #[cfg(all(test, feature = "fixture-tests"))]
        let mut fixture = fixture::take();
        let metadata = transfers.as_ref().map(Transfers::metadata);
        let (sender, commands) = mpsc::sync_channel::<(JobId, Job)>(1);
        let (completed, receiver) = mpsc::sync_channel(1);
        let handle = thread::Builder::new()
            .name("rad-store".into())
            .spawn(move || {
                let _on_exit = WakeOnExit(wake.clone());
                let mut uncertain_media: Option<String> = None;
                let mut uncertain_collection = false;
                while let Ok((id, job)) = commands.recv() {
                    #[cfg(all(test, feature = "fixture-tests"))]
                    let is_commit = matches!(&job, Job::Commit(_));
                    #[cfg(all(test, feature = "fixture-tests"))]
                    if is_commit
                        && fixture.as_ref().is_some_and(|hook| {
                            !hook.used && matches!(hook.fault, fixture::Fault::UncertainReturn)
                        })
                    {
                        store
                            .as_mut()
                            .expect("Store fixture requires its real Store owner")
                            .fixture_fail_next_commit_return();
                    }
                    let result = match job {
                        Job::Commit(candidate) => ResultValue::Commit(match store.as_mut() {
                            Some(store) => store.commit(&candidate),
                            None => CommitOutcome::NotApplied(StoreError::Invalid),
                        }),
                        Job::ReadRevision(revision) => {
                            ResultValue::Revision(match store.as_ref() {
                                Some(store) => store
                                    .revision(revision)
                                    .map(|l| l.configuration.data().clone()),
                                None => Err(StoreError::Invalid),
                            })
                        }
                        Job::Transfer { context, operation } => {
                            ResultValue::Transfer(match transfers.as_mut() {
                                Some(t) => t.execute_with(store.as_mut(), context, operation),
                                None => Err(transfers::Error::NotAllowed),
                            })
                        }
                        Job::ReleaseMedia { transfer_id } => {
                            ResultValue::Released(match transfers.as_mut() {
                                Some(t) => t.resolved(
                                    &transfer_id,
                                    &CommitOutcome::NotApplied(StoreError::Invalid),
                                ),
                                None => Err(transfers::Error::NotFound),
                            })
                        }
                        Job::CommitMedia {
                            candidate,
                            connection,
                            transfer_id,
                        } => {
                            let outcome = match (transfers.as_mut(), store.as_mut()) {
                                (Some(t), Some(store)) => {
                                    t.commit(store, &candidate, connection, &transfer_id)
                                }
                                _ => CommitOutcome::NotApplied(StoreError::Invalid),
                            };
                            if matches!(outcome, CommitOutcome::Uncertain { .. }) {
                                uncertain_media = Some(transfer_id.clone());
                            }
                            if let Some(t) = transfers.as_mut() {
                                let _ = t.resolved(&transfer_id, &outcome);
                            }
                            ResultValue::Commit(outcome)
                        }
                        Job::ReplaceFromMedia {
                            path,
                            stamp,
                            candidate,
                            connection,
                            transfer_id,
                        } => {
                            let replaced = match transfers.as_mut() {
                                Some(t) if store.is_none() => {
                                    rad_store::replace_damaged(&path, stamp, |temporary| {
                                        t.create_store(
                                            temporary,
                                            &candidate,
                                            connection,
                                            &transfer_id,
                                        )
                                    })
                                    .map(drop)
                                }
                                _ => Err(ReplaceError::NotApplied(StoreError::Invalid)),
                            };
                            if let Some(t) = transfers.as_mut() {
                                let outcome = match replaced {
                                    Ok(()) => CommitOutcome::Applied {
                                        revision: 0,
                                        changed: true,
                                    },
                                    Err(ReplaceError::NotApplied(error)) => {
                                        CommitOutcome::NotApplied(error)
                                    }
                                    Err(ReplaceError::AfterExchange) => CommitOutcome::Uncertain {
                                        revision: 0,
                                        error: StoreError::Uncertain,
                                    },
                                };
                                let _ = t.resolved(&transfer_id, &outcome);
                            }
                            ResultValue::Replaced(replaced)
                        }
                        Job::Reconcile => {
                            let outcome = store.as_mut().map_or(
                                CommitOutcome::NotApplied(StoreError::Invalid),
                                Store::reconcile,
                            );
                            if let Some(token) = &uncertain_media
                                && let Some(t) = transfers.as_mut()
                            {
                                let _ = t.resolved(token, &outcome);
                            }
                            if !matches!(outcome, CommitOutcome::Uncertain { .. }) {
                                uncertain_media = None;
                            }
                            if uncertain_collection {
                                let result = collection_result(store.as_ref(), outcome);
                                uncertain_collection = matches!(
                                    &result,
                                    ResultValue::Collected {
                                        outcome: CommitOutcome::Uncertain { .. },
                                        ..
                                    }
                                );
                                result
                            } else {
                                ResultValue::Commit(outcome)
                            }
                        }
                        Job::Collect(roots) => {
                            let collected = store
                                .as_mut()
                                .map_or(Err(StoreError::Invalid), |store| store.collect(&roots));
                            let outcome = match collected {
                                Ok(()) => CommitOutcome::Applied {
                                    revision: 0,
                                    changed: false,
                                },
                                Err(StoreError::Uncertain) => CommitOutcome::Uncertain {
                                    revision: 0,
                                    error: StoreError::Uncertain,
                                },
                                Err(error) => CommitOutcome::NotApplied(error),
                            };
                            let result = collection_result(store.as_ref(), outcome);
                            uncertain_collection = matches!(
                                &result,
                                ResultValue::Collected {
                                    outcome: CommitOutcome::Uncertain { .. },
                                    ..
                                }
                            );
                            result
                        }
                    };
                    #[cfg(all(test, feature = "fixture-tests"))]
                    if let Some(fixture) = &mut fixture
                        && is_commit
                    {
                        fixture
                            .commits
                            .fetch_add(1, std::sync::atomic::Ordering::Release);
                        if !fixture.used {
                            fixture.used = true;
                            if let ResultValue::Commit(outcome) = &result {
                                let _ = fixture.entered.try_send(*outcome);
                            }
                            // A panic/drop of the controller also releases this wait.
                            let _ = fixture
                                .release
                                .recv_timeout(std::time::Duration::from_secs(5));
                            if matches!(fixture.fault, fixture::Fault::LostAfterCommit) {
                                break;
                            }
                        }
                    }
                    let metadata = transfers.as_ref().map(Transfers::metadata);
                    let released = transfers
                        .as_mut()
                        .map(Transfers::take_released)
                        .unwrap_or_default();
                    if completed
                        .send(Completion {
                            id,
                            result,
                            metadata,
                            released,
                        })
                        .is_err()
                    {
                        break;
                    }
                    let _ = wake.wake();
                }
                #[cfg(all(test, feature = "fixture-tests"))]
                if let Some(fixture) = fixture {
                    drop(store);
                    drop(transfers);
                    let _ = fixture.ended.try_send(());
                }
            })
            .map_err(|_| WorkerError::Unavailable)?;
        Ok(Self {
            sender,
            receiver,
            pending: None,
            next: 1,
            metadata,
            released: vec![],
            thread: handle,
        })
    }
    pub fn submit(&mut self, job: Job) -> Result<JobId, WorkerError> {
        self.try_submit(job).map_err(|(e, _)| e)
    }
    /// Return the original job on failure, so the one already-started Authority
    /// persist can wait behind transfer IO without cloning or losing its candidate.
    pub fn try_submit(&mut self, job: Job) -> Result<JobId, (WorkerError, Box<Job>)> {
        if self.pending.is_some() || self.released.len() >= 2 {
            return Err((WorkerError::Busy, Box::new(job)));
        }
        let id = JobId(self.next);
        let Some(next) = self.next.checked_add(1) else {
            return Err((WorkerError::Unavailable, Box::new(job)));
        };
        match self.sender.try_send((id, job)) {
            Ok(()) => {}
            Err(TrySendError::Full((_, job))) => return Err((WorkerError::Busy, Box::new(job))),
            Err(TrySendError::Disconnected((_, job))) => {
                return Err((WorkerError::Unavailable, Box::new(job)));
            }
        }
        self.next = next;
        self.pending = Some(id);
        Ok(id)
    }
    pub fn take(&mut self) -> Result<Option<(JobId, ResultValue)>, WorkerError> {
        match self.receiver.try_recv() {
            Ok(c) => {
                if self.pending != Some(c.id) {
                    return Err(WorkerError::Unavailable);
                }
                self.pending = None;
                self.metadata = c.metadata;
                self.released.extend(c.released);
                Ok(Some((c.id, c.result)))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(WorkerError::Unavailable),
        }
    }
    /// Ends the thread and waits for it, so what it owned (the transfer folder) is free.
    pub fn stop(self) {
        let Self {
            sender,
            receiver,
            thread,
            ..
        } = self;
        drop(sender);
        drop(receiver);
        let _ = thread.join();
    }
    pub fn transfer_metadata(&self) -> Option<&Metadata> {
        self.metadata.as_ref()
    }
    pub fn take_released(&mut self) -> Vec<String> {
        std::mem::take(&mut self.released)
    }
}

fn collection_result(store: Option<&Store>, outcome: CommitOutcome) -> ResultValue {
    if matches!(outcome, CommitOutcome::Applied { .. }) {
        match store.ok_or(StoreError::Invalid).and_then(Store::load) {
            Ok(loaded) => ResultValue::Collected {
                outcome: CommitOutcome::Applied {
                    revision: loaded.revision,
                    changed: false,
                },
                registry: Some(loaded.registry),
            },
            Err(error) => ResultValue::Collected {
                outcome: CommitOutcome::Uncertain { revision: 0, error },
                registry: None,
            },
        }
    } else {
        ResultValue::Collected {
            outcome,
            registry: None,
        }
    }
}
