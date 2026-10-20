//! Top bar, connection notices and the status line.
use super::{
    App,
    look::{Icon, Look, icon_button, state_pill},
    string, tr,
};
use crate::{Request, native::Job};
use eframe::egui;
use serde_json::json;

impl App {
    fn folder_title(&self) -> String {
        let Some(board) = &self.board else {
            return "RAD".into();
        };
        match board.folder["fields"]["name"].as_str() {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => tr(self.es, "Main", "Principal").into(),
        }
    }
    pub(super) fn bar(&mut self, ui: &mut egui::Ui, look: &Look) {
        ui.horizontal(|ui| {
            let parent = self
                .board
                .as_ref()
                .and_then(|b| b.folder["fields"]["parent_id"].as_str())
                .map(str::to_owned);
            if let Some(parent) = parent
                && icon_button(ui, look, Icon::Back, tr(self.es, "Back", "Volver")).clicked()
            {
                self.send(Request::new("navigate", json!({"folder_id":parent})));
            }
            ui.label(egui::RichText::new(self.folder_title()).heading());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let settings = ui
                    .add(egui::Button::new(egui::RichText::new("⚙").size(17.)).frame(false))
                    .on_hover_text(tr(self.es, "Settings", "Ajustes"));
                if settings.clicked() {
                    self.settings = self.status["settings"].clone();
                    self.global_permissions = json!({"own":self.status["global_permissions"]});
                    self.settings_open = true;
                }
                ui.add_enabled_ui(self.connected, |ui| {
                    for (op, icon, en, es) in [
                        (
                            "redo",
                            Icon::Redo,
                            "Redo (Ctrl+Shift+Z)",
                            "Rehacer (Ctrl+Shift+Z)",
                        ),
                        ("undo", Icon::Undo, "Undo (Ctrl+Z)", "Deshacer (Ctrl+Z)"),
                    ] {
                        if icon_button(ui, look, icon, tr(self.es, en, es)).clicked() {
                            self.send(Request::new(op, json!({})));
                        }
                    }
                });
                if self.connected {
                    ui.add_space(6.);
                    let dot = match self.status["deck_state"].as_str() {
                        Some("connected") => look.ok,
                        Some("disconnected") => look.warn,
                        _ => look.muted,
                    };
                    let label = crate::status::deck_label(&self.status, self.es);
                    state_pill(ui, look, dot, label);
                }
            });
        });
        if self.attempted && !self.connected {
            ui.add_space(2.);
            ui.horizontal(|ui| {
                ui.colored_label(
                    look.warn,
                    tr(
                        self.es,
                        "RAD is not running. Start rad-runtime, then",
                        "RAD no está en marcha. Inicia rad-runtime y luego",
                    ),
                );
                if ui
                    .add(look.primary(tr(self.es, "Reconnect", "Reconectar")))
                    .clicked()
                {
                    self.command(Job::Connect);
                }
            });
        }
        if let Some(notice) = crate::status::recovery_notice(&self.status, self.es) {
            ui.colored_label(look.warn, notice);
        }
    }
    pub(super) fn status_line(&self, ui: &mut egui::Ui, look: &Look) {
        ui.horizontal(|ui| {
            let color = if self.message_is_error {
                look.danger
            } else {
                look.muted
            };
            let message = ui.label(egui::RichText::new(&self.message).color(color).small());
            if !self.message_code.is_empty() {
                message.on_hover_text(&self.message_code);
            }
            let storage = string(&self.status["storage_state"]);
            if self.connected && !storage.is_empty() && storage != "ready" {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(storage).color(look.warn).small());
                });
            }
        });
    }
}
