//! Settings screen: device, language, backup, plugins and global automation permissions,
//! one section at a time, filling the window until Back or Esc (E4).
use super::{
    App,
    inspector::permission_ui,
    look::{Icon, Look, icon_button},
    tr,
};
use crate::{
    native::{Job, TransferKind},
    plugins::FormTarget,
    screens::{Screen, Section},
};
use eframe::egui;
use serde_json::json;
use std::path::PathBuf;

/// Widest the settings grow: wider, the eye loses the line between a label and its value.
const WIDTH: f32 = 640.;
/// Width of the labels in the General section, so the values line up.
const LABEL: f32 = 170.;

impl App {
    /// Shows a section of the settings; coming from the keys, the drafts start from what the
    /// runtime last published.
    pub(super) fn open_settings(&mut self, section: Section) {
        if self.navigation.screen() == Screen::Settings(Section::Updates)
            && section != Section::Updates
            && !self.updates.leave()
        {
            return;
        }
        if self.navigation.screen() == Screen::Keys {
            self.settings = self.status["settings"].clone();
            self.global_permissions = json!({"own":self.status["global_permissions"]});
        }
        if section == Section::Updates
            && self.navigation.screen() != Screen::Settings(Section::Updates)
        {
            self.updates.enter();
            self.update_outcome_asked = false;
        }
        self.navigation.open(section);
    }
    fn general_unapplied(&self) -> bool {
        !self.status["settings"].is_null() && self.settings != self.status["settings"]
    }
    /// What leaving the settings would lose: the General draft, a plugin's settings or a
    /// password being typed.
    pub(super) fn settings_unapplied(&self) -> bool {
        let form = self
            .plugins
            .form
            .as_ref()
            .filter(|(target, _)| matches!(target, FormTarget::Settings(_)));
        self.general_unapplied()
            || form.is_some_and(|(_, loaded)| loaded.form.as_ref().is_some_and(|f| f.dirty))
            || ((form.is_some() || self.plugins.dependency_credentials.is_some())
                && self.credentials.editing())
    }
    /// Back or Esc: to the keys, unless something unapplied makes the screen ask first.
    pub(super) fn leave_settings(&mut self) {
        if self.navigation.screen() == Screen::Settings(Section::Updates) {
            if self.updates.relaying() {
                return;
            }
            if self.updates.installation() == &crate::updates::Installation::Confirming {
                self.updates.cancel_confirmation();
                return;
            }
        }
        let unapplied = self.settings_unapplied();
        if self.navigation.back(unapplied) {
            self.settings_closed();
        }
    }
    fn discard_settings(&mut self) {
        self.navigation.discard();
        self.settings_closed();
    }
    /// Nothing of the settings outlives their screen.
    fn settings_closed(&mut self) {
        self.updates.leave();
        self.update_outcome_asked = false;
        self.settings = self.status["settings"].clone();
        if matches!(self.plugins.form, Some((FormTarget::Settings(_), _))) {
            self.plugins.clear_form();
            self.credentials.clear();
        }
        if self.plugins.dependency_credentials.take().is_some() {
            self.credentials.clear();
        }
        self.plugins.section_open = false;
        self.plugins.settings_asked = None;
    }
    pub(super) fn settings_screen(&mut self, ui: &mut egui::Ui, look: &Look) {
        let Screen::Settings(section) = self.navigation.screen() else {
            return;
        };
        egui::Panel::top("settings-bar")
            .frame(look.panel(look.surface, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if icon_button(ui, look, Icon::Back, tr(self.es, "Back (Esc)")).clicked() {
                        self.leave_settings();
                    }
                    ui.label(egui::RichText::new(tr(self.es, "Settings")).heading());
                    ui.add_space(18.);
                    for (choice, en) in [
                        (Section::General, "General"),
                        (Section::Backup, "Backup"),
                        (Section::Plugins, "Plugins"),
                        (Section::Automation, "Automation"),
                        (Section::Updates, "Updates"),
                    ] {
                        if ui
                            .selectable_label(section == choice, tr(self.es, en))
                            .clicked()
                        {
                            self.open_settings(choice);
                        }
                    }
                });
                if self.navigation.asking(self.settings_unapplied()) {
                    ui.horizontal(|ui| {
                        ui.colored_label(look.warn, tr(self.es, "Some changes are not applied"));
                        if ui.add(look.dangerous(tr(self.es, "Discard"))).clicked() {
                            self.discard_settings();
                        }
                    });
                }
            });
        egui::Panel::bottom("status")
            .frame(
                look.panel(look.surface, 4)
                    .inner_margin(egui::Margin::symmetric(14, 5)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.status_line(ui, look));
        egui::CentralPanel::default()
            .frame(look.panel(look.background, 18))
            .show(ui, |ui| {
                if section == Section::Plugins {
                    // Two columns that scroll on their own, across the whole window.
                    ui.add_enabled_ui(self.connected, |ui| self.plugin_settings(ui, look));
                    return;
                }
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.set_max_width(WIDTH.min(ui.available_width()));
                        self.settings_section(ui, look, section);
                    });
            });
        // Plugin status is asked for only while its section is in view.
        self.plugins.section_open = section == Section::Plugins;
        if !matches!(section, Section::Plugins | Section::Updates)
            && self.plugins.dependency_credentials.take().is_some()
        {
            self.credentials.clear();
        }
    }
    fn settings_section(&mut self, ui: &mut egui::Ui, look: &Look, section: Section) {
        if section == Section::Updates {
            return self.updates_view(ui, look);
        }
        ui.add_enabled_ui(
            self.connected || section == Section::General,
            |ui| match section {
                Section::General => self.device(ui, look),
                Section::Backup => self.backup(ui, look),
                Section::Plugins => {}
                Section::Updates => self.updates_view(ui, look),
                Section::Automation => {
                    ui.label(look.muted_text(tr(
                        self.es,
                        "What rad-cli and scripts may do, unless a key or action says otherwise.",
                    )));
                    ui.add_space(4.);
                    if permission_ui(ui, &mut self.global_permissions, self.es, false) {
                        self.send(crate::Request::SetPermissions {
                            target: "global".into(),
                            value: serde_json::from_value(self.global_permissions.clone())
                                .expect("permissions draft"),
                        });
                    }
                }
            },
        );
        ui.add_space(12.);
        ui.collapsing(tr(self.es, "Advanced"), |ui| {
            ui.collapsing(tr(self.es, "Log"), |ui| self.plugin_log(ui));
            ui.collapsing(tr(self.es, "RAD log"), |ui| self.runtime_log_ui(ui));
        });
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
    /// RAD's own recent events, read on request: technical, so they live in Advanced.
    fn runtime_log_ui(&mut self, ui: &mut egui::Ui) {
        if self.connected
            && !self.worker.busy()
            && ui.button(tr(self.es, "Read recent log")).clicked()
        {
            self.send(crate::Request::RuntimeLog {});
        }
        let Some(log) = &self.runtime_log else {
            return;
        };
        if log.entries.is_empty() {
            ui.label(tr(self.es, "No recent messages"));
        } else {
            let mut text = log
                .entries
                .iter()
                .map(crate::diagnostics::log_line)
                .collect::<Vec<_>>()
                .join("\n");
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .interactive(false)
                    .desired_rows(8)
                    .desired_width(f32::INFINITY),
            );
        }
        if log.dropped != "0" {
            ui.label(format!(
                "{} {}",
                log.dropped,
                tr(self.es, "older messages were discarded")
            ));
        }
    }
    /// A label in the General section's column, followed by its value.
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
                    .clamp(10, 100) as u8;
                ui.add(egui::Slider::new(&mut brightness, 10..=100).suffix(tr(self.es, " %")));
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
        self.machine_section(ui, look);
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
        // Shown only when there is something to save, as on a key.
        if self.general_unapplied() {
            ui.add_space(8.);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if ui.add(look.primary(tr(self.es, "Save settings"))).clicked() {
                    self.send(crate::Request::SetSettings {
                        changes: serde_json::from_value(self.settings.clone())
                            .expect("settings draft"),
                    });
                }
            });
        }
    }
    fn export_backup(&mut self) {
        if !self.connected
            || !self.recovery.is_normal()
            || self.transfer_active
            || self.updates.busy()
        {
            return;
        }
        if self.update_drafts() {
            self.warn(tr(
                self.es,
                "Save pending changes before exporting a backup.",
            ));
            return;
        }
        if self.command(Job::ExportBackup) {
            self.transfer_active = true;
        }
    }
    fn backup(&mut self, ui: &mut egui::Ui, look: &Look) {
        ui.label(look.muted_text(tr(self.es, "Saves your keys, folders and images to one file. Credentials and plugin programs are not included.")));
        ui.add_space(4.);
        if self.connected
            && self.recovery.is_normal()
            && !self.transfer_active
            && !self.updates.busy()
        {
            if self.update_drafts() {
                ui.label(tr(
                    self.es,
                    "Save pending changes before exporting a backup.",
                ));
            } else if ui.add(look.primary(tr(self.es, "Export Backup"))).clicked() {
                self.export_backup();
            }
        }
        ui.label(look.muted_text(tr(
            self.es,
            "One-click backups are saved in RAD Backups in your home folder.",
        )));
        ui.add_space(8.);
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
        self.restore_confirmation(ui, look);
    }
    /// Once a copy is uploaded: what it holds, and the choice to replace everything with it.
    pub(super) fn restore_confirmation(&mut self, ui: &mut egui::Ui, look: &Look) {
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

#[cfg(test)]
mod backup_tests {
    use super::*;

    // Rendering Updates must never read or maintain the real session's updater state.
    struct InMemoryUpdates;
    impl crate::updates::Backend for InMemoryUpdates {
        fn check(&self, _: &rad_update::Cancellation) -> Result<rad_update::CheckResult, String> {
            panic!("checking was not requested")
        }
        fn prepare(
            &self,
            _: &rad_update::Candidate,
            _: &rad_update::Cancellation,
            _: &rad_update::Progress,
        ) -> Result<rad_update::Prepared, String> {
            panic!("preparation was not requested")
        }
        fn handoff(&self, _: &rad_update::Prepared) -> Result<(), String> {
            panic!("handoff was not requested")
        }
        fn discard(&self, _: &rad_update::Prepared) {
            panic!("no prepared update exists")
        }
    }
    fn app() -> App {
        let root = std::env::temp_dir().join(format!("rad-backup-ui-{}", std::process::id()));
        let mut app = App::with_preferences_path(root.clone(), root.join("theme"), None);
        app.connected = true;
        app.es = crate::preferences::Language::En;
        app.updates = crate::updates::Controller::new(std::sync::Arc::new(InMemoryUpdates));
        app
    }
    fn texts(app: &mut App, section: Section) -> Vec<String> {
        app.open_settings(section);
        let ctx = egui::Context::default();
        ctx.run_ui(Default::default(), |ui| {
            app.settings_screen(ui, &Look::new(&Default::default()))
        })
        .shapes
        .iter()
        .filter_map(|shape| {
            if let egui::epaint::Shape::Text(text) = &shape.shape {
                Some(text.galley.job.text.clone())
            } else {
                None
            }
        })
        .collect()
    }
    #[test]
    fn one_click_export_is_explicit_and_blocks_duplicates_and_unsaved_drafts() {
        let mut app = app();
        let (sent, received) = std::sync::mpsc::channel();
        app.worker = crate::worker::Worker::spawn(move |job| {
            sent.send(format!("{job:?}")).unwrap();
            crate::native::Event {
                plugin_apply: false,
                plugin_admin: false,
                plugin_manifest: false,
                plugin_dependencies: false,
                plugin_form: false,
                plugin_visual: false,
                connected: true,
                transfer_job: true,
                result: Ok(crate::native::Reply::TransferProgress(0)),
                palette: Default::default(),
            }
        });
        let _ = texts(&mut app, Section::Backup);
        assert!(
            received.try_recv().is_err(),
            "entry must not start a backup"
        );
        app.export_backup();
        assert!(
            app.transfer_active,
            "the explicit click must reserve its transfer"
        );
        assert_eq!(
            received
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            "ExportBackup"
        );
        app.export_backup();
        assert!(
            received.try_recv().is_err(),
            "a second click must not export twice"
        );
        app.transfer_active = false;
        app.status = serde_json::json!({"settings":{"brightness_percent":10}});
        app.settings = serde_json::json!({"brightness_percent":20});
        app.export_backup();
        assert!(
            !app.transfer_active,
            "unpublished drafts must be resolved first"
        );
        assert!(received.try_recv().is_err());
    }
}
