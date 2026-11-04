//! Bottom panel: what the selected key is and does. Only Try runs anything.
use super::{
    App, Draft, Drag,
    deck::{native_icon, paint_key},
    look::{Icon, Look, glyph_button, icon_button, menu_button, paint_icon},
    string, tr,
};
use crate::{
    commands,
    native::{Job, TransferKind},
};
use eframe::egui;
use serde_json::{Value, json};
use std::path::PathBuf;

const FIELD: f32 = 240.;
const PREVIEW: f32 = 80.;
/// Widest an action row grows: wider, the eye loses the line between name and controls.
const ACTION_WIDTH: f32 = 560.;
/// Longest wait the runtime accepts; checked here to explain it instead of a bare rejection.
const MAX_WAIT_MS: u64 = 600_000;
/// Seconds the panel of a newly selected key takes to fade in once it is read.
const REVEAL_SECONDS: f32 = 0.12;

impl App {
    pub(super) fn inspector(&mut self, ui: &mut egui::Ui, look: &Look) {
        if self.key_form_open() {
            return self.key_form(ui, look);
        }
        match (self.selected(), self.selected_slot) {
            (Some(id), _) => self.key_editor(ui, look, &id),
            (None, Some(position)) => self.empty_key(ui, look, position),
            (None, None) => {
                ui.vertical_centered(|ui| {
                    ui.add_space(28.);
                    ui.label(look.muted_text(tr(
                        self.es,
                        "Select a key to edit it, or drag an action onto one",
                    )));
                });
            }
        }
    }
    fn empty_key(&mut self, ui: &mut egui::Ui, look: &Look, position: usize) {
        if self.pending_application_row(ui, look) {
            return;
        }
        ui.label(
            egui::RichText::new(format!("{} {}", tr(self.es, "Empty key"), position + 1)).heading(),
        );
        ui.label(look.muted_text(tr(
            self.es,
            "Drag an action here from the list on the right, or create it yourself:",
        )));
        ui.add_space(2.);
        ui.horizontal(|ui| {
            ui.add(
                look.field(&mut self.new_name, tr(self.es, "Name (optional)"))
                    .desired_width(FIELD),
            );
            if ui.add(look.primary(tr(self.es, "Create button"))).clicked() {
                self.create(position, false);
            }
            if ui.button(tr(self.es, "Create folder")).clicked() {
                self.create(position, true);
            }
        });
    }
    /// The draft differs from what the runtime last published for this key.
    pub(super) fn unsaved(&self) -> bool {
        self.icon_library.dirty_for(&self.draft.id)
            || self.inspection.as_ref().is_some_and(|published| {
                let mut published = Draft::load(published);
                if let Some(state) = &self.draft.state {
                    published.select_state(state.clone());
                }
                let draft = &self.draft;
                published.states != draft.state_faces()
                    || published.name != draft.name
                    || published.text != draft.text
                    || published.image != draft.image
                    || published.style != draft.style
                    || published.mode != draft.mode
                    || published.gesture != draft.gesture
                    || published.order != draft.order
                    || published.waits != draft.waits
                    || published.entries != draft.entries
                    || published.brightness_directions != draft.brightness_directions
            })
    }
    pub(super) fn save(&mut self) {
        if self.icon_library.dirty_for(&self.draft.id)
            && self
                .inspection
                .as_ref()
                .is_none_or(|view| view.button["id"].as_str() != Some(self.draft.id.as_str()))
        {
            return; // A revision invalidated the inspection; wait before capturing action edits.
        }
        let actions = self.inspection.iter().flat_map(|i| &i.actions);
        let waits: Option<Vec<_>> = actions
            .zip(&self.draft.waits)
            .filter(|(action, _)| action["fields"]["type"] == "wait")
            .map(|(action, typed)| {
                let milliseconds = typed.trim().parse::<u64>().ok()?;
                (milliseconds <= MAX_WAIT_MS).then(|| (string(&action["id"]), milliseconds))
            })
            .collect();
        let Some(waits) = waits else {
            return self.warn(tr(
                self.es,
                "A wait is a number of milliseconds, up to 600000 (10 minutes)",
            ));
        };
        let published_style = self
            .inspection
            .as_ref()
            .map(|i| super::text_style::from_view(&i.appearance))
            .unwrap_or_default();
        let base_style = if self.draft.state.is_some() {
            super::text_style::from_face(&self.draft.base_face)
        } else {
            self.draft.style
        };
        let text_position = (base_style.position != published_style.position)
            .then_some(super::text_style::position_name(base_style.position));
        let launchers: Vec<_> = self
            .inspection
            .iter()
            .flat_map(|i| &i.actions)
            .zip(&self.draft.entries)
            .filter(|(action, _)| action["fields"]["type"] == "launcher")
            .map(|(action, entry)| (string(&action["id"]), entry.clone()))
            .collect();
        let brightness: Vec<_> = self
            .inspection
            .iter()
            .flat_map(|i| &i.actions)
            .zip(&self.draft.brightness_directions)
            .filter(|(action, _)| action["fields"]["type"] == "brightness")
            .map(|(action, direction)| (string(&action["id"]), *direction))
            .collect();
        let mut request = commands::save_button(&commands::KeySave {
            id: &self.draft.id,
            name: &self.draft.name,
            text: &self.draft.text,
            image: (!self.draft.image.is_empty()).then_some(self.draft.image.as_str()),
            text_position,
            mode: &self.draft.mode,
            gesture: &self.draft.gesture,
            order: &self.draft.order,
            waits: &waits,
            launchers: &launchers,
            brightness: &brightness,
        });
        if let crate::Request::Edit {
            changes,
            expected_revision,
        } = &mut request
        {
            if self.draft.state.is_some() || self.icon_library.dirty_for(&self.draft.id) {
                let Some(revision) = self
                    .inspection
                    .as_ref()
                    .and_then(|view| view.button["view_revision"].as_str())
                else {
                    return;
                };
                *expected_revision = Some(revision.to_owned());
            }
            for change in changes {
                if let rad_api::Change::Set {
                    edit: rad_api::FieldEdit::Appearance(a),
                    ..
                } = change
                {
                    a.text_size =
                        (base_style.size != published_style.size).then_some(Some(base_style.size));
                    a.text_color = (base_style.color != published_style.color)
                        .then(|| Some(super::text_style::color_string(base_style.color)));
                    a.text_band =
                        (base_style.band != published_style.band).then_some(Some(base_style.band));
                    a.states = Some(self.draft.state_faces());
                    if self.draft.state.is_some() {
                        a.text = self.draft.base_face.text.clone();
                        a.image = self.draft.base_face.image.clone();
                    }
                }
            }
        }
        if self.icon_library.dirty_for(&self.draft.id) {
            if let Some(mut job) = self.icon_library.save_job(request) {
                if let Job::SaveIcon { state, .. } = &mut job {
                    *state = self.draft.state.clone();
                }
                if self.command(job) {
                    self.icon_library.saving();
                }
            }
        } else {
            self.send(request);
        }
    }
    fn key_editor(&mut self, ui: &mut egui::Ui, look: &Look, id: &str) {
        self.prepare_state_tab(ui);
        let actions = self
            .inspection
            .as_ref()
            .map(|i| i.actions.clone())
            .unwrap_or_default();
        // Fades in once read; a key read ahead shows at once, and edits replace it in place.
        let reveal = ui.ctx().animate_bool_with_time(
            egui::Id::new(("key-reveal", self.selection)),
            self.inspection.is_some(),
            REVEAL_SECONDS,
        );
        ui.horizontal(|ui| {
            let (preview, preview_response) = ui
                .vertical(|ui| {
                    let (preview, response) =
                        ui.allocate_exact_size(egui::Vec2::splat(PREVIEW), egui::Sense::click());
                    self.paint_preview(ui, look, id, preview);
                    self.state_tabs(ui);
                    (preview, response)
                })
                .inner;
            let _ = preview;
            // Clicking the face opens the image choices, as the image button does.
            let open_image = self.inspection.is_some() && preview_response.clicked();
            if self.inspection.is_some() {
                preview_response.on_hover_text(tr(self.es, "Change the image"));
            }
            ui.add_space(10.);
            if self.inspection.is_none() {
                // A quick read shows the key without a flash of "Loading…".
                let waited = self.selected_at.elapsed();
                if waited >= super::LOADING_DELAY {
                    ui.spinner();
                    ui.label(look.muted_text(tr(self.es, "Loading…")));
                } else {
                    ui.ctx()
                        .request_repaint_after(super::LOADING_DELAY - waited);
                }
                return;
            }
            ui.set_opacity(reveal);
            egui::Grid::new("key-fields")
                .num_columns(2)
                .spacing([12., 8.])
                .show(ui, |ui| {
                    ui.label(look.muted_text(tr(self.es, "Text on key")));
                    // The placeholder says what an empty field shows: the name on a key
                    // without image, nothing on one that has an image to speak for it.
                    let placeholder = if self.draft.state.is_none()
                        && self.draft.image.is_empty()
                        && !self.icon_library.dirty_for(id)
                    {
                        self.draft.name.clone()
                    } else {
                        tr(self.es, "No text").to_owned()
                    };
                    let controls = super::text_style::controls(
                        self.draft.style,
                        self.draft_resolved_text().position,
                        self.draft_has_image(),
                    );
                    ui.horizontal(|ui| {
                        ui.add_enabled(
                            controls.text,
                            look.field(&mut self.draft.text, &placeholder)
                                .desired_width(FIELD),
                        );
                        self.text_placement_menu(ui, look);
                    });
                    ui.end_row();
                    ui.label(look.muted_text(tr(self.es, "Image")));
                    self.image_menu(ui, look, open_image);
                    ui.end_row();
                    if !actions.is_empty() || self.draft.gesture != "press" {
                        ui.label(look.muted_text(tr(self.es, "Gesture")));
                        self.gesture_control(ui, &actions);
                        ui.end_row();
                    }
                    if self.appearance_visible() {
                        ui.label(look.muted_text(tr(self.es, "Appearance")))
                            .on_hover_text(tr(self.es, "What draws this key: you, or a plugin"));
                        ui.horizontal(|ui| self.appearance_control(ui, id));
                        ui.end_row();
                    }
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                // One click, or Delete, deletes: undo is the safety net.
                if glyph_button(
                    ui,
                    look,
                    "🗑",
                    tr(self.es, "Delete (Del; Ctrl+Z brings it back)"),
                )
                .clicked()
                {
                    self.delete_selected();
                }
                // A key without actions has nothing to run.
                if !actions.is_empty()
                    && ui
                        .button(tr(self.es, "⏵ Try"))
                        .on_hover_text(tr(self.es, "Run this key now (or double-click it)"))
                        .clicked()
                {
                    self.try_selected();
                }
                // Shown only when there is something to save; last, so it moves nothing.
                if !self.icon_library.open
                    && self.unsaved()
                    && ui.add(look.primary(tr(self.es, "Save"))).clicked()
                {
                    self.save();
                }
            });
        });
        if self.inspection.is_none() {
            return;
        }
        ui.set_opacity(reveal);
        ui.add_space(6.);
        ui.label(look.section(tr(self.es, "Actions")));
        if actions.is_empty() && !self.app_picker.adds_to(id) {
            ui.label(look.muted_text(tr(
                self.es,
                "Nothing yet — drag an action onto this key from the right",
            )));
        }
        ui.spacing_mut().item_spacing.y = 4.;
        for display_index in 0..self.draft.order.len() {
            let index = self.draft.order[display_index];
            self.action_row(ui, look, &actions[index], display_index);
        }
        self.pending_application_row(ui, look);
        ui.spacing_mut().item_spacing.y = 8.;
        ui.add_space(6.);
        egui::CollapsingHeader::new(look.muted_text(tr(self.es, "Advanced")))
            .id_salt("advanced")
            .show(ui, |ui| self.advanced(ui, look, id, &actions));
    }
    fn state_entries(&self) -> Option<crate::plugins::CatalogEntry> {
        let key = self.current_visual()?;
        self.plugins
            .catalog
            .iter()
            .find(|e| e.type_key == key && !e.states().is_empty())
            .cloned()
    }
    fn prepare_state_tab(&mut self, ui: &egui::Ui) {
        let Some(entry) = self.state_entries() else {
            return;
        };
        let observed = self
            .plugins
            .visuals
            .get(&self.draft.id)
            .filter(|f| f.state == "current" || f.state == "stale")
            .and_then(|f| f.visual_state.as_deref())
            .filter(|id| entry.states().iter().any(|s| s.id == *id));
        self.draft.initial_state(
            observed,
            &entry.states()[1].id,
            self.icon_library.open || self.icon_library.dirty_for(&self.draft.id),
        );
        self.plugins.state_icons.retain(|key, _| {
            self.plugins.catalog.iter().any(|e| {
                e.states()
                    .iter()
                    .any(|state| key == &format!("{}:{}", e.type_key, state.id))
            })
        });
        for state in entry.states() {
            let Some(hex) = &state.icon else {
                continue;
            };
            let key = format!("{}:{}", entry.type_key, state.id);
            if self
                .plugins
                .state_icons
                .get(&key)
                .is_some_and(|(old, _)| old == hex)
            {
                continue;
            }
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
                .collect::<Result<Vec<_>, _>>();
            if let Ok(bytes) = bytes
                && let Ok(pixels) = crate::preview::decode(&bytes)
            {
                let texture = ui.ctx().load_texture(
                    &key,
                    egui::ColorImage::from_rgba_unmultiplied(pixels.size, &pixels.rgba),
                    Default::default(),
                );
                self.plugins.state_icons.insert(key, (hex.clone(), texture));
            }
        }
    }
    fn state_tabs(&mut self, ui: &mut egui::Ui) {
        let Some(entry) = self.state_entries() else {
            return;
        };
        for state in entry.states() {
            if ui
                .selectable_label(
                    self.draft.state.as_deref() == Some(&state.id),
                    entry.texts.get(self.es, &state.label),
                )
                .clicked()
            {
                self.draft.state_initialized = true;
                if self.draft.state.as_deref() != Some(&state.id) {
                    self.icon_library.reset();
                    self.draft.select_state(state.id.clone());
                }
            }
        }
    }
    /// The key face as the deck will show it; once loaded it follows the draft, so edits
    /// show before saving.
    fn paint_preview(&self, ui: &egui::Ui, look: &Look, id: &str, preview: egui::Rect) {
        let face = self.plugins.visuals.get(id);
        let state = self.draft.state.as_ref();
        let now = ui.input(|i| i.time);
        let state_key = state.map(|state| {
            format!(
                "{}:{state}",
                self.draft.visual_type.as_deref().unwrap_or("")
            )
        });
        let picture = self
            .icon_library
            .preview(id)
            .map(|texture| (texture, 1.))
            .or_else(|| self.pictures.get(&self.draft.image, now))
            .or_else(|| {
                state_key
                    .as_ref()
                    .and_then(|key| self.plugins.state_icons.get(key))
                    .map(|(_, texture)| (texture, 1.))
            })
            .or_else(|| {
                face.filter(|face| state.is_none() || face.visual_state.as_ref() == state)
                    .and_then(|face| face.texture.as_ref())
                    .map(|texture| (texture, 1.))
            });
        let native = self
            .inspection
            .as_ref()
            .filter(|view| view.appearance["visual_type"].is_null())
            .and_then(|view| {
                native_icon(&view.appearance).or_else(|| {
                    view.actions
                        .iter()
                        .find_map(|a| match a["fields"]["type"].as_str() {
                            Some("sleep") => Some(rad_api::NativeIcon::Sleep),
                            Some("brightness") => Some(rad_api::NativeIcon::Brightness),
                            _ => None,
                        })
                })
            });
        let mut preview_look = Look::new(&self.palette);
        if let Some(c) = face.and_then(|face| face.color) {
            preview_look.key = egui::Color32::from_rgb(
                (u16::from(c[0]) * u16::from(c[3]) / 255) as u8,
                (u16::from(c[1]) * u16::from(c[3]) / 255) as u8,
                (u16::from(c[2]) * u16::from(c[3]) / 255) as u8,
            );
        }
        paint_key(
            ui,
            &preview_look,
            preview,
            self.draft_resolved_text(),
            false,
            picture,
            native,
        );
        if let Some(face) = face {
            self.paint_plugin_status(ui, look, preview, &face.state);
        }
    }
    fn draft_has_image(&self) -> bool {
        if !self.draft.image.is_empty() || self.icon_library.preview(&self.draft.id).is_some() {
            return true;
        }
        if let Some(state) = &self.draft.state {
            let key = format!(
                "{}:{state}",
                self.draft.visual_type.as_deref().unwrap_or("")
            );
            self.plugins.state_icons.contains_key(&key)
                || self
                    .plugins
                    .visuals
                    .get(&self.draft.id)
                    .is_some_and(|face| {
                        face.visual_state.as_ref() == Some(state) && face.texture.is_some()
                    })
        } else {
            self.plugins
                .visuals
                .get(&self.draft.id)
                .is_some_and(|face| face.texture.is_some())
                || self.inspection.as_ref().is_some_and(|view| {
                    native_icon(&view.appearance).is_some()
                        || (view.appearance["visual_type"].is_null()
                            && view.actions.iter().any(|a| {
                                matches!(a["fields"]["type"].as_str(), Some("sleep" | "brightness"))
                            }))
                })
        }
    }
    fn draft_resolved_text(&self) -> rad_api::ResolvedText<'_> {
        let plugin = self
            .draft
            .state
            .is_none()
            .then(|| self.plugins.visuals.get(&self.draft.id))
            .flatten()
            .and_then(|face| face.source_text.as_deref());
        self.draft.style.resolve(
            &self.draft.text,
            self.draft_has_image(),
            plugin,
            self.draft
                .state
                .is_none()
                .then_some(self.draft.name.as_str()),
        )
    }
    /// A position click fixes the automatic choice; changing color, size or band does not.
    fn text_placement_menu(&mut self, ui: &mut egui::Ui, look: &Look) {
        use rad_api::TextPosition as P;
        let button = menu_button(ui, tr(self.es, "Style"));
        egui::Popup::menu(&button)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                let effective = self.draft_resolved_text().position;
                let controls = super::text_style::controls(
                    self.draft.style,
                    effective,
                    self.draft_has_image(),
                );
                let row = match effective {
                    P::AboveImage => P::Top,
                    P::BelowImage => P::Bottom,
                    p => p,
                };
                let beside = matches!(effective, P::AboveImage | P::BelowImage);
                ui.horizontal(|ui| {
                    for (value, label) in [
                        (P::Top, "Top"),
                        (P::Middle, "Middle"),
                        (P::Bottom, "Bottom"),
                        (P::Hidden, "No text"),
                    ] {
                        if ui
                            .selectable_label(row == value, tr(self.es, label))
                            .clicked()
                        {
                            self.draft.style.position = Some(match (value, beside) {
                                (P::Top, true) => P::AboveImage,
                                (P::Bottom, true) => P::BelowImage,
                                _ => value,
                            });
                        }
                    }
                });
                ui.add_enabled_ui(controls.placement, |ui| {
                    ui.horizontal(|ui| {
                        for (value, label) in [(false, "Over the image"), (true, "Moves the image")]
                        {
                            if ui
                                .selectable_label(beside == value, tr(self.es, label))
                                .clicked()
                            {
                                self.draft.style.position = Some(match (row, value) {
                                    (P::Top, true) => P::AboveImage,
                                    (P::Top, false) => P::Top,
                                    (_, true) => P::BelowImage,
                                    (_, false) => P::Bottom,
                                });
                            }
                        }
                    });
                });
                ui.add_enabled_ui(controls.size_color, |ui| {
                    ui.add(
                        egui::Slider::new(&mut self.draft.style.size, 8..=24)
                            .step_by(1.)
                            .text(tr(self.es, "Size")),
                    );
                    super::icon_library::color_row(
                        ui,
                        look,
                        self.es,
                        "Text color",
                        &mut self.draft.style.color,
                        [255; 3],
                    );
                });
                ui.add_enabled_ui(controls.band, |ui| {
                    ui.checkbox(
                        &mut self.draft.style.band,
                        tr(self.es, "Bar behind the text"),
                    );
                });
            });
    }
    /// Button with everything about the key's picture: open the icon library, take the icon
    /// of the application the key opens, import a PNG/JPEG, or remove it. Imports and removals
    /// stick with Save.
    fn image_menu(&mut self, ui: &mut egui::Ui, look: &Look, open_now: bool) {
        let has_image = !self.draft.image.is_empty() || self.icon_library.dirty_for(&self.draft.id);
        let label = if has_image {
            tr(self.es, "Change")
        } else {
            tr(self.es, "Choose")
        };
        let button = menu_button(ui, label);
        if open_now {
            egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&button));
        }
        // Offered only where it can work: no image yet and an application on top.
        let icon_available = !has_image
            && self
                .inspection
                .as_ref()
                .is_some_and(|i| crate::icons::wanted(i).is_some());
        egui::Popup::menu(&button)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                // egui draws menu entries as bare text until hovered; next to a label and a
                // field they would read as text, so they keep the editor's button look.
                ui.reset_style();
                let manual = self
                    .inspection
                    .as_ref()
                    .is_some_and(|i| i.appearance["visual_type"].is_null());
                if (manual || self.draft.state.is_some())
                    && ui.button(tr(self.es, "Icon library")).clicked()
                {
                    // Start from what drew the current image, when this computer remembers it.
                    let recalled = crate::icon_memory::default_path()
                        .and_then(|path| crate::icon_memory::recall(&path, &self.draft.image));
                    self.icon_library.begin(&self.draft.id, recalled);
                    ui.close();
                }
                if icon_available
                    && ui
                        .button(tr(self.es, "Use app icon"))
                        .on_hover_text(tr(
                            self.es,
                            "Take the installed icon of the application this key opens first",
                        ))
                        .clicked()
                    && self.command(Job::AppIcon {
                        button: self.draft.id.clone(),
                        application: None,
                    })
                {
                    self.icon_library.reset();
                    self.image_target = Some((self.draft.id.clone(), self.draft.state.clone()));
                    self.icon_asked = true;
                    ui.close();
                }
                if manual || icon_available {
                    ui.separator();
                }
                ui.label(look.muted_text(tr(self.es, "From a PNG or JPEG file")));
                ui.horizontal(|ui| {
                    ui.add(
                        look.field(
                            &mut self.image_path,
                            tr(self.es, "/path/to/image.png or .jpg"),
                        )
                        .desired_width(FIELD),
                    );
                    if ui
                        .add_enabled(
                            !self.image_path.trim().is_empty(),
                            egui::Button::new(tr(self.es, "Import")),
                        )
                        .clicked()
                        && self.command(Job::Transfer {
                            path: PathBuf::from(self.image_path.trim()),
                            kind: TransferKind::Image,
                        })
                    {
                        self.image_target = Some((self.draft.id.clone(), self.draft.state.clone()));
                        self.icon_library.reset();
                        self.transfer_active = true;
                        ui.close();
                    }
                });
                if has_image {
                    ui.separator();
                    if ui
                        .add(look.dangerous(tr(self.es, "Remove image")))
                        .clicked()
                    {
                        self.icon_library.reset();
                        self.draft.image.clear();
                        self.discard_unused_imports();
                        ui.close();
                    }
                }
            });
    }
    /// One action of the key, as a row that can be dragged by its handle to reorder.
    fn action_row(&mut self, ui: &mut egui::Ui, look: &Look, action: &Value, display_index: usize) {
        let kind = string(&action["fields"]["type"]);
        let card = look.card().show(ui, |ui| {
            ui.set_width(ACTION_WIDTH.min(ui.available_width()));
            ui.horizontal(|ui| {
                let (handle_rect, handle) =
                    ui.allocate_exact_size(egui::vec2(30., 26.), egui::Sense::drag());
                paint_icon(
                    ui.painter(),
                    handle_rect.left_center() + egui::vec2(6., 0.),
                    Icon::Grip,
                    look.muted,
                );
                ui.painter().text(
                    handle_rect.left_center() + egui::vec2(16., 0.),
                    egui::Align2::LEFT_CENTER,
                    display_index + 1,
                    egui::FontId::proportional(13.),
                    look.muted,
                );
                handle
                    .on_hover_text(tr(self.es, "Drag to reorder"))
                    .dnd_set_drag_payload(Drag::Reorder(display_index));
                self.action_content(ui, look, action, &kind, display_index);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !matches!(kind.as_str(), "enter_folder" | "return")
                        && icon_button(ui, look, Icon::Close, tr(self.es, "Remove action"))
                            .clicked()
                    {
                        let id = string(&action["id"]);
                        let texts = self.plugins.texts(&kind);
                        if texts.binds_visual(&kind) {
                            self.command(Job::LinkedAction(crate::linked_actions::LinkedAction {
                                button: self.draft.id.clone(),
                                type_key: kind.clone(),
                                texts,
                                mutation: crate::linked_actions::Mutation::Remove { id },
                            }));
                        } else {
                            self.send(crate::Request::Edit {
                                changes: vec![rad_api::Change::RemoveAction { id }],
                                expected_revision: None,
                            });
                        }
                    }
                    if display_index > 0
                        && icon_button(ui, look, Icon::Up, tr(self.es, "Move up")).clicked()
                    {
                        self.draft.order.swap(display_index, display_index - 1);
                    }
                });
            });
        });
        if let Some(from) = card.response.dnd_release_payload::<Drag>()
            && let Drag::Reorder(from) = *from
        {
            self.draft.order.swap(from, display_index);
        }
    }
    /// What an action does, in words, with the one setting that some actions show inline.
    fn action_content(
        &mut self,
        ui: &mut egui::Ui,
        look: &Look,
        action: &Value,
        kind: &str,
        display_index: usize,
    ) {
        match kind {
            "launcher" => {
                ui.label(tr(self.es, "Open App"));
                let index = self.draft.order[display_index];
                let entry = self.draft.entries[index].clone();
                if let Some(chosen) =
                    self.application_choice(ui, look, &string(&action["id"]), &entry)
                {
                    self.draft.entries[index] = chosen.desktop_entry;
                }
            }
            "enter_folder" => {
                ui.label(tr(self.es, "Open this folder"));
            }
            "return" => {
                ui.label(tr(self.es, "Go back to the parent folder"));
            }
            "sleep" => {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(22., 22.), egui::Sense::hover());
                paint_icon(ui.painter(), rect.center(), Icon::Moon, look.muted);
                ui.label(tr(self.es, "Sleep")).on_hover_text(tr(
                    self.es,
                    "Any key wakes the deck without running its action",
                ));
            }
            "brightness" => {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(22., 22.), egui::Sense::hover());
                paint_icon(ui.painter(), rect.center(), Icon::Sun, look.muted);
                ui.label(tr(self.es, "Brightness"));
                let index = self.draft.order[display_index];
                for (direction, label) in [
                    (rad_api::BrightnessDirection::Increase, "Increase (+10)"),
                    (rad_api::BrightnessDirection::Decrease, "Decrease (−10)"),
                ] {
                    ui.selectable_value(
                        &mut self.draft.brightness_directions[index],
                        direction,
                        tr(self.es, label),
                    );
                }
            }
            "wait" => {
                ui.label(tr(self.es, "Wait"));
                let index = self.draft.order[display_index];
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.waits[index])
                        .desired_width(64.)
                        .margin(egui::vec2(6., 3.)),
                );
                ui.label(look.muted_text(tr(self.es, "ms")));
            }
            other => {
                ui.label(self.plugin_label(other)).on_hover_text(other);
                if self.connected
                    && !self.plugins.form_pending
                    && !self.plugins.form_loading
                    && ui.small_button(tr(self.es, "Configure…")).clicked()
                {
                    self.open_plugin_form(
                        other,
                        Some(string(&action["id"])),
                        crate::plugins::FormTarget::Action(string(&action["id"])),
                    );
                }
            }
        }
    }
    /// Name, run mode, identifiers, automation permissions and cross-folder moves:
    /// needed rarely.
    fn advanced(&mut self, ui: &mut egui::Ui, look: &Look, id: &str, actions: &[Value]) {
        let copy = tr(self.es, "Copy");
        egui::Grid::new("advanced")
            .num_columns(2)
            .spacing([14., 10.])
            .show(ui, |ui| {
                ui.label(look.muted_text(tr(self.es, "Name")))
                    .on_hover_text(tr(
                        self.es,
                        "Shown on the key while it has no text of its own",
                    ));
                ui.add(look.field(&mut self.draft.name, "").desired_width(FIELD));
                ui.end_row();
                if actions.len() > 1 {
                    ui.label(look.muted_text(tr(self.es, "Run")));
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.draft.mode,
                            "sequential".into(),
                            tr(self.es, "Sequential"),
                        )
                        .on_hover_text(tr(self.es, "Each action waits for the previous one"));
                        ui.selectable_value(
                            &mut self.draft.mode,
                            "parallel".into(),
                            tr(self.es, "Parallel"),
                        )
                        .on_hover_text(tr(
                            self.es,
                            "Actions start without waiting for the previous one",
                        ));
                    });
                    ui.end_row();
                }
                ui.label(look.muted_text(tr(self.es, "Automation may")))
                    .on_hover_text(tr(self.es, "What rad-cli and scripts may do with this key"));
                if permission_ui(ui, &mut self.draft.permissions, self.es, true) {
                    self.send(crate::Request::SetPermissions {
                        target: id.into(),
                        value: serde_json::from_value(self.draft.permissions.clone())
                            .expect("permission draft"),
                    });
                }
                ui.end_row();
                ui.label(look.muted_text(tr(self.es, "Copy or move")));
                ui.horizontal(|ui| {
                    ui.label(look.muted_text(tr(self.es, "to folder ID")));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.move_folder)
                            .desired_width(70.)
                            .margin(egui::vec2(6., 3.)),
                    );
                    ui.label(look.muted_text(tr(self.es, "key")));
                    let mut key = self.move_position + 1;
                    ui.add(egui::DragValue::new(&mut key).range(1..=32));
                    self.move_position = key - 1;
                    if ui.button(tr(self.es, "Duplicate")).clicked() {
                        self.send(crate::Request::Duplicate {
                            id: id.into(),
                            folder_id: self.move_folder.clone(),
                            position: self.move_position,
                            replace: false,
                        });
                    }
                    if let Some(request) =
                        commands::move_folder(actions, &self.move_folder, self.move_position)
                        && ui.button(tr(self.es, "Move folder")).clicked()
                    {
                        self.send(request);
                    }
                });
                ui.end_row();
                ui.label(look.muted_text(tr(self.es, "Identifiers")));
                ui.horizontal(|ui| {
                    ui.label(look.muted_text(tr(self.es, "Key")));
                    ui.monospace(id);
                    if ui.small_button(copy).clicked() {
                        ui.ctx().copy_text(id.to_owned());
                    }
                    ui.add_space(12.);
                    let folder = self.folder().unwrap_or_default();
                    ui.label(look.muted_text(tr(self.es, "Folder")));
                    ui.monospace(&folder);
                    if ui.small_button(copy).clicked() {
                        ui.ctx().copy_text(folder);
                    }
                });
                ui.end_row();
                for (index, action) in actions.iter().enumerate() {
                    let action_id = string(&action["id"]);
                    ui.label(look.muted_text(tr(self.es, "Action")));
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.monospace(&action_id);
                            if ui.small_button(copy).clicked() {
                                ui.ctx().copy_text(action_id.clone());
                            }
                            if action["fields"]["type"] == "launcher" {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.draft.entries[index])
                                        .desired_width(180.)
                                        .margin(egui::vec2(6., 3.)),
                                );
                            }
                        });
                        if permission_ui(
                            ui,
                            &mut self.draft.action_permissions[index],
                            self.es,
                            true,
                        ) {
                            self.send(crate::Request::SetPermissions {
                                target: string(&action["id"]),
                                value: serde_json::from_value(
                                    self.draft.action_permissions[index].clone(),
                                )
                                .expect("permission draft"),
                            });
                        }
                    });
                    ui.end_row();
                }
                ui.label(look.muted_text(tr(self.es, "Image reference")));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.draft.image)
                            .desired_width(FIELD)
                            .margin(egui::vec2(6., 3.)),
                    );
                    if ui.button(tr(self.es, "Reload draft")).clicked() {
                        self.inspect_needed = true;
                    }
                });
                ui.end_row();
            });
    }
}

/// Consult/Modify/Execute for automation. Returns true when the user applies the value.
pub(super) fn permission_ui(
    ui: &mut egui::Ui,
    value: &mut Value,
    es: impl Into<crate::preferences::Language> + Copy,
    allow_inherit: bool,
) -> bool {
    let mut inherit = allow_inherit && value["inherit"] == true;
    let mut next = if value["own"].is_object() {
        value.clone()
    } else {
        json!({"own":{"consult":true,"modify":true,"execute":false}})
    };
    let mut save = false;
    ui.horizontal(|ui| {
        if allow_inherit {
            ui.checkbox(&mut inherit, tr(es, "Inherit"));
        }
        ui.add_enabled_ui(!inherit, |ui| {
            for (field, key, en) in [
                (commands::PermissionField::Consult, "consult", "Consult"),
                (commands::PermissionField::Modify, "modify", "Modify"),
                (commands::PermissionField::Execute, "execute", "Execute"),
            ] {
                let mut enabled = next["own"][key].as_bool().unwrap_or(false);
                if ui.checkbox(&mut enabled, tr(es, en)).changed() {
                    next = commands::permission_change(&next, field, enabled);
                }
            }
        });
        save = ui.small_button(tr(es, "Apply permissions")).clicked();
    });
    *value = if inherit {
        json!({"inherit":true})
    } else {
        next
    };
    save
}

impl App {
    fn gesture_control(&mut self, ui: &mut egui::Ui, actions: &[Value]) {
        let choices = [
            ("press", "Press", rad_api::Gesture::Press),
            ("hold", "Hold", rad_api::Gesture::Hold),
            ("long_press", "Long press", rad_api::Gesture::LongPress),
        ];
        let allowed = crate::plugins::plan_gestures(actions, &self.plugins.catalog);
        let enabled: Vec<_> = choices
            .iter()
            .map(|(_, _, g)| allowed.contains(g))
            .collect();
        let current_label = choices
            .iter()
            .find(|(key, _, _)| *key == self.draft.gesture)
            .map(|(_, label, _)| *label)
            .unwrap_or("Press");
        if allowed.is_empty() {
            ui.label(tr(self.es, current_label));
            return;
        }
        egui::ComboBox::from_id_salt("button-gesture")
            .selected_text(tr(self.es, current_label))
            .show_ui(ui, |ui| {
                for ((key, label, _), enabled) in choices.iter().zip(enabled) {
                    if enabled {
                        ui.selectable_value(
                            &mut self.draft.gesture,
                            (*key).into(),
                            tr(self.es, label),
                        );
                    }
                }
            });
    }
}

#[cfg(test)]
mod brightness_tests {
    use super::*;

    fn app() -> App {
        let root =
            std::env::temp_dir().join(format!("rad-brightness-inspector-{}", std::process::id()));
        let mut app = App::with_preferences_path(root.clone(), root.join("theme"), None);
        app.es = crate::preferences::Language::En;
        app.inspection = Some(crate::session::Inspection {
            button: json!({"id":"b2","fields":{"name":"Light","mode":"sequential","gesture":"press"}}),
            appearance: json!({"text":"","image":null,"text_position":"bottom"}),
            actions: vec![
                json!({"id":"a3","fields":{"type":"brightness","parameters":{"direction":"increase"}}}),
            ],
        });
        app.draft = Draft::load(app.inspection.as_ref().unwrap());
        app
    }

    #[test]
    fn changing_brightness_direction_is_an_unsaved_draft_until_one_atomic_save() {
        let mut app = app();
        assert!(!app.unsaved());
        app.draft.brightness_directions[0] = rad_api::BrightnessDirection::Decrease;
        assert!(app.unsaved());
        let (sent, received) = std::sync::mpsc::channel();
        app.worker = crate::worker::Worker::spawn(move |job| {
            if let Job::Request(request) = job {
                sent.send(request.arguments()).unwrap();
            }
            crate::native::Event {
                paste: false,
                plugin_apply: false,
                plugin_admin: false,
                plugin_manifest: false,
                plugin_dependencies: false,
                plugin_form: false,
                plugin_visual: false,
                background: false,
                connected: false,
                transfer_job: false,
                palette: Default::default(),
                result: Err(crate::session::Error::protocol()),
            }
        });
        assert!(
            received.try_recv().is_err(),
            "a direction draft sends nothing"
        );
        app.save();
        let request = received
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let changes = request["changes"].as_array().unwrap();
        assert_eq!(
            changes
                .iter()
                .filter(|edit| edit["id"] == "a3")
                .collect::<Vec<_>>(),
            vec![
                &json!({"kind":"set","id":"a3","field":"parameters","value":{"direction":"decrease"}})
            ]
        );
        assert!(app.unsaved(), "sending a request is not a publication");
    }
    #[test]
    fn hidden_plugin_text_returns_in_preview_before_saving_and_states_ignore_it() {
        let mut app = app();
        app.plugins.visuals.insert(
            "b2".into(),
            super::super::plugins::Face {
                visual_state: None,
                revision: "1".into(),
                state: "current".into(),
                text: Some(String::new()),
                source_text: Some("Playing".into()),
                style: rad_api::TextStyle {
                    position: Some(rad_api::TextPosition::Hidden),
                    ..Default::default()
                },
                color: None,
                texture: None,
            },
        );
        app.draft.style.position = Some(rad_api::TextPosition::Hidden);
        app.draft.text = "Preserved".into();
        assert_eq!(app.draft_resolved_text().text, "");
        assert_eq!(app.draft.text, "Preserved");
        app.draft.style.position = Some(rad_api::TextPosition::Top);
        assert_eq!(app.draft_resolved_text().text, "Playing");
        app.draft.select_state("off".into());
        assert_eq!(app.draft_resolved_text().text, "");
        app.draft.text = "Quiet".into();
        assert_eq!(app.draft_resolved_text().text, "Quiet");
    }
}
