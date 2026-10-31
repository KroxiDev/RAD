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
