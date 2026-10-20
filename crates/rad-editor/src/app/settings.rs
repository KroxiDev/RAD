//! Settings window: device, language, backup, plugins and global automation permissions,
//! one tab at a time.
use super::{App, inspector::permission_ui, look::Look, string, tr};
use crate::{
    Request,
    native::{Job, TransferKind},
};
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
        egui::Window::new(tr(self.es, "Settings", "Ajustes"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .fixed_size([WIDTH, 0.])
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                self.settings_tabs(ui);
                ui.separator();
                ui.add_space(4.);
                ui.add_enabled_ui(self.connected, |ui| match self.settings_tab {
                    Tab::General => self.device(ui, look),
                    Tab::Backup => self.backup(ui, look),
                    Tab::Plugins => self.plugin_settings(ui),
                    Tab::Permissions => {
                        ui.label(look.muted_text(tr(
                            self.es,
                            "What rad-cli and scripts may do, unless a folder or key says otherwise.",
                            "Qué pueden hacer rad-cli y los scripts, salvo que una carpeta o tecla diga otra cosa.",
                        )));
                        ui.add_space(4.);
                        if permission_ui(ui, &mut self.global_permissions, self.es, false) {
                            self.send(Request::new(
                                "set_permissions",
                                json!({"target":"global","value":self.global_permissions}),
                            ));
                        }
                    }
                });
            });
        // Plugin status is asked for only while its tab is in view.
        self.plugins.section_open = self.settings_tab == Tab::Plugins;
        self.settings_open &= open;
    }
    fn settings_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tab, en, es) in [
                (Tab::General, "General", "General"),
                (Tab::Backup, "Backup", "Respaldo"),
                (Tab::Plugins, "Plugins", "Plugins"),
                (Tab::Permissions, "Automation", "Automatización"),
            ] {
                ui.selectable_value(&mut self.settings_tab, tab, tr(self.es, en, es));
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
        ui.label(look.section(tr(self.es, "Stream Deck", "Stream Deck")));
        ui.horizontal(|ui| {
            Self::setting_label(ui, look, tr(self.es, "Brightness", "Brillo"));
            let mut brightness = self.settings["brightness_percent"]
                .as_u64()
                .unwrap_or(100)
                .min(100) as u8;
            ui.add(egui::Slider::new(&mut brightness, 0..=100).suffix(" %"));
            self.settings["brightness_percent"] = json!(brightness);
        });
        for (key, en, es, unit_en, unit_es) in [
            (
                "sleep_ms",
                "Sleep after",
                "Reposo tras",
                "ms · 0 = never",
                "ms · 0 = nunca",
            ),
            (
                "press_threshold_ms",
                "Hold to press",
                "Mantener para pulsar",
                "ms",
                "ms",
            ),
        ] {
            ui.horizontal(|ui| {
                Self::setting_label(ui, look, tr(self.es, en, es));
                let mut value = string(&self.settings[key]);
                ui.add(look.field(&mut value, "").desired_width(110.));
                ui.label(look.muted_text(tr(self.es, unit_en, unit_es)));
                self.settings[key] = json!(value);
            });
        }
        ui.add_space(6.);
        ui.label(look.section(tr(self.es, "Window", "Ventana")));
        ui.horizontal(|ui| {
            Self::setting_label(ui, look, tr(self.es, "Language", "Idioma"));
            let mut language = string(&self.settings["gui_language"]);
            ui.selectable_value(&mut language, "en".into(), "English");
            ui.selectable_value(&mut language, "es".into(), "Español");
            self.settings["gui_language"] = json!(language);
        });
        ui.add_space(8.);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            if ui
                .add(look.primary(tr(self.es, "Save settings", "Guardar ajustes")))
                .clicked()
            {
                self.send(Request::new(
                    "set_settings",
                    json!({"changes":self.settings}),
                ));
                self.settings_open = false;
            }
        });
    }
    fn backup(&mut self, ui: &mut egui::Ui, look: &Look) {
        ui.label(look.muted_text(tr(
            self.es,
            "Saves your keys, folders and images to one file. Credentials and plugin programs are not included.",
            "Guarda tus teclas, carpetas e imágenes en un archivo. No incluye credenciales ni los programas de los plugins.",
        )));
        ui.add_space(4.);
        ui.add(
            look.field(
                &mut self.backup_path,
                tr(self.es, "/path/to/backup.rad", "/ruta/al/respaldo.rad"),
            )
            .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            for (kind, en, es) in [
                (
                    TransferKind::Export,
                    "Export to this file",
                    "Exportar a este archivo",
                ),
                (
                    TransferKind::Restore,
                    "Restore from this file…",
                    "Restaurar desde este archivo…",
                ),
            ] {
                if ui
                    .add_enabled(
                        !self.transfer_active && !self.backup_path.trim().is_empty(),
                        egui::Button::new(tr(self.es, en, es)),
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
                    .add(look.dangerous(tr(
                        self.es,
                        "Confirm replace all",
                        "Confirmar sustitución completa",
                    )))
                    .clicked()
                {
                    self.send(Request::new(
                        "restore_commit",
                        json!({"transfer_id":token,"confirm_replace":true}),
                    ));
                    self.restore_token = None;
                }
                if ui.button(tr(self.es, "Cancel", "Cancelar")).clicked() {
                    self.restore_token = None;
                }
            });
        }
    }
}
