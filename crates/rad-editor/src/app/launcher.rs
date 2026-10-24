//! Application choices stay in the editor until a real desktop entry is selected.
//! Both lists use the main worker, with one tagged read so their searches cannot mix.
use super::{App, Entry, Job, NewAction, Reply, SEARCH_DEBOUNCE_MS, look::Look, tr};
use crate::{catalog::Catalog, session::Error};
use eframe::egui;
use std::collections::BTreeMap;

#[derive(Clone)]
enum Placement {
    Button(String),
    EmptySlot { folder: String, position: usize },
}

#[derive(Default)]
pub(super) struct ApplicationPicker {
    catalog: Catalog,
    due: Option<u64>,
    more: bool,
    loaded: bool,
    failed: bool,
    placement: Option<Placement>,
    /// Names survive page replacement and clearing either search's visible results.
    names: BTreeMap<String, String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CatalogTarget {
    Actions,
    Picker,
}
pub(super) struct CatalogRead {
    target: CatalogTarget,
    query: String,
    append: bool,
}

impl ApplicationPicker {
    pub(super) fn is_due(&self, now: u64) -> bool {
        self.due.is_some_and(|due| now >= due)
    }
    pub(super) fn cancel(&mut self) {
        self.placement = None;
    }
    pub(super) fn adds_to(&self, id: &str) -> bool {
        matches!(&self.placement, Some(Placement::Button(button)) if button == id)
    }
}

impl App {
    pub(super) fn begin_application_choice(&mut self, position: usize) {
        if self.plugins.form_pending || self.plugins.form_loading {
            self.warn(tr(self.es, "Busy — not sent"));
            return;
        }
        if self.key_form_open() {
            self.leave_key_form();
            if self.key_form_open() {
                return; // The existing form asks for Discard before it may be closed.
            }
        }
        let Some(folder) = self.folder() else { return };
        let button = self.slot_id(position);
        if self.selected_slot != Some(position) || self.selected() != button {
            self.select(position, button.clone());
        }
        self.app_picker.placement = Some(match button {
            Some(button) => Placement::Button(button),
            None => Placement::EmptySlot { folder, position },
        });
    }

    pub(super) fn add_application(&mut self, button: &str, entry: &Entry) -> bool {
        let sent = self.command(Job::Request(crate::commands::add_action(
            button,
            "launcher",
            serde_json::json!({"desktop_entry": entry.desktop_entry}),
        )));
        if sent {
            self.icon_candidate = Some((button.into(), entry.desktop_entry.clone()));
        }
        sent
    }

    pub(super) fn application_catalog_loading(&self, target: CatalogTarget) -> bool {
        let due = match target {
            CatalogTarget::Actions => self.catalog_due,
            CatalogTarget::Picker => self.app_picker.due,
        };
        due.is_some()
            || self
                .catalog_read
                .as_ref()
                .is_some_and(|read| read.target == target)
    }

    pub(super) fn request_application_catalog(&mut self, target: CatalogTarget) {
        let (catalog, more) = match target {
            CatalogTarget::Actions => {
                self.catalog_due = None;
                (&self.catalog, &mut self.catalog_more)
            }
            CatalogTarget::Picker => {
                self.app_picker.due = None;
                (&self.app_picker.catalog, &mut self.app_picker.more)
            }
        };
        let query = catalog.query.clone();
        let append = std::mem::take(more);
        match self.worker.submit(Job::Request(catalog.request(append))) {
            Ok(()) => {
                self.catalog_read = Some(CatalogRead {
                    target,
                    query,
                    append,
                })
            }
            Err(_) => {
                if target == CatalogTarget::Picker {
                    self.app_picker.failed = true;
                }
                self.warn(tr(self.es, "Could not load applications"));
            }
        }
    }

    /// Consume exactly the read that just finished, including errors. A failed picker
    /// stays failed until Retry or a new query; stale replies cannot change its state.
    pub(super) fn finish_application_catalog(&mut self, reply: &Result<Reply, Error>) {
        let Some(read) = self.catalog_read.take() else {
            return;
        };
        let catalog = match read.target {
            CatalogTarget::Actions => &mut self.catalog,
            CatalogTarget::Picker => &mut self.app_picker.catalog,
        };
        if read.query != catalog.query {
            return;
        }
        let mut page = Catalog::default();
        let result = match reply {
            Ok(Reply::Response {
                operation,
                response,
            }) if operation == "launcher_catalog" => page.accept(&response.data),
            Err(error) => Err(error.clone()),
            _ => Err(Error::protocol()),
        };
        if let Err(error) = result {
            if read.target == CatalogTarget::Picker {
                self.app_picker.failed = true;
            }
            // Transport errors are already explained by the main pump.
            if reply.is_ok() {
                self.warn(crate::status::describe_failure(&error, self.es));
            }
            return;
        }
        self.app_picker.names.extend(
            page.entries
                .iter()
                .map(|entry| (entry.desktop_entry.clone(), entry.name.clone())),
        );
        if read.target == CatalogTarget::Picker && read.append {
            for entry in page.entries {
                if let Some(existing) = catalog
                    .entries
                    .iter_mut()
                    .find(|existing| existing.desktop_entry == entry.desktop_entry)
                {
                    *existing = entry;
                } else {
                    catalog.entries.push(entry);
                }
            }
        } else {
            catalog.entries = page.entries;
        }
        catalog.next = page.next;
        if read.target == CatalogTarget::Picker {
            self.app_picker.loaded = true;
            self.app_picker.failed = false;
        }
    }

    /// The provisional row does not create a key or action. Both happen only after a
    /// choice, through the same launcher path used by direct application drops.
    pub(super) fn pending_application_row(&mut self, ui: &mut egui::Ui, look: &Look) -> bool {
        let Some(placement) = self.app_picker.placement.clone() else {
            return false;
        };
        look.card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr(self.es, "Open App"));
                if let Some(entry) = self.application_choice(ui, look, "new-application", "") {
                    let sent = match &placement {
                        Placement::Button(button) => self.add_application(button, &entry),
                        Placement::EmptySlot { folder, position } => {
                            if self.folder().as_deref() != Some(folder.as_str())
                                || self.slot_id(*position).is_some()
                            {
                                self.app_picker.cancel();
                                self.warn(tr(
                                    self.es,
                                    "State refreshed — check your change before trying again",
                                ));
                                false
                            } else {
                                self.create_action_button(
                                    folder.clone(),
                                    *position,
                                    &NewAction::Launcher(entry),
                                )
                            }
                        }
                    };
                    if sent {
                        self.app_picker.cancel();
                    }
                }
                if ui.button(tr(self.es, "Cancel")).clicked() {
                    self.app_picker.cancel();
                }
            });
        });
        true
    }

    pub(super) fn application_choice(
        &mut self,
        ui: &mut egui::Ui,
        look: &Look,
        id: &str,
        current: &str,
    ) -> Option<Entry> {
        let shown = if current.is_empty() {
            tr(self.es, "Choose application").to_owned()
        } else {
            self.app_picker
                .names
                .get(current)
                .cloned()
                .unwrap_or_else(|| current.trim_end_matches(".desktop").to_owned())
        };
        let mut chosen = None;
        egui::ComboBox::from_id_salt(("application", id))
            .selected_text(shown)
            .width(220.)
            .height(240.)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show_ui(ui, |ui| {
                // Keep async results from being constrained to the initial loading size.
                ui.set_min_height(200.);
                let search = ui.add(
                    look.field(
                        &mut self.app_picker.catalog.query,
                        tr(self.es, "Search applications"),
                    )
                    .desired_width(240.),
                );
                if search.changed() {
                    self.app_picker.catalog.entries.clear();
                    self.app_picker.catalog.next = None;
                    self.app_picker.more = false;
                    self.app_picker.loaded = false;
                    self.app_picker.failed = false;
                    self.app_picker.due = Some(self.now_ms() + SEARCH_DEBOUNCE_MS);
                    if let Some(wake) = &self.search_wake {
                        wake.schedule(std::time::Duration::from_millis(SEARCH_DEBOUNCE_MS));
                    }
                }
                if !self.app_picker.loaded
                    && !self.app_picker.failed
                    && !self.application_catalog_loading(CatalogTarget::Picker)
                {
                    self.app_picker.due = Some(0);
                }
                let loading = self.application_catalog_loading(CatalogTarget::Picker);
                if self.app_picker.failed {
                    ui.label(tr(self.es, "Could not load applications"));
                    if ui.button(tr(self.es, "Retry")).clicked() {
                        self.app_picker.failed = false;
                        self.app_picker.due = Some(0);
                    }
                } else if loading {
                    ui.spinner();
                } else if self.app_picker.catalog.entries.is_empty() {
                    ui.label(look.muted_text(tr(self.es, "No applications found")));
                }
                // ComboBox already owns the menu scroll.
                for entry in &self.app_picker.catalog.entries {
                    if ui
                        .selectable_label(entry.desktop_entry == current, &entry.name)
                        .clicked()
                    {
                        chosen = Some(entry.clone());
                        ui.close();
                    }
                }
                if self.app_picker.catalog.next.is_some()
                    && !loading
                    && ui.button(tr(self.es, "More…")).clicked()
                {
                    self.app_picker.more = true;
                    self.app_picker.due = Some(0);
                }
            });
        chosen
    }
}
