//! Top bar, connection notices and the status line.
use super::{
    App,
    look::{Icon, Look, icon_button, state_pill},
    string, tr,
};
use crate::service::Action;
use eframe::egui;
use serde_json::json;

impl App {
    fn folder_title(&self) -> String {
        let Some(board) = &self.board else {
            return tr(self.es, "RAD").into();
        };
        match board.folder["fields"]["name"].as_str() {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => tr(self.es, "Main").into(),
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
                && icon_button(ui, look, Icon::Back, tr(self.es, "Back")).clicked()
            {
                self.send(crate::Request::Navigate { folder_id: parent });
            }
            ui.label(egui::RichText::new(self.folder_title()).heading());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let settings = ui
                    .add(egui::Button::new(egui::RichText::new("⚙").size(17.)).frame(false))
                    .on_hover_text(tr(self.es, "Settings"));
                if settings.clicked() {
                    self.settings = self.status["settings"].clone();
                    self.global_permissions = json!({"own":self.status["global_permissions"]});
                    self.settings_open = true;
                }
                ui.add_enabled_ui(self.connected, |ui| {
                    for (op, icon, en) in [
                        ("redo", Icon::Redo, "Redo (Ctrl+Shift+Z)"),
                        ("undo", Icon::Undo, "Undo (Ctrl+Z)"),
                    ] {
                        if icon_button(ui, look, icon, tr(self.es, en)).clicked() {
                            self.send(if op == "redo" {
                                crate::Request::Redo {}
                            } else {
                                crate::Request::Undo {}
                            });
                        }
                    }
                });
                if self.connected {
                    ui.add_space(6.);
                    let dot = match self.status["deck_state"].as_str() {
                        Some("connected") => look.ok,
                        Some("disconnected" | "inaccessible") => look.warn,
                        _ => look.muted,
                    };
                    let label = crate::status::deck_label(&self.status, self.es);
                    state_pill(ui, look, dot, label);
                }
            });
        });
        if self.attempted && !self.connected {
            ui.add_space(2.);
            self.service_bar(ui, look);
        }
        let attention = crate::status::plugin_attention_states(&self.status);
        if self.connected && !attention.is_empty() {
            ui.horizontal_wrapped(|ui| {
                if ui.button(tr(self.es, "Plugins")).clicked() {
                    self.settings = self.status["settings"].clone();
                    self.global_permissions = json!({"own":self.status["global_permissions"]});
                    self.settings_tab = super::settings::Tab::Plugins;
                    self.settings_open = true;
                }
                for state in attention {
                    ui.colored_label(look.warn, state.label(self.es));
                }
            });
        }
        if let Some(notice) = crate::status::recovery_notice(&self.status, self.es) {
            ui.colored_label(look.warn, notice);
        }
    }
    /// "Starting RAD…", or what stopped it with the way out (docs/ux.md); technical text
    /// only under "View details".
    fn service_bar(&mut self, ui: &mut egui::Ui, look: &Look) {
        let Some(failure) = self.service_notice() else {
            ui.colored_label(look.muted, tr(self.es, "Starting RAD…"));
            return;
        };
        ui.horizontal(|ui| {
            ui.colored_label(look.warn, tr(self.es, failure.kind.message()));
            for action in failure.kind.actions() {
                match action {
                    Action::Retry => {
                        if ui.add(look.primary(tr(self.es, "Retry"))).clicked() {
                            self.retry_service(&ui.ctx().clone());
                        }
                    }
                    Action::ViewDetails => {
                        ui.collapsing(tr(self.es, "View details"), |ui| {
                            ui.label(&failure.details);
                        });
                    }
                }
            }
        });
    }
    pub(super) fn status_line(&self, ui: &mut egui::Ui, look: &Look) {
        ui.horizontal(|ui| {
            let color = if self.message_is_error {
                look.danger
            } else {
                look.muted
            };
            ui.label(egui::RichText::new(&self.message).color(color).small());
            if !self.message_code.is_empty() {
                ui.collapsing(tr(self.es, "View details"), |ui| {
                    ui.label(&self.message_code);
                });
            }
            let storage = string(&self.status["storage_state"]);
            if self.connected && !storage.is_empty() && storage != "ready" {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(crate::status::describe_error(&storage, self.es))
                            .color(look.warn)
                            .small(),
                    );
                });
            }
        });
    }
}
