//! Settings window: device, language, backup, plugins and global automation permissions,
//! one tab at a time.
use super::{App, inspector::permission_ui, look::Look, tr};
use crate::native::{Job, TransferKind};
use eframe::egui;
use serde_json::json;
use std::path::PathBuf;

/// Width of the settings window's content; the tabs and fields are laid out for it.
const WIDTH: f32 = 440.;
/// Width of the labels in the General tab, so the values line up.
const LABEL: f32 = 170.;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Tab {
    #[default]
    General,
    Backup,
    Plugins,
    Permissions,
}

impl App {
    pub(super) fn settings_window(&mut self, ctx: &egui::Context, look: &Look) {
        if !self.settings_open {
            return;
        }
        let mut open = true;
        egui::Window::new(tr(self.es, "Settings"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .fixed_size([WIDTH, 0.])
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                self.settings_tabs(ui);
                ui.separator();
                ui.add_space(4.);
                ui.add_enabled_ui(self.connected || self.settings_tab == Tab::General, |ui| match self.settings_tab {
                    Tab::General => self.device(ui, look),
                    Tab::Backup => self.backup(ui, look),
                    Tab::Plugins => {
                        egui::ScrollArea::vertical()
                            .max_height(ui.ctx().content_rect().height() * 0.7)
                            .show(ui, |ui| self.plugin_settings(ui));
                    },
                    Tab::Permissions => {
                        ui.label(look.muted_text(tr(self.es, "What rad-cli and scripts may do, unless a folder or key says otherwise.")));
                        ui.add_space(4.);
                        if permission_ui(ui, &mut self.global_permissions, self.es, false) {
                            self.send(crate::Request::SetPermissions {target:"global".into(),value:serde_json::from_value(self.global_permissions.clone()).expect("permissions draft")});
                        }
                    }
                });
                ui.collapsing(tr(self.es, "Advanced"), |ui| {
                    ui.collapsing(tr(self.es, "Log"), |ui| self.plugin_log(ui));
                });
            });
        // Plugin status is asked for only while its tab is in view.
        self.plugins.section_open = self.settings_tab == Tab::Plugins;
        self.settings_open &= open;
        if (!self.settings_open || self.settings_tab != Tab::Plugins)
            && self.plugins.dependency_credentials.take().is_some()
        {
            self.credentials.clear();
        }
    }
    fn plugin_log(&mut self, ui: &mut egui::Ui) {
        let previous = self.plugins.log_selected.clone();
        let items = self.plugins.status["items"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        egui::ComboBox::from_id_salt("log-plugin")
            .selected_text(if self.plugins.log_selected.is_empty() {
                tr(self.es, "Choose…")
            } else {
                &self.plugins.log_selected
            })
            .show_ui(ui, |ui| {
                for item in items {
                    if let Some(id) = item["id"].as_str() {
                        ui.selectable_value(&mut self.plugins.log_selected, id.to_owned(), id);
                    }
                }
            });
        if previous != self.plugins.log_selected {
            self.plugins.log = None;
        }
        if self.connected
            && !self.worker.busy()
            && !self.plugins.log_selected.is_empty()
            && ui.button(tr(self.es, "Read recent log")).clicked()
        {
            self.send(crate::Request::PluginLog {
                plugin_id: self.plugins.log_selected.clone(),
            });
        }
        if let Some(log) = &self.plugins.log {
            if log.lines.is_empty() {
                ui.label(tr(self.es, "No recent messages"));
            } else {
                let mut text = log.lines.join("\n");
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .interactive(false)
                        .desired_rows(8)
                        .desired_width(f32::INFINITY),
                );
            }
        }
    }
    fn settings_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tab, en) in [
                (Tab::General, "General"),
                (Tab::Backup, "Backup"),
                (Tab::Plugins, "Plugins"),
                (Tab::Permissions, "Automation"),
            ] {
                ui.selectable_value(&mut self.settings_tab, tab, tr(self.es, en));
            }
        });
    }
    /// A label in the General tab's column, followed by its value.
    fn setting_label(ui: &mut egui::Ui, look: &Look, text: &str) {
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL, ui.spacing().interact_size.y),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(LABEL);
                ui.label(look.muted_text(text));
            },
        );
    }
    fn device(&mut self, ui: &mut egui::Ui, look: &Look) {
        ui.add_enabled_ui(self.connected, |ui| {
            ui.label(look.section(tr(self.es, "Stream Deck")));
            ui.horizontal(|ui| {
                Self::setting_label(ui, look, tr(self.es, "Brightness"));
                let mut brightness = self.settings["brightness_percent"]
                    .as_u64()
                    .unwrap_or(100)
                    .min(100) as u8;
                ui.add(egui::Slider::new(&mut brightness, 0..=100).suffix(tr(self.es, " %")));
                self.settings["brightness_percent"] = json!(brightness);
            });
            for (key, en, unit_en) in [
                ("sleep_ms", "Sleep after", "ms · 0 = never"),
                ("press_threshold_ms", "Hold to press", "ms"),
                ("long_press_threshold_ms", "Long press after", "ms"),
            ] {
                ui.horizontal(|ui| {
                    Self::setting_label(ui, look, tr(self.es, en));
                    let mut value = self.settings[key]
                        .as_str()
                        .unwrap_or(if key == "long_press_threshold_ms" {
                            "500"
                        } else {
                            "0"
                        })
                        .to_owned();
                    ui.add(look.field(&mut value, "").desired_width(110.));
                    ui.label(look.muted_text(tr(self.es, unit_en)));
                    self.settings[key] = json!(value);
                });
            }
        });
        ui.add_space(6.);
        ui.label(look.section(tr(self.es, "Window")));
        ui.horizontal(|ui| {
            Self::setting_label(ui, look, tr(self.es, "Language"));
            use crate::preferences::Language;
            let mut language = self.preferences.language;
            let mut changed = ui
                .selectable_value(&mut language, None, tr(self.es, "System"))
                .changed();
            for available in Language::available() {
                changed |= ui
                    .selectable_value(&mut language, Some(available), available.name())
                    .changed();
            }
            if changed {
                let preferences = crate::preferences::Preferences { language };
                let result = self
                    .preferences_path
                    .as_deref()
                    .ok_or_else(|| std::io::Error::other("preferences path unavailable"))
                    .and_then(|path| preferences.save(path));
                match result {
                    Ok(()) => {
                        self.preferences = preferences;
                        self.es = preferences.language(&crate::preferences::system_locale());
                    }
                    Err(error) => {
                        self.warn(tr(self.es, "Language not saved — try again"));
                        self.message_code = error.to_string();
                    }
                }
            }
        });
        ui.add_space(8.);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            if ui
                .add_enabled(self.connected, look.primary(tr(self.es, "Save settings")))
                .clicked()
            {
                self.send(crate::Request::SetSettings {
                    changes: serde_json::from_value(self.settings.clone()).expect("settings draft"),
                });
                self.settings_open = false;
            }
        });
    }
    fn backup(&mut self, ui: &mut egui::Ui, look: &Look) {
        ui.label(look.muted_text(tr(self.es, "Saves your keys, folders and images to one file. Credentials and plugin programs are not included.")));
        ui.add_space(4.);
        ui.add(
            look.field(&mut self.backup_path, tr(self.es, "/path/to/backup.rad"))
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            for (kind, en) in [
                (TransferKind::Export, "Export to this file"),
                (TransferKind::Restore, "Restore from this file…"),
            ] {
                if ui
                    .add_enabled(
                        !self.transfer_active && !self.backup_path.trim().is_empty(),
                        egui::Button::new(tr(self.es, en)),
                    )
                    .clicked()
                    && self.command(Job::Transfer {
                        path: PathBuf::from(self.backup_path.trim()),
                        kind,
                    })
                {
                    self.transfer_active = true;
                }
            }
        });
        if let Some(token) = self.restore_token.clone() {
            ui.add_space(4.);
            ui.label(look.muted_text(&self.restore_summary));
            ui.horizontal(|ui| {
                if ui
                    .add(look.dangerous(tr(self.es, "Confirm replace all")))
                    .clicked()
                {
                    self.send(crate::Request::RestoreCommit {
                        transfer_id: token,
                        confirm_replace: true,
                    });
                    self.restore_token = None;
                }
                if ui.button(tr(self.es, "Cancel")).clicked() {
                    self.restore_token = None;
                }
            });
        }
    }
}
