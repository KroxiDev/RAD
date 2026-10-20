//! Bottom panel: what the selected key is and does. Only Try runs anything.
use super::{
    App, Draft, Drag,
    deck::{key_label, paint_key},
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

impl App {
    pub(super) fn inspector(&mut self, ui: &mut egui::Ui, look: &Look) {
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
    fn unsaved(&self) -> bool {
        self.inspection.as_ref().is_some_and(|published| {
            let published = Draft::load(published);
            let draft = &self.draft;
            published.name != draft.name
                || published.text != draft.text
                || published.image != draft.image
                || published.text_position != draft.text_position
                || published.mode != draft.mode
                || published.gesture != draft.gesture
                || published.order != draft.order
                || published.waits != draft.waits
        })
    }
    fn save(&mut self) {
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
        // Sent only when changed here, so an untouched key keeps following its image.
        let published_position = self
            .inspection
            .as_ref()
            .map(|i| string(&i.appearance["text_position"]));
        let text_position = (published_position.as_deref()
            != Some(self.draft.text_position.as_str()))
        .then_some(self.draft.text_position.as_str());
        let request = commands::save_button(&commands::KeySave {
            id: &self.draft.id,
            name: &self.draft.name,
            text: &self.draft.text,
            image: (!self.draft.image.is_empty()).then_some(self.draft.image.as_str()),
            text_position,
            mode: &self.draft.mode,
            gesture: &self.draft.gesture,
            order: &self.draft.order,
            waits: &waits,
        });
        self.send(request);
    }
    fn key_editor(&mut self, ui: &mut egui::Ui, look: &Look, id: &str) {
        let actions = self
            .inspection
            .as_ref()
            .map(|i| i.actions.clone())
            .unwrap_or_default();
        ui.horizontal(|ui| {
            let (preview, preview_response) =
                ui.allocate_exact_size(egui::Vec2::splat(PREVIEW), egui::Sense::click());
            self.paint_preview(ui, look, id, preview);
            // Clicking the face opens the image choices, as the image button does.
            let open_image = self.inspection.is_some() && preview_response.clicked();
            if self.inspection.is_some() {
                preview_response.on_hover_text(tr(self.es, "Change the image"));
            }
            ui.add_space(10.);
            if self.inspection.is_none() {
                ui.spinner();
                ui.label(look.muted_text(tr(self.es, "Loading…")));
            } else {
                egui::Grid::new("key-fields")
                    .num_columns(2)
                    .spacing([12., 8.])
                    .show(ui, |ui| {
                        ui.label(look.muted_text(tr(self.es, "Text on key")));
                        // The placeholder says what an empty field shows: the name on a key
                        // without image, nothing on one that has an image to speak for it.
                        let placeholder = if self.draft.image.is_empty() {
                            self.draft.name.clone()
                        } else {
                            tr(self.es, "No text").to_owned()
                        };
                        ui.horizontal(|ui| {
                            ui.add(
                                look.field(&mut self.draft.text, &placeholder)
                                    .desired_width(FIELD),
                            );
                            self.text_placement_menu(ui);
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
                                .on_hover_text(tr(
                                    self.es,
                                    "What draws this key: you, or a plugin",
                                ));
                            ui.horizontal(|ui| self.appearance_control(ui, id));
                            ui.end_row();
                        }
                    });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                // One click deletes: undo, a keystroke away, is the safety net.
                if glyph_button(ui, look, "🗑", tr(self.es, "Delete (Ctrl+Z brings it back)"))
                    .clicked()
                {
                    self.send(commands::delete_selection(id, &actions));
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
                if self.unsaved() && ui.add(look.primary(tr(self.es, "Save"))).clicked() {
                    self.save();
                }
            });
        });
        if self.inspection.is_none() {
            return;
        }
        ui.add_space(6.);
        ui.label(look.section(tr(self.es, "Actions")));
        if actions.is_empty() {
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
        ui.spacing_mut().item_spacing.y = 8.;
        ui.add_space(6.);
        egui::CollapsingHeader::new(look.muted_text(tr(self.es, "Advanced")))
            .id_salt("advanced")
            .show(ui, |ui| self.advanced(ui, look, id, &actions));
    }
    /// The key face as the deck will show it; once loaded it follows the draft, so edits
    /// show before saving.
    fn paint_preview(&self, ui: &egui::Ui, look: &Look, id: &str, preview: egui::Rect) {
        let Some(view) = self
            .board
            .as_ref()
            .and_then(|b| b.buttons.iter().find(|b| b.entity["id"] == id))
        else {
            return;
        };
        let published = view.appearance["image"].as_str();
        let (label, position, image) = if self.inspection.is_some() {
            (
                key_label(
                    &self.draft.text,
                    &self.draft.name,
                    !self.draft.image.is_empty(),
                ),
                self.draft.text_position.clone(),
                published.filter(|reference| *reference == self.draft.image),
            )
        } else {
            (
                key_label(
                    &string(&view.appearance["text"]),
                    &string(&view.entity["fields"]["name"]),
                    published.is_some(),
                ),
                string(&view.appearance["text_position"]),
                published,
            )
        };
        let texture = image.and_then(|reference| self.textures.get(reference));
        if !self.plugin_face(ui, look, id, preview) {
            paint_key(ui, look, preview, &label, &position, texture);
        }
    }
    /// Button that opens how the text is placed: at the top, in the middle or at the
    /// bottom, and either over the image or beside it. Applied with Save.
    fn text_placement_menu(&mut self, ui: &mut egui::Ui) {
        let button = menu_button(ui, tr(self.es, "Position"))
            .on_hover_text(tr(self.es, "Where the text goes on the key"));
        egui::Popup::menu(&button)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                let (row, beside) = match self.draft.text_position.as_str() {
                    "top" => ("top", false),
                    "above_image" => ("top", true),
                    "bottom" => ("bottom", false),
                    "below_image" => ("bottom", true),
                    _ => ("middle", false),
                };
                let mut choice = (row, beside);
                ui.horizontal(|ui| {
                    for (value, en) in [("top", "Top"), ("middle", "Middle"), ("bottom", "Bottom")]
                    {
                        if ui.selectable_label(row == value, tr(self.es, en)).clicked() {
                            choice.0 = value;
                        }
                    }
                });
                ui.add_enabled_ui(row != "middle", |ui| {
                    ui.horizontal(|ui| {
                        for (value, en, hint_en) in [
                            (
                                false,
                                "Over the image",
                                "The image fills the key and the text is drawn on it",
                            ),
                            (
                                true,
                                "Moves the image",
                                "The image shrinks so the text has room of its own",
                            ),
                        ] {
                            if ui
                                .selectable_label(beside == value, tr(self.es, en))
                                .on_hover_text(tr(self.es, hint_en))
                                .clicked()
                            {
                                choice.1 = value;
                            }
                        }
                    });
                });
                self.draft.text_position = match choice {
                    ("top", false) => "top",
                    ("top", true) => "above_image",
                    ("bottom", false) => "bottom",
                    ("bottom", true) => "below_image",
                    _ => "middle",
                }
                .into();
            });
    }
    /// Button with everything about the key's picture: import a PNG/JPEG, take the icon of
    /// the application the key opens, or remove it. Imports and removals stick with Save.
    fn image_menu(&mut self, ui: &mut egui::Ui, look: &Look, open_now: bool) {
        let has_image = !self.draft.image.is_empty();
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
                        self.transfer_active = true;
                        ui.close();
                    }
                });
                if icon_available || has_image {
                    ui.separator();
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
                    self.icon_asked = true;
                    ui.close();
                }
                if has_image && ui.button(tr(self.es, "Remove image")).clicked() {
                    self.draft.image.clear();
                    ui.close();
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
                        self.send(crate::Request::Delete {
                            id: string(&action["id"]),
                            confirm_contents: false,
                        });
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
                let entry = string(&action["fields"]["parameters"]["desktop_entry"]);
                let name = self
                    .catalog
                    .entries
                    .iter()
                    .find(|e| e.desktop_entry == entry)
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| entry.trim_end_matches(".desktop").to_owned());
                ui.label(format!("{} {name}", tr(self.es, "Open")));
            }
            "enter_folder" => {
                ui.label(tr(self.es, "Open this folder"));
            }
            "return" => {
                ui.label(tr(self.es, "Go back to the parent folder"));
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
                                if ui.small_button(tr(self.es, "Apply entry")).clicked() {
                                    self.send(crate::Request::Edit {
                                        changes: vec![rad_api::Change::Set {
                                            id: string(&action["id"]),
                                            edit: rad_api::FieldEdit::DesktopEntry(
                                                self.draft.entries[index].clone(),
                                            ),
                                        }],
                                    });
                                }
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
