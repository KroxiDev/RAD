//! Recovery mode (ADR 0011): when the runtime cannot open the configuration the editor shows
//! what happened and only the ways out the runtime offers, in place of the normal window.
use super::{App, look::Look, tr};
use crate::{
    native::{Job, TransferKind},
    recovery::{Action, Mode, Screen},
};
use eframe::egui;
use serde_json::Value;
use std::path::PathBuf;

/// Width of the recovery screen's content.
const WIDTH: f32 = 440.;

impl App {
    /// The runtime refused a request because it is in recovery mode: nothing else is read
    /// until it has said why and the user has chosen a way out.
    pub(super) fn enter_recovery(&mut self) {
        if !self.recovery.is_normal() {
            return;
        }
        self.recovery = Mode::Asking;
        self.recovery_due = true;
        self.recovery_reset_confirm = false;
        self.restore_token = None;
        self.force = false;
        self.overview_due = false;
        self.result_due = false;
        self.inspect_needed = false;
        self.catalog_due = None;
        self.catalog_read = None;
        self.app_picker = Default::default();
        self.machine_due = false;
        if self.navigation.screen()
            != crate::screens::Screen::Settings(crate::screens::Section::Updates)
        {
            self.navigation.discard();
        }
        self.say("");
    }
    /// The runtime works normally again, on this same connection: read everything anew, as
    /// after a reconnection.
    pub(super) fn leave_recovery(&mut self, message: &str) {
        self.recovery = Mode::Normal;
        self.recovery_due = false;
        self.recovery_reset_confirm = false;
        self.restore_token = None;
        self.restore_summary.clear();
        self.transfer_active = false;
        self.plugins.reconnect();
        self.credentials.clear();
        self.status = Value::Null;
        self.board = None;
        self.machine = None;
        self.force = true;
        self.overview_due = true;
        self.catalog_due = Some(0);
        self.machine_due = true;
        self.say(message);
    }
    /// Answers of the runtime that concern recovery mode; `true` when this was one of them.
    pub(super) fn recovery_response(
        &mut self,
        operation: &str,
        response: &rad_client::response::Response,
    ) -> bool {
        let applied = response.status == rad_client::response::Status::Applied;
        match operation {
            "recovery" => {
                if let Ok(view) = serde_json::to_value(&response.data) {
                    self.recovery = Mode::Active(Screen::from_value(&view));
                }
            }
            "recovery_reset" if applied => {
                self.leave_recovery(tr(self.es, "Started with an empty configuration"));
            }
            "restore_commit" if applied && !self.recovery.is_normal() => {
                self.leave_recovery(tr(self.es, "Configuration restored"));
            }
            _ => return false,
        }
        true
    }
    /// The whole window while the runtime is in recovery mode.
    pub(super) fn recovery_view(&mut self, ui: &mut egui::Ui, look: &Look) {
        egui::Panel::top("bar")
            .frame(look.panel(look.surface, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tr(self.es, "RAD")).heading());
                    if ui.button(tr(self.es, "Updates")).clicked() {
                        self.open_settings(crate::screens::Section::Updates);
                    }
                });
            });
        egui::Panel::bottom("status")
            .frame(
                look.panel(look.surface, 4)
                    .inner_margin(egui::Margin::symmetric(14, 5)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.status_line(ui, look));
        egui::CentralPanel::default()
            .frame(look.panel(look.background, 24))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| match self.recovery.clone() {
                    Mode::Active(screen) => {
                        ui.set_max_width(WIDTH);
                        self.recovery_screen(ui, look, &screen);
                    }
                    _ => {
                        ui.spinner();
                    }
                })
            });
    }
    fn recovery_screen(&mut self, ui: &mut egui::Ui, look: &Look, screen: &Screen) {
        ui.label(egui::RichText::new(screen.title(self.es)).heading());
        ui.add_space(12.);
        if screen.offers(Action::Restore) {
            ui.add(
                look.field(&mut self.backup_path, tr(self.es, "/path/to/backup.rad"))
                    .desired_width(f32::INFINITY),
            );
            let restore = ui.add_enabled(
                !self.transfer_active && !self.backup_path.trim().is_empty(),
                look.primary(Action::Restore.label(self.es)),
            );
            if restore.clicked()
                && self.command(Job::Transfer {
                    path: PathBuf::from(self.backup_path.trim()),
                    kind: TransferKind::Restore,
                })
            {
                self.transfer_active = true;
            }
            self.restore_confirmation(ui, look);
            ui.add_space(8.);
        }
        if screen.offers(Action::Reset) {
            if self.recovery_reset_confirm {
                ui.label(look.muted_text(tr(
                    self.es,
                    "This starts again with an empty configuration. The damaged file is kept aside.",
                )));
                ui.horizontal(|ui| {
                    if ui
                        .add(look.dangerous(tr(self.es, "Yes, start over")))
                        .clicked()
                    {
                        self.send(crate::Request::RecoveryReset { confirm: true });
                        self.recovery_reset_confirm = false;
                    }
                    if ui.button(tr(self.es, "Cancel")).clicked() {
                        self.recovery_reset_confirm = false;
                    }
                });
            } else if ui
                .add(look.dangerous(Action::Reset.label(self.es)))
                .clicked()
            {
                self.recovery_reset_confirm = true;
            }
            ui.add_space(8.);
        }
        if screen.offers(Action::Details) {
            ui.collapsing(Action::Details.label(self.es), |ui| {
                ui.label(screen.details(self.es));
            });
        }
    }
}
