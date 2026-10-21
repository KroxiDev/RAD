//! Network half of a plugin operation, kept off the admin worker.
//!
//! The worker also spawns and reaps plugin processes; a download of up to minutes
//! must never delay another plugin's cold start. This thread only produces bytes:
//! package validation, staging, permits and publication stay with the worker and
//! the coordinator.
use super::*;
use rad_plugin_manager::{Cancellation, Error, GithubSource};
use std::sync::Mutex;

pub(super) type Fetched = Result<Option<Vec<u8>>, Error>;

enum Stage {
    Downloading {
        request: PluginAdmin,
        thread: thread::JoinHandle<()>,
        ended: crate::thread_end::Ended,
        done: mpsc::Receiver<Fetched>,
    },
    Done {
        request: PluginAdmin,
        fetched: Fetched,
    },
}

pub(super) struct Fetcher {
    download: Arc<Mutex<Download>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    stage: Option<Stage>,
}

impl Fetcher {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>, download: Download) -> Self {
        Self {
            download: Arc::new(Mutex::new(download)),
            wake,
            stage: None,
        }
    }
    /// An operation is downloading or waiting for the worker to take its result.
    pub fn active(&self) -> bool {
        self.stage.is_some()
    }
    /// False once no download thread is left running.
    pub fn stopped(&self) -> bool {
        match &self.stage {
            Some(Stage::Downloading { ended, .. }) => ended.get(),
            _ => true,
        }
    }
    /// Requests without a source (or with an invalid one) need no thread.
    pub fn start(
        &mut self,
        request: PluginAdmin,
        cancel: &Cancellation,
    ) -> Result<(), std::io::Error> {
        debug_assert!(self.stage.is_none(), "one fetch at a time");
        let source = match request
            .source
            .as_deref()
            .map(GithubSource::parse)
            .transpose()
        {
            Ok(source) => source,
            Err(e) => {
                self.finished(request, Err(e));
                return Ok(());
            }
        };
        if source.is_none() {
            self.finished(request, Ok(None));
            return Ok(());
        }
        let (tx, done) = mpsc::sync_channel(1);
        let (download, wake, cancel) = (self.download.clone(), self.wake.clone(), cancel.clone());
        let url = request.source.clone().unwrap_or_default();
        // Also when the downloader panics: `collect` then reports the lost result.
        let (end, ended) = crate::thread_end::end_signal(move || wake());
        let thread = thread::Builder::new()
            .name("rad-plugin-fetch".into())
            .spawn(move || {
                let _end = end;
                let fetched = {
                    // A panicking downloader poisons the lock; the next fetch still runs.
                    let download = download.lock().unwrap_or_else(|e| e.into_inner());
                    if cancel.is_cancelled() {
                        Err(Error::Cancelled)
                    } else {
                        (download)(&url, &cancel).map(Some).and_then(|p| {
                            if cancel.is_cancelled() {
                                Err(Error::Cancelled)
                            } else {
                                Ok(p)
                            }
                        })
                    }
                };
                let _ = tx.send(fetched);
            })?;
        self.stage = Some(Stage::Downloading {
            request,
            thread,
            ended,
            done,
        });
        Ok(())
    }
    fn finished(&mut self, request: PluginAdmin, fetched: Fetched) {
        self.stage = Some(Stage::Done { request, fetched });
    }
    /// Moves a returned download to `Done`; a thread that died without a result is an I/O error.
    pub fn collect(&mut self) {
        let Some(Stage::Downloading { done, .. }) = &self.stage else {
            return;
        };
        let fetched = match done.try_recv() {
            Ok(fetched) => fetched,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(Error::Io),
        };
        let Some(Stage::Downloading {
            request, thread, ..
        }) = self.stage.take()
        else {
            return;
        };
        // The result is the thread's last act: this join is immediate.
        let _ = thread.join();
        self.finished(request, fetched);
    }
    /// The finished operation, if any; the fetcher is idle afterwards.
    pub fn take_done(&mut self) -> Option<(PluginAdmin, Fetched)> {
        match self.stage.take() {
            Some(Stage::Done { request, fetched }) => Some((request, fetched)),
            other => {
                self.stage = other;
                None
            }
        }
    }
}
