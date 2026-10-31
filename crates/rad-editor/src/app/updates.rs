//! Manual update flow; no global notification path.
use super::{App, look::Look};
use eframe::egui;
impl App {
    /// Explicit reopen from the coordinator, never a network check.
    pub fn open_update_result(&mut self) {
        self.open_settings(crate::screens::Section::Updates);
        self.update_reopen_ack = true;
    }
    pub(super) fn update_drafts(&self) -> bool {
        self.unsaved()
            || (!self.status["settings"].is_null() && self.settings != self.status["settings"])
            || (!self.status["global_permissions"].is_null()
                && self.global_permissions
                    != serde_json::json!({"own":self.status["global_permissions"]}))
            || self
                .plugins
                .form
                .as_ref()
                .is_some_and(|(_, loaded)| loaded.form.as_ref().is_some_and(|form| form.dirty))
            || self.credentials.editing()
    }
    pub(super) fn update_pending(&self) -> bool {
        self.worker.busy()
            || self.options_worker.busy()
            || self.credential_worker.busy()
            || self
                .service_worker
                .as_ref()
                .is_some_and(|worker| worker.busy())
            || self.transfer_active
            || self.machine_pending.is_some()
            || self.pending_action.is_some()
            || self.restore_token.is_some()
    }
    fn resolve_update_drafts(&mut self) {
        self.updates.cancel_confirmation();
        if !self.status["settings"].is_null() && self.settings != self.status["settings"] {
            self.open_settings(crate::screens::Section::General);
        } else if self
            .plugins
            .form
            .as_ref()
            .is_some_and(|(target, _)| matches!(target, crate::plugins::FormTarget::Settings(_)))
            || self.plugins.dependency_credentials.is_some()
        {
            self.open_settings(crate::screens::Section::Plugins);
        } else if !self.status["global_permissions"].is_null()
            && self.global_permissions
                != serde_json::json!({"own":self.status["global_permissions"]})
        {
            self.open_settings(crate::screens::Section::Automation);
        } else {
            self.leave_settings();
        }
    }
    pub(super) fn updates_view(&mut self, ui: &mut egui::Ui, look: &Look) {
        use super::tr;
        use crate::updates::{Installation, Verification};
        ui.label(look.section(tr(self.es, "Updates")));
        ui.add_space(8.);
        match self.updates.verification() {
            Verification::Unchecked => {}
            Verification::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr(self.es, "Checking…"));
                });
            }
            Verification::Failed(details) => {
                ui.label(tr(self.es, "Could not check for updates."));
                ui.collapsing(tr(self.es, "View details"), |ui| {
                    ui.label(details);
                });
            }
            Verification::Current => {
                ui.label(tr(self.es, "No updates are available."));
            }
            Verification::Available { managed, .. } => {
                ui.label(tr(self.es, "An update is available."));
                if !managed {
                    ui.label(look.muted_text(tr(
                        self.es,
                        "Updates can be installed here only from the official system package.",
                    )));
                }
            }
        }
        ui.add_space(8.);
        match self.updates.installation().clone() {
            Installation::Idle | Installation::Cancelled | Installation::Failed(_) => {
                if !self.updates.busy() {
                    if ui.button(tr(self.es, "Check for updates")).clicked() {
                        self.updates.check();
                    }
                    if matches!(
                        self.updates.verification(),
                        Verification::Available { managed: true, .. }
                    ) && ui.add(look.primary(tr(self.es, "Update RAD"))).clicked()
                    {
                        self.updates.confirm();
                    }
                }
            }
            Installation::Confirming => {
                ui.label(tr(self.es,"RAD will close briefly to install the update. RAD will temporarily stop controlling the Stream Deck."));
                ui.add_space(8.);
                if self.update_drafts() {
                    ui.label(tr(
                        self.es,
                        "Save or discard your pending changes before updating.",
                    ));
                    if ui.button(tr(self.es, "Resolve pending changes")).clicked() {
                        self.resolve_update_drafts();
                    }
                } else if self.update_pending() {
                    ui.label(tr(
                        self.es,
                        "Wait for the current operation to finish before updating.",
                    ));
                } else if ui.add(look.primary(tr(self.es, "Continue"))).clicked() {
                    self.updates.prepare();
                }
                if ui.button(tr(self.es, "Cancel")).clicked() {
                    self.updates.cancel_confirmation();
                }
            }
            Installation::Preparing | Installation::Ready => {
                ui.label(tr(self.es, "Preparing update…"));
                let progress = self.updates.percent().unwrap_or(0) as f32 / 100.;
                ui.add(
                    egui::ProgressBar::new(progress)
                        .show_percentage()
                        .animate(self.updates.percent().is_none()),
                );
                if ui.button(tr(self.es, "Cancel")).clicked() {
                    self.updates.cancel();
                }
            }
            Installation::HandingOff => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr(self.es, "Authorizing update…"));
                });
            }
            Installation::Relayed => {
                ui.label(tr(self.es, "Closing RAD for the update…"));
            }
        }
        match self.updates.installation() {
            Installation::Failed(details) => {
                ui.label(tr(self.es, "Could not install the update."));
                ui.collapsing(tr(self.es, "View details"), |ui| {
                    ui.label(details);
                });
            }
            Installation::Cancelled => {
                ui.label(tr(self.es, "Update cancelled."));
            }
            _ => {}
        }
        if self.updates.cancelling() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr(self.es, "Cancelling…"));
            });
        }
        if let Some(outcome) = self.updates.outcome() {
            match outcome {
                Ok(Some(result)) => {
                    ui.label(tr(
                        self.es,
                        if result.success {
                            "RAD was updated."
                        } else {
                            "Could not install the update."
                        },
                    ));
                    ui.collapsing(tr(self.es, "View details"), |ui| {
                        ui.label(&result.details);
                    });
                }
                Err(details) => {
                    ui.label(tr(self.es, "Could not install the update."));
                    ui.collapsing(tr(self.es, "View details"), |ui| {
                        ui.label(details);
                    });
                }
                Ok(None) => {}
            }
        }
        ui.add_space(12.);
        if let Ok(release) = rad_update::current_release() {
            ui.label(format!(
                "{} {}-{}",
                tr(self.es, "RAD version"),
                release.version,
                release.pkgrel
            ));
        }
        if !self.update_outcome_asked && !self.updates.busy() && self.updates.read_outcome() {
            self.update_outcome_asked = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_settings_drafts_are_not_discarded_by_back_from_updates() {
        let root = std::env::temp_dir().join(format!("rad-update-draft-{}", std::process::id()));
        let mut app = App::with_preferences_path(root.clone(), root.join("theme"), None);
        app.status = serde_json::json!({"settings":{"brightness_percent":10}});
        app.open_settings(crate::screens::Section::General);
        app.settings["brightness_percent"] = serde_json::json!(20);
        app.open_settings(crate::screens::Section::Updates);
        app.leave_settings();
        assert_eq!(
            app.navigation.screen(),
            crate::screens::Screen::Settings(crate::screens::Section::Updates),
            "Back must ask before losing a disconnected draft"
        );
        assert_eq!(app.settings["brightness_percent"], 20);
        assert!(app.navigation.asking(app.settings_unapplied()));
    }
}
