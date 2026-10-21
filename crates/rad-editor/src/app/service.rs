//! The runtime as a service: start it when it does not answer, and say plainly when it cannot.
use super::App;
use crate::{
    service::{Failure, FailureKind, Job, Outcome},
    worker::Polled,
};
use eframe::egui;

#[derive(Default)]
pub(super) struct ServiceUi {
    /// A start was asked for since the last connection: one per attempt, never a loop.
    tried: bool,
    /// The next failed connection asks systemd why.
    inspect_due: bool,
    /// Something was asked of systemd and the verdict is not in.
    waiting: bool,
    failure: Option<Failure>,
}

impl App {
    /// Starts the service when the runtime does not answer, at most once per attempt, and
    /// asks systemd why when it still does not. Called every turn; it sleeps when idle.
    pub(super) fn pump_service(&mut self, ctx: &egui::Context) {
        if self.connected {
            self.service = ServiceUi::default();
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
            None if outcome.job == Job::Inspect => self.service.waiting = false,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        native::{Event, Job as Main, Reply},
        theme::Palette,
        worker::Worker,
    };
    use std::sync::{Arc, Mutex};

    type Log<T> = Arc<Mutex<Vec<T>>>;

    fn unavailable() -> Event {
        Event {
            plugin_apply: false,
            plugin_admin: false,
            plugin_manifest: false,
            plugin_dependencies: false,
            plugin_form: false,
            plugin_visual: false,
            connected: false,
            transfer_job: false,
            palette: Palette::default(),
            result: Err(crate::session::Error {
                code: "runtime_unavailable".into(),
                cause: None,
                source_change: None,
                unknown: false,
            }),
        }
    }
    fn connected() -> Event {
        Event {
            connected: true,
            result: Ok(Reply::Connected),
            ..unavailable()
        }
    }
    /// An editor whose connection succeeds from the `ok_from`-th attempt (1-based; 0 never)
    /// and whose service answers each job as `script` says. Returns what each was asked.
    fn editor(
        ok_from: usize,
        script: impl Fn(Job) -> Option<Failure> + Send + 'static,
    ) -> (App, Log<&'static str>, Log<Job>) {
        let root =
            std::env::temp_dir().join(format!("rad-editor-service-ui-{}", std::process::id()));
        let mut app = App::with_preferences_path(root.clone(), root.join("no-theme"), None);
        app.es = crate::preferences::Language::En;
        let connects: Log<&'static str> = Default::default();
        let record = connects.clone();
        app.worker = Worker::spawn(move |job: Main| {
            if !matches!(job, Main::Connect) {
                // What a connected editor asks next is not under test.
                return Event {
                    result: Err(crate::session::Error::protocol()),
                    ..connected()
                };
            }
            let mut seen = record.lock().unwrap();
            seen.push("connect");
            if ok_from != 0 && seen.len() >= ok_from {
                connected()
            } else {
                unavailable()
            }
        });
        let asked: Log<Job> = Default::default();
        let record = asked.clone();
        app.service_worker = Some(Worker::spawn(move |job| {
            record.lock().unwrap().push(job);
            Outcome {
                job,
                failure: script(job),
            }
        }));
        app.worker.submit(Main::Connect).unwrap();
        (app, connects, asked)
    }
    fn pump_until(app: &mut App, done: impl Fn(&App) -> bool) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !done(app) {
            assert!(
                std::time::Instant::now() < deadline,
                "the editor never settled"
            );
            app.pump(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    /// Lets the editor run well past the point where a loop would have shown itself.
    fn quiet(app: &mut App) {
        let ctx = egui::Context::default();
        for _ in 0..150 {
            app.pump(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    fn stopped() -> Failure {
        Failure {
            kind: FailureKind::StoppedByError,
            details: "Result=start-limit-hit".into(),
        }
    }

    #[test]
    fn an_unreachable_runtime_is_started_once_and_the_editor_connects_again() {
        let (mut app, connects, asked) = editor(2, |_| None);
        pump_until(&mut app, |app| app.connected);
        quiet(&mut app);
        assert_eq!(*asked.lock().unwrap(), [Job::Start]);
        assert_eq!(connects.lock().unwrap().len(), 2);
        assert!(app.service.failure.is_none() && !app.service.waiting);
    }

    #[test]
    fn a_runtime_that_stays_silent_is_explained_once_and_waits_for_the_user() {
        let (mut app, connects, asked) = editor(0, |job| (job == Job::Inspect).then(stopped));
        pump_until(&mut app, |app| app.service.failure.is_some());
        quiet(&mut app);
        assert_eq!(*asked.lock().unwrap(), [Job::Start, Job::Inspect]);
        assert_eq!(connects.lock().unwrap().len(), 2, "no connection loop");
        assert_eq!(
            app.service.failure.as_ref().unwrap().kind,
            FailureKind::StoppedByError
        );
        // Reintentar: reset and start, connect once, explain once more.
        app.retry_service(&egui::Context::default());
        pump_until(&mut app, |_| asked.lock().unwrap().len() == 4);
        quiet(&mut app);
        assert_eq!(
            *asked.lock().unwrap(),
            [Job::Start, Job::Inspect, Job::Retry, Job::Inspect]
        );
        assert_eq!(connects.lock().unwrap().len(), 3);
    }

    #[test]
    fn a_start_that_fails_is_shown_and_not_repeated_until_the_user_retries() {
        let (mut app, connects, asked) = editor(0, |_| Some(stopped()));
        pump_until(&mut app, |app| app.service.failure.is_some());
        quiet(&mut app);
        assert_eq!(*asked.lock().unwrap(), [Job::Start]);
        assert_eq!(connects.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_bar_names_the_state_in_plain_words_and_keeps_the_technical_side_for_details() {
        let (mut app, _, _) = editor(0, |_| None);
        app.service.failure = Some(Failure {
            kind: FailureKind::CouldNotStart,
            details: "Result=exit-code, exit code 75".into(),
        });
        app.attempted = true;
        let look = super::super::look::Look::new(&Default::default());
        let ctx = egui::Context::default();
        let output = ctx.run_ui(Default::default(), |ui| app.bar(ui, &look));
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.job.text.clone()),
                _ => None,
            })
            .collect();
        for expected in ["RAD could not start", "Retry", "View details"] {
            assert!(texts.iter().any(|t| t == expected), "{expected}: {texts:?}");
        }
        assert!(
            !texts
                .iter()
                .any(|t| t.contains("75") || t.contains("rad-runtime")),
            "{texts:?}"
        );
    }
}
