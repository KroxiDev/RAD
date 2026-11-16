//! The runtime as a service: start it when it does not answer, and say plainly when it cannot.
use super::{App, look::Look, tr};
use crate::{
    service::{AutostartWarning, Failure, FailureKind, Job, Outcome},
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
    /// Autostart needs attention even if the runtime is already connected.
    warning: Option<AutostartWarning>,
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
                warning: self.service.warning.take(),
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
                if self.machine_pending.is_some() {
                    self.machine_pending = Some(super::machine::Pending::Unresolved);
                }
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
        if outcome.failure.is_none() && outcome.job != Job::Inspect {
            self.service.warning = outcome.warning;
        }
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

    pub(super) fn service_warning_bar(&mut self, ui: &mut egui::Ui, look: &Look) {
        let Some(warning) = self.service.warning.clone() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                look.warn,
                tr(
                    self.es,
                    "Could not change whether RAD starts with the computer",
                ),
            );
            ui.collapsing(tr(self.es, "View details"), |ui| {
                ui.label(&warning.details);
            });
            if ui.button(tr(self.es, "Dismiss")).clicked() {
                self.service.warning = None;
            }
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        service::{Answer, Service, Systemctl, UNIT, Unavailable},
        worker::Worker,
    };

    #[test]
    fn autostart_migration_warning_remains_visible_after_connecting() {
        struct MigrationFails;
        impl Systemctl for MigrationFails {
            fn run(&self, args: &[&str]) -> Result<Answer, Unavailable> {
                Ok(Answer {
                    code: Some(i32::from(args[0] == "reenable")),
                    stdout: if args[0] == "show" {
                        "ActiveState=active\nResult=success\nExecMainStatus=0\n".into()
                    } else {
                        String::new()
                    },
                    stderr: "synthetic migration error".into(),
                })
            }
        }

        let root =
            std::env::temp_dir().join(format!("rad-autostart-warning-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let units = root.join("units");
        std::fs::create_dir_all(units.join("default.target.wants")).unwrap();
        std::fs::write(root.join("rad-runtime"), b"").unwrap();
        std::os::unix::fs::symlink(
            units.join(UNIT),
            units.join("default.target.wants").join(UNIT),
        )
        .unwrap();
        let mut app = App::with_preferences_path(root.clone(), root.join("theme"), None);
        app.connected = true;
        app.service_worker = Some(crate::service::worker(
            Ok(Service::new(MigrationFails, units, root.join("rad-editor"))),
            std::time::Duration::ZERO,
        ));
        app.worker = Worker::spawn(|job| {
            assert!(matches!(job, crate::native::Job::Connect));
            super::super::reload_tests::event(Ok(crate::native::Reply::Connected))
        });
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        app.bootstrap_service();
        while app.service_worker.as_ref().unwrap().busy() {
            app.pump_service(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "service never settled"
            );
            std::thread::yield_now();
        }
        app.worker.submit(crate::native::Job::Connect).unwrap();
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "connection never settled"
            );
            std::thread::yield_now();
        }
        app.pump_service(&ctx);
        assert!(app.connected);

        for language in [
            crate::preferences::Language::En,
            crate::preferences::Language::Es,
        ] {
            app.es = language;
            let output = ctx.run_ui(Default::default(), |ui| {
                app.bar(ui, &super::super::look::Look::new(&Default::default()));
            });
            let text: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect();
            assert!(text.contains(&super::super::tr(
                language,
                "Could not change whether RAD starts with the computer"
            )));
            assert!(text.contains(&super::super::tr(language, "View details")));
            assert!(text.contains(&super::super::tr(language, "Dismiss")));
            assert!(!text.contains(&super::super::tr(language, "RAD could not start")));
            assert!(
                !text
                    .iter()
                    .any(|line| line.contains("synthetic migration error")),
                "technical details must stay collapsed"
            );
        }
    }
}
