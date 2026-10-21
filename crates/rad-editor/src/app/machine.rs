//! This computer's settings (ADR 0012): using the Stream Deck with RAD and starting RAD with
//! the session. The runtime keeps the truth; the switches only show what it last said.
use super::{App, look::Look, tr};
use crate::{machine::Change, service::Failure};
use eframe::egui;

impl App {
    /// The «This computer» section of General: only while connected and out of recovery.
    pub(super) fn machine_section(&mut self, ui: &mut egui::Ui, look: &Look) {
        let Some(settings) = self.machine else {
            return;
        };
        if !self.connected || !self.recovery.is_normal() {
            return;
        }
        ui.add_space(6.);
        ui.label(look.section(tr(self.es, "This computer")));
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
        let sent = match (change.autostart(), self.service_worker.as_mut()) {
            (Some(enabled), Some(worker)) => {
                let result = worker.enqueue(crate::service::Job::SetAutostart(enabled));
                if result.is_ok() {
                    self.machine_pending = Some(change);
                } else {
                    self.warn(tr(self.es, "Busy — not sent"));
                }
                return;
            }
            _ => change.request(),
        };
        self.send(sent);
    }
    /// systemd has answered the autostart step of a change.
    pub(super) fn machine_autostart_done(&mut self, failure: Option<Failure>) {
        let Some(change) = self.machine_pending.take() else {
            return;
        };
        match change.after_autostart(failure.is_none()) {
            Some(request) => self.send(request),
            None => {
                self.warn(tr(
                    self.es,
                    "Could not change whether RAD starts with the computer",
                ));
                self.message_code = failure.map(|f| f.details).unwrap_or_default();
            }
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
                    self.say(tr(self.es, "Saved"));
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
        native::{Event, Job as Main, Reply},
        service::{FailureKind, Job, Outcome},
        theme::Palette,
        worker::Worker,
    };
    use std::sync::{Arc, Mutex};

    /// An editor whose systemd answers `autostart` and whose runtime only records requests.
    fn editor(autostart: Option<Failure>) -> (App, Arc<Mutex<Vec<String>>>) {
        let root = std::env::temp_dir().join(format!("rad-editor-machine-{}", std::process::id()));
        let mut app = App::with_preferences_path(root.clone(), root.join("no-theme"), None);
        app.es = crate::preferences::Language::En;
        let sent: Arc<Mutex<Vec<String>>> = Default::default();
        let record = sent.clone();
        app.worker = Worker::spawn(move |job: Main| {
            // The plugin polling that goes on meanwhile is not under test.
            if let Main::Request(request) = job
                && request.operation().as_str() == "set_machine_settings"
            {
                record
                    .lock()
                    .unwrap()
                    .push(serde_json::to_string(&request).unwrap());
            }
            Event {
                plugin_apply: false,
                plugin_admin: false,
                plugin_manifest: false,
                plugin_dependencies: false,
                plugin_form: false,
                plugin_visual: false,
                connected: true,
                transfer_job: false,
                palette: Palette::default(),
                result: Ok(Reply::NoAppIcon),
            }
        });
        app.service_worker = Some(Worker::spawn(move |job: Job| Outcome {
            job,
            failure: autostart.clone(),
        }));
        app.connected = true;
        (app, sent)
    }
    fn settle(app: &mut App, sent: &Arc<Mutex<Vec<String>>>, expected: usize) {
        let ctx = egui::Context::default();
        for _ in 0..200 {
            app.pump(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let seen = sent.lock().unwrap().clone();
        assert_eq!(seen.len(), expected, "{seen:?}");
    }

    #[test]
    fn a_failed_autostart_saves_nothing_in_the_runtime_and_says_so() {
        let failure = Failure {
            kind: FailureKind::CouldNotStart,
            details: "systemctl --user enable: denied".into(),
        };
        let (mut app, sent) = editor(Some(failure));
        app.change_machine(Change::use_stream_deck(true));
        settle(&mut app, &sent, 0);
        assert!(app.machine_pending.is_none());
        assert!(app.message_is_error);
        assert_eq!(app.message_code, "systemctl --user enable: denied");
    }

    #[test]
    fn a_working_autostart_is_followed_by_the_runtime_saving_both_switches() {
        let (mut app, sent) = editor(None);
        app.change_machine(Change::use_stream_deck(true));
        settle(&mut app, &sent, 1);
        let request: serde_json::Value = serde_json::from_str(&sent.lock().unwrap()[0]).unwrap();
        assert_eq!(request["args"]["use_stream_deck"], true);
        assert_eq!(request["args"]["start_with_session"], true);
    }
}
