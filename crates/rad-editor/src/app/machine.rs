//! This computer's settings (ADR 0012): using the Stream Deck with RAD and starting RAD with
//! the session. The runtime keeps the truth; the switches only show what it last said.
use super::{App, look::Look, tr};
use crate::{machine::Change, service::Failure};
use eframe::egui;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Pending {
    /// The service worker is changing enablement before publication.
    Autostart(Change),
    /// The one machine mutation is queued or awaiting its answer.
    Saving(Change),
    /// Read the runtime after failure, including after reconnection.
    Reading,
    /// The service worker is applying the runtime's observed setting.
    Restoring,
    /// Terminal failure, awaiting an explicit retry; no operation is in flight.
    Unresolved,
}

impl App {
    /// The «This computer» section of General: only while connected and out of recovery.
    pub(super) fn machine_section(&mut self, ui: &mut egui::Ui, look: &Look) {
        if !self.connected
            || !self.recovery.is_normal()
            || (self.machine.is_none() && self.machine_pending != Some(Pending::Unresolved))
        {
            return;
        }
        ui.add_space(6.);
        ui.label(look.section(tr(self.es, "This computer")));
        if self.machine_pending == Some(Pending::Unresolved) {
            ui.label(tr(
                self.es,
                "Could not change whether RAD starts with the computer",
            ));
            if ui.button(tr(self.es, "Retry")).clicked() {
                self.retry_machine(ui.ctx());
            }
            return;
        }
        let Some(settings) = self.machine else {
            return;
        };
        let free = self.machine_pending.is_none();
        let mut deck = settings.use_stream_deck;
        let mut start = settings.start_with_session;
        let changed_deck = ui
            .add_enabled(
                free,
                egui::Checkbox::new(&mut deck, tr(self.es, "Use the Stream Deck with RAD")),
            )
            .changed();
        let changed_start = ui
            .add_enabled(
                free,
                egui::Checkbox::new(&mut start, tr(self.es, "Start RAD with the computer")),
            )
            .changed();
        if changed_deck {
            self.change_machine(Change::use_stream_deck(deck));
        } else if changed_start {
            self.change_machine(Change::start_with_session(start));
        }
    }
    /// Starting with the session goes through systemd first, when the editor has it.
    fn change_machine(&mut self, change: Change) {
        if self.machine_pending.is_some() {
            return;
        }
        let sent = match (change.autostart(), self.service_worker.as_mut()) {
            (Some(enabled), Some(worker)) => {
                let result = worker.enqueue(crate::service::Job::SetAutostart(enabled));
                if result.is_ok() {
                    self.machine_pending = Some(Pending::Autostart(change));
                } else {
                    self.warn(tr(self.es, "Busy — not sent"));
                }
                return;
            }
            (Some(_), None) => {
                self.machine_pending = Some(Pending::Unresolved);
                self.warn(tr(
                    self.es,
                    "Could not change whether RAD starts with the computer",
                ));
                return;
            }
            _ => change.request(),
        };
        self.machine_pending = Some(Pending::Saving(change));
        if !self.command(crate::native::Job::Request(sent)) {
            self.machine_pending = None;
        }
    }
    /// systemd has answered the autostart step of a change.
    pub(super) fn machine_autostart_done(&mut self, failure: Option<Failure>) {
        let Some(pending) = self.machine_pending else {
            return;
        };
        match pending {
            Pending::Autostart(change) => {
                if let Some(request) = change.after_autostart(failure.is_none()) {
                    self.machine_pending = Some(Pending::Saving(change));
                    if !self.command(crate::native::Job::Request(request)) {
                        self.reconcile_machine();
                    }
                } else {
                    // A failed systemctl may already have changed enablement.
                    self.reconcile_machine();
                }
            }
            Pending::Restoring => {
                self.machine_pending = failure.as_ref().map(|_| Pending::Unresolved);
            }
            _ => return,
        }
        if let Some(failure) = failure {
            self.warn(tr(
                self.es,
                "Could not change whether RAD starts with the computer",
            ));
            self.message_code = failure.details;
        }
    }
    /// Resolve against the runtime before repairing systemd; never replay the mutation.
    pub(super) fn reconcile_machine(&mut self) {
        self.machine_pending = Some(Pending::Reading);
        self.machine_due = true;
    }
    fn retry_machine(&mut self, ctx: &egui::Context) {
        if self.service_worker.is_none() {
            let worker =
                crate::service::worker(crate::service::user_service(), super::SERVICE_SETTLE);
            let wake = ctx.clone();
            worker.set_waker(move || wake.request_repaint());
            self.service_worker = Some(worker);
        }
        self.reconcile_machine();
    }
    /// A failed mutation needs a read; a failed read waits for Retry or reconnect.
    pub(super) fn machine_request_failed(&mut self) {
        match self.machine_pending {
            Some(Pending::Saving(change)) if change.autostart().is_none() => {
                // A deck-only change never touched systemd; only its runtime view is stale.
                self.machine_pending = None;
                self.machine_due = true;
            }
            Some(Pending::Saving(_)) => self.reconcile_machine(),
            Some(Pending::Reading) => self.machine_pending = Some(Pending::Unresolved),
            _ => {}
        }
    }
    /// Answers about this computer's settings and RAD's own log; `true` when it was one.
    pub(super) fn machine_response(
        &mut self,
        operation: &str,
        response: &rad_client::response::Response,
    ) -> bool {
        match (operation, &response.data) {
            ("machine_settings" | "set_machine_settings", data) => {
                let view = serde_json::to_value(data).unwrap_or_default();
                self.machine = crate::machine::Settings::from_value(&view);
                if operation == "set_machine_settings" {
                    self.machine_pending = None;
                    self.say(tr(self.es, "Saved"));
                } else if self.machine_pending == Some(Pending::Reading) {
                    let sent = self.machine.zip(self.service_worker.as_mut()).is_some_and(
                        |(settings, worker)| {
                            worker
                                .enqueue(crate::service::Job::SetAutostart(
                                    settings.start_with_session,
                                ))
                                .is_ok()
                        },
                    );
                    self.machine_pending = Some(if sent {
                        Pending::Restoring
                    } else {
                        Pending::Unresolved
                    });
                }
            }
            ("runtime_log", rad_api::ResponseData::RuntimeLog(log)) => {
                self.runtime_log = Some(log.clone());
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Request,
        native::{Job, Reply},
        worker::Worker,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    fn settings(start: bool) -> crate::machine::Settings {
        crate::machine::Settings {
            use_stream_deck: false,
            start_with_session: start,
        }
    }
    fn answer(start: bool) -> Reply {
        Reply::Response {
            operation: "machine_settings".into(),
            response: rad_api::Response {
                v: 1,
                id: None,
                session_id: None,
                status: rad_api::Status::Ok,
                error: None,
                data: rad_api::ResponseData::MachineSettings(rad_api::MachineSettingsView {
                    use_stream_deck: false,
                    start_with_session: start,
                }),
            },
        }
    }
    fn app(enabled: Arc<AtomicBool>) -> App {
        let root = std::env::temp_dir().join(format!("rad-machine-test-{}", std::process::id()));
        let mut app = App::with_preferences_path(root.clone(), root.join("theme"), None);
        app.connected = true;
        app.machine = Some(settings(false));
        app.service_worker = Some(Worker::spawn(move |job| {
            if let crate::service::Job::SetAutostart(value) = job {
                enabled.store(value, Ordering::SeqCst);
            }
            crate::service::Outcome { job, failure: None }
        }));
        app
    }
    fn settle(app: &mut App) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            app.pump(&ctx);
            if (app.machine_pending.is_none() || app.machine_pending == Some(Pending::Unresolved))
                && app
                    .service_worker
                    .as_ref()
                    .is_none_or(|worker| !worker.busy())
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "machine change did not settle"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn autostart_is_restored_when_publication_cannot_be_queued() {
        let enabled = Arc::new(AtomicBool::new(false));
        let mut app = app(enabled.clone());
        let (release, blocked) = mpsc::channel();
        let mut first = true;
        app.worker = Worker::spawn(move |job| {
            if std::mem::take(&mut first) {
                blocked.recv().unwrap();
            }
            super::super::reload_tests::event(Ok(match job {
                Job::Request(Request::MachineSettings {}) => answer(false),
                Job::Request(Request::SetMachineSettings { .. }) => {
                    panic!("refused mutation must not be replayed")
                }
                _ => Reply::Theme,
            }))
        });
        app.worker.submit(Job::Theme).unwrap();
        app.worker.enqueue(Job::Theme).unwrap();
        app.change_machine(Change::start_with_session(true));
        let ctx = egui::Context::default();
        while app.service_worker.as_ref().unwrap().busy() {
            app.pump_service(&ctx);
            std::thread::yield_now();
        }
        release.send(()).unwrap();
        settle(&mut app);
        assert!(
            !enabled.load(Ordering::SeqCst),
            "autostart disagrees with the runtime after a refused send"
        );
        assert_eq!(app.machine, Some(settings(false)));
    }

    #[test]
    fn failed_publication_is_resolved_from_runtime_without_replay() {
        for (unknown, persisted) in [(false, false), (true, false), (true, true)] {
            let enabled = Arc::new(AtomicBool::new(false));
            let mut app = app(enabled.clone());
            let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let sent = attempts.clone();
            let mut connected = true;
            app.worker = Worker::spawn(move |job| {
                let mut event = super::super::reload_tests::event(match job {
                    Job::Request(Request::SetMachineSettings { .. }) => {
                        connected = !unknown;
                        sent.fetch_add(1, Ordering::SeqCst);
                        Ok(Reply::MachineFailed(crate::session::Error {
                            code: "storage_error".into(),
                            cause: None,
                            source_change: None,
                            unknown,
                        }))
                    }
                    Job::Connect => {
                        connected = true;
                        Ok(Reply::Connected)
                    }
                    Job::Request(Request::MachineSettings {}) => Ok(answer(persisted)),
                    _ => Ok(Reply::Theme),
                });
                event.connected = connected;
                event
            });
            app.change_machine(Change::start_with_session(true));
            settle(&mut app);
            assert_eq!(enabled.load(Ordering::SeqCst), persisted);
            assert_eq!(app.machine, Some(settings(persisted)));
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn missing_service_worker_never_publishes_an_autostart_change() {
        let mut app = app(Arc::new(AtomicBool::new(false)));
        app.worker = Worker::spawn(|_| panic!("autostart must not be published without systemd"));
        app.service_worker = None;
        app.change_machine(Change::start_with_session(true));
        assert!(
            !app.worker.busy(),
            "no request may be sent without its systemd step"
        );
        let output = egui::Context::default().run_ui(Default::default(), |ui| {
            app.machine_section(ui, &Look::new(&Default::default()));
        });
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::epaint::Shape::Text(text) if text.galley.job.text == "Retry")));
    }

    #[test]
    fn a_failed_deck_only_change_rereads_without_touching_systemd() {
        for unknown in [false, true] {
            let mut app = app(Arc::new(AtomicBool::new(true)));
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let record = calls.clone();
            app.service_worker = Some(Worker::spawn(move |job| {
                record.fetch_add(1, Ordering::SeqCst);
                crate::service::Outcome { job, failure: None }
            }));
            let mutations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let sent = mutations.clone();
            let mut connected = true;
            app.worker = Worker::spawn(move |job| {
                let mut event = super::super::reload_tests::event(Ok(match job {
                    Job::Request(Request::SetMachineSettings { .. }) => {
                        sent.fetch_add(1, Ordering::SeqCst);
                        connected = !unknown;
                        Reply::MachineFailed(crate::session::Error {
                            code: "storage_error".into(),
                            cause: None,
                            source_change: None,
                            unknown,
                        })
                    }
                    Job::Connect => {
                        connected = true;
                        Reply::Connected
                    }
                    Job::Request(Request::MachineSettings {}) => answer(true),
                    _ => Reply::Theme,
                }));
                event.connected = connected;
                event
            });
            app.change_machine(Change::use_stream_deck(false));
            let ctx = egui::Context::default();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while app.machine != Some(settings(true)) || app.service_worker.as_ref().unwrap().busy()
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "machine setting was not reread"
                );
                app.pump(&ctx);
                std::thread::yield_now();
            }
            assert_eq!(
                calls.load(Ordering::SeqCst),
                0,
                "a deck-only failure must not change systemd"
            );
            assert_eq!(
                mutations.load(Ordering::SeqCst),
                1,
                "failed mutation must not be replayed"
            );
            assert!(app.machine_pending.is_none());
        }
    }

    #[test]
    fn a_direct_machine_change_prevents_overlapping_autostart() {
        let enabled = Arc::new(AtomicBool::new(false));
        let mut app = app(enabled.clone());
        app.worker = Worker::spawn(|job| {
            super::super::reload_tests::event(Ok(match job {
                Job::Request(Request::SetMachineSettings {
                    use_stream_deck: Some(false),
                    ..
                }) => {
                    let mut reply = answer(false);
                    if let Reply::Response { operation, .. } = &mut reply {
                        *operation = "set_machine_settings".into();
                    }
                    reply
                }
                Job::Request(Request::SetMachineSettings { .. }) => panic!("overlapping change"),
                _ => Reply::Theme,
            }))
        });
        app.change_machine(Change::use_stream_deck(false));
        app.change_machine(Change::start_with_session(true));
        assert!(!app.service_worker.as_ref().unwrap().busy());
        settle(&mut app);
        assert!(!enabled.load(Ordering::SeqCst));
        assert!(app.machine_pending.is_none());
    }

    #[test]
    fn successful_change_stays_pending_until_the_runtime_confirms_it() {
        for use_deck in [false, true] {
            let enabled = Arc::new(AtomicBool::new(false));
            let mut app = app(enabled.clone());
            app.worker = Worker::spawn(move |job| {
                super::super::reload_tests::event(Ok(match job {
                    Job::Request(Request::SetMachineSettings {
                        start_with_session: Some(true),
                        use_stream_deck,
                    }) => {
                        assert_eq!(use_stream_deck, use_deck.then_some(true));
                        let mut reply = answer(true);
                        if let Reply::Response {
                            operation,
                            response,
                        } = &mut reply
                        {
                            *operation = "set_machine_settings".into();
                            if let rad_api::ResponseData::MachineSettings(settings) =
                                &mut response.data
                            {
                                settings.use_stream_deck = use_deck;
                            }
                        }
                        reply
                    }
                    _ => Reply::Theme,
                }))
            });
            app.change_machine(if use_deck {
                Change::use_stream_deck(true)
            } else {
                Change::start_with_session(true)
            });
            let ctx = egui::Context::default();
            while app.service_worker.as_ref().unwrap().busy() {
                app.pump_service(&ctx);
                std::thread::yield_now();
            }
            assert!(app.machine_pending.is_some());
            assert_eq!(app.machine, Some(settings(false)));
            settle(&mut app);
            assert!(enabled.load(Ordering::SeqCst));
            assert_eq!(
                app.machine,
                Some(crate::machine::Settings {
                    use_stream_deck: use_deck,
                    start_with_session: true
                })
            );
            assert!(app.machine_pending.is_none());
        }
    }

    #[test]
    fn failed_reconciliation_hides_the_switches_and_allows_an_explicit_retry() {
        let enabled = Arc::new(AtomicBool::new(true));
        let mut app = app(enabled.clone());
        app.worker = Worker::spawn(|_| super::super::reload_tests::event(Ok(answer(false))));
        app.service_worker = Some(Worker::spawn(|job| crate::service::Outcome {
            job,
            failure: Some(Failure {
                kind: crate::service::FailureKind::CouldNotStart,
                details: "synthetic failure".into(),
            }),
        }));
        app.reconcile_machine();
        settle(&mut app);
        assert!(enabled.load(Ordering::SeqCst));
        assert!(app.machine_pending == Some(Pending::Unresolved));
        // Reconnection may have cleared the last view before a failed reread.
        app.machine = None;
        let ctx = egui::Context::default();
        let output = ctx.run_ui(Default::default(), |ui| {
            app.machine_section(ui, &Look::new(&Default::default()));
        });
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                _ => None,
            })
            .collect();
        assert!(!texts.contains(&"Start RAD with the computer"));
        assert!(texts.contains(&"Retry"));
        // Retry also recreates a dead service worker. The new worker stays idle until
        // the runtime answers; replace it with a double before any systemd work.
        app.service_worker = None;
        let position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text == "Retry" => {
                    Some(text.pos + text.galley.rect.center().to_vec2())
                }
                _ => None,
            })
            .unwrap();
        for pressed in [true, false] {
            let _ = ctx.run_ui(
                egui::RawInput {
                    events: vec![
                        egui::Event::PointerMoved(position),
                        egui::Event::PointerButton {
                            pos: position,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ui| app.machine_section(ui, &Look::new(&Default::default())),
            );
        }
        assert!(app.machine_pending == Some(Pending::Reading));
        assert!(
            app.service_worker
                .as_ref()
                .is_some_and(|worker| !worker.busy())
        );
        let actual = enabled.clone();
        app.service_worker = Some(Worker::spawn(move |job| {
            if let crate::service::Job::SetAutostart(value) = job {
                enabled.store(value, Ordering::SeqCst);
            }
            crate::service::Outcome { job, failure: None }
        }));
        settle(&mut app);
        assert!(app.machine_pending.is_none());
        assert!(!actual.load(Ordering::SeqCst));
    }

    #[test]
    fn a_failed_or_disconnected_reconciliation_is_not_an_update_in_flight() {
        let mut app = app(Arc::new(AtomicBool::new(false)));
        app.worker = Worker::spawn(|_| super::super::reload_tests::event(Ok(Reply::Theme)));
        app.machine_pending = Some(Pending::Unresolved);
        assert!(!app.update_pending());
        app.machine_pending = Some(Pending::Reading);
        app.connected = false;
        assert!(!app.update_pending());
        app.connected = true;
        assert!(
            app.update_pending(),
            "a runnable reconciliation still precedes handoff"
        );
    }
}
