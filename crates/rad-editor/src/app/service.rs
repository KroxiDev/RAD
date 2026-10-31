//! The runtime as a service: start it when it does not answer, and say plainly when it cannot.
use super::App;
use crate::{
    service::{Failure, FailureKind, Job, Outcome},
    worker::Polled,
};
use eframe::egui;

#[derive(Default)]
pub(super) struct ServiceUi {
    /// Bootstrap was attempted or a connection already succeeded; never undo a stop.
    tried: bool,
    /// The next failed connection asks systemd why.
    inspect_due: bool,
    /// Something was asked of systemd and the verdict is not in.
    waiting: bool,
    failure: Option<Failure>,
}

impl App {
    /// One editor launch refreshes the installed units even when the runtime already answers.
    pub(super) fn bootstrap_service(&mut self) {
        if self.service.tried {
            return;
        }
        if self
            .service_worker
            .as_mut()
            .is_some_and(|worker| worker.submit(Job::Start).is_ok())
        {
            self.service = ServiceUi {
                tried: true,
                waiting: true,
                ..Default::default()
            };
        }
    }

    /// Starts the service when the runtime does not answer, at most once per attempt, and
    /// asks systemd why when it still does not. Called every turn; it sleeps when idle.
    pub(super) fn pump_service(&mut self, ctx: &egui::Context) {
        if self.connected {
            // After connecting, only systemd owns restart policy. Otherwise losing a
            // connection after the tray's Stop would silently start the service again.
            self.service = ServiceUi {
                tried: true,
                ..Default::default()
            };
        }
        let Some(worker) = self.service_worker.as_mut() else {
            return;
        };
        match worker.poll() {
            Polled::Ready(outcome) => self.service_done(ctx, outcome),
            Polled::Failed { .. } | Polled::Closed { .. } => {
                self.service_worker = None;
                self.machine_pending = None;
                self.service.waiting = false;
                self.service.failure = Some(Failure {
                    kind: FailureKind::CouldNotStart,
                    details: "the editor's service worker stopped".into(),
                });
            }
            Polled::Pending => {}
        }
        let service_idle = self.service_worker.as_ref().is_some_and(|w| !w.busy());
        let silent = self.attempted
            && !self.connected
            && !self.worker.busy()
            && !self.reconnect_needed
            && !self.worker_respawn.gave_up();
        if !(service_idle && silent) {
            return;
        }
        let job = if !self.service.tried {
            Job::Start
        } else if self.service.inspect_due {
            Job::Inspect
        } else {
            return;
        };
        let submitted = self
            .service_worker
            .as_mut()
            .is_some_and(|w| w.submit(job).is_ok());
        if submitted {
            self.service.tried = true;
            self.service.inspect_due = false;
            self.service.waiting = true;
        }
    }
    /// The user asked again: reset the unit, start it, connect.
    pub(super) fn retry_service(&mut self, ctx: &egui::Context) {
        let Some(worker) = self.service_worker.as_mut() else {
            return self.reconnect(ctx);
        };
        if worker.submit(Job::Retry).is_ok() {
            self.service = ServiceUi {
                tried: true,
                waiting: true,
                ..Default::default()
            };
        }
    }
    fn service_done(&mut self, ctx: &egui::Context, outcome: Outcome) {
        ctx.request_repaint();
        if matches!(outcome.job, Job::SetAutostart(_)) {
            return self.machine_autostart_done(outcome.failure);
        }
        match outcome.failure {
            Some(failure) => {
                self.service.failure = Some(failure);
                self.service.waiting = false;
            }
            None if outcome.job == Job::Inspect || self.connected => {
                // Bootstrap also updates units for an already-active runtime. Do not
                // replace a healthy RPC session just because its idempotent Start ended.
                self.service.waiting = false;
            }
            None => {
                // Started: connect once; if the runtime still is silent, say why.
                self.service.inspect_due = true;
                self.reconnect(ctx);
            }
        }
    }
    /// What the bar says while the runtime does not answer: `None` means "starting".
    pub(super) fn service_notice(&self) -> Option<Failure> {
        self.service.failure.clone().or_else(|| {
            (!self.service.waiting && !self.worker.busy()).then(|| Failure {
                kind: FailureKind::CouldNotStart,
                details: "RAD did not answer".into(),
            })
        })
    }
}
