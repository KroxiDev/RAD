//! Contextual library in the actions column. Raster work stays off the GUI thread.
use super::{App, look::Look, tr};
use crate::{
    icon_library::{Choice, ENTRIES, GROUPS, Rendered, Renderer, SCALE_STEP, SCALES},
    preferences::Language,
    preview::Pixels,
    worker::{Polled, Worker},
};
use eframe::egui::{self, Color32, Stroke, StrokeKind};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone)]
struct Prepared {
    choice: Choice,
    texture: egui::TextureHandle,
    png: Arc<Vec<u8>>,
}
enum RenderJob {
    Preview(u64, Choice),
    Thumbnails(Vec<usize>),
}
enum RenderReply {
    Preview(u64, Choice, Result<Rendered, String>),
    Thumbnails(Vec<(usize, Result<Pixels, String>)>),
}
#[derive(Default)]
pub(super) struct Library {
    pub open: bool,
    owner: String,
    query: String,
    scroll_query: Option<String>,
    choice: Option<Choice>,
    /// What this computer remembers drew the key's image: the choice starts here and only a
    /// different one is a change to save.
    recalled: Option<Choice>,
    /// Scroll the grid once to the recalled icon.
    reveal: bool,
    prepared: Option<Prepared>,
    before: Option<Prepared>,
    generation: u64,
    submitted: Option<u64>,
    pending_save: Option<(String, u64)>,
    worker: Option<Worker<RenderJob, RenderReply>>,
    failed: bool,
    error: Option<String>,
    thumbnails: BTreeMap<usize, egui::TextureHandle>,
    wanted_thumbnails: BTreeSet<usize>,
    bad_thumbnails: BTreeSet<usize>,
}
impl Library {
    pub fn reset(&mut self) {
        self.open = false;
        self.owner.clear();
        self.choice = None;
        self.recalled = None;
        self.reveal = false;
        self.prepared = None;
        self.before = None;
        self.pending_save = None;
        self.error = None;
        self.failed = false;
        self.invalidate();
    }
    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.submitted = None;
    }
    pub fn begin(&mut self, owner: &str, recalled: Option<Choice>) {
        if self.owner != owner {
            self.reset();
            self.owner = owner.into();
        }
        if !self.open {
            self.before = self.prepared.clone();
            self.query.clear();
            self.scroll_query = None;
            self.open = true;
            if self.choice.is_none()
                && let Some(choice) = recalled
            {
                self.choice = Some(choice);
                self.recalled = Some(choice);
                self.reveal = true;
                self.invalidate();
            }
        }
    }
    pub fn cancel(&mut self) {
        if self.pending_save.is_some() {
            return;
        }
        self.prepared = self.before.take();
        self.choice = self.prepared.as_ref().map(|p| p.choice);
        if self.choice.is_none() {
            self.recalled = None;
        }
        self.reveal = false;
        self.error = None;
        self.open = false;
        self.invalidate();
    }
    fn choose(&mut self, choice: Choice) {
        if self.choice != Some(choice) {
            self.choice = Some(choice);
            self.error = None;
            self.invalidate();
        }
    }
    pub fn owns_draft(&self, owner: &str) -> bool {
        self.owner == owner && (self.open || self.choice.is_some())
    }
    pub fn pending(&self) -> bool {
        self.pending_save.is_some()
    }
    pub fn dirty_for(&self, owner: &str) -> bool {
        self.owner == owner && self.choice.is_some() && self.choice != self.recalled
    }
    pub fn preview(&self, owner: &str) -> Option<&egui::TextureHandle> {
        (self.owner == owner)
            .then_some(self.prepared.as_ref())
            .flatten()
            .map(|p| &p.texture)
    }
    pub fn ready(&self) -> bool {
        self.pending_save.is_none()
            && self.choice.is_some()
            && self
                .prepared
                .as_ref()
                .is_some_and(|p| Some(p.choice) == self.choice)
    }
    pub fn save_job(&self, request: crate::Request) -> Option<crate::native::Job> {
        if !self.ready() {
            return None;
        }
        Some(crate::native::Job::SaveIcon {
            state: None,
            button: self.owner.clone(),
            ticket: self.generation,
            png: self.prepared.as_ref()?.png.clone(),
            request,
            choice: self.choice?,
        })
    }
    pub fn saving(&mut self) {
        self.pending_save = Some((self.owner.clone(), self.generation));
    }
    pub fn finish(&mut self, owner: &str, ticket: u64, applied: bool, unknown: bool) -> bool {
        if self
            .pending_save
            .as_ref()
            .is_none_or(|(id, t)| id != owner || *t != ticket)
        {
            return false;
        }
        self.pending_save = None;
        if applied || unknown {
            self.reset();
        }
        true
    }
    fn accept_preview(
        &mut self,
        ctx: &egui::Context,
        ticket: u64,
        choice: Choice,
        result: Result<Rendered, String>,
    ) {
        if ticket != self.generation || self.choice != Some(choice) {
            return;
        }
        match result {
            Ok(rendered) => {
                self.prepared = Some(Prepared {
                    choice,
                    texture: texture(ctx, "icon-preview", rendered.pixels),
                    png: Arc::new(rendered.png),
                });
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn pump(&mut self, ctx: &egui::Context) {
        if self.worker.is_none() && self.open && !self.failed {
            let mut renderer = Renderer::default();
            let worker = Worker::spawn(move |job| match job {
                RenderJob::Preview(ticket, choice) => {
                    RenderReply::Preview(ticket, choice, renderer.render(choice))
                }
                RenderJob::Thumbnails(ids) => RenderReply::Thumbnails(
                    ids.into_iter()
                        .map(|id| (id, renderer.thumbnail(id)))
                        .collect(),
                ),
            });
            let wake = ctx.clone();
            worker.set_waker(move || wake.request_repaint());
            self.worker = Some(worker);
        }
        let event = self.worker.as_mut().map(Worker::poll);
        match event {
            Some(Polled::Ready(RenderReply::Preview(ticket, choice, result))) => {
                self.accept_preview(ctx, ticket, choice, result)
            }
            Some(Polled::Ready(RenderReply::Thumbnails(items))) => {
                for (id, result) in items {
                    match result {
                        Ok(pixels) => {
                            self.thumbnails
                                .insert(id, texture(ctx, &format!("icon-{id}"), pixels));
                        }
                        Err(error) => {
                            self.bad_thumbnails.insert(id);
                            self.error = Some(error);
                        }
                    }
                }
            }
            Some(Polled::Failed { .. } | Polled::Closed { .. }) => {
                self.failed = true;
                self.worker = None;
                self.error = Some("icon rendering worker stopped".into());
            }
            _ => {}
        }
        let Some(worker) = self.worker.as_mut() else {
            return;
        };
        if worker.busy() || !self.open {
            return;
        }
        if let Some(choice) = self.choice
            && self.submitted != Some(self.generation)
            && !self.prepared.as_ref().is_some_and(|p| p.choice == choice)
        {
            if worker
                .submit(RenderJob::Preview(self.generation, choice))
                .is_ok()
            {
                self.submitted = Some(self.generation);
            }
        } else {
            let ids: Vec<_> = self
                .wanted_thumbnails
                .iter()
                .copied()
                .filter(|id| !self.thumbnails.contains_key(id) && !self.bad_thumbnails.contains(id))
                .take(8)
                .collect();
            if !ids.is_empty() {
                let _ = worker.submit(RenderJob::Thumbnails(ids));
            }
        }
    }
    fn panel(
        &mut self,
        ui: &mut egui::Ui,
        look: &Look,
        language: Language,
        can_save: bool,
    ) -> bool {
        ui.heading(tr(language, "Icon library"));
        ui.label(look.muted_text("Phosphor"));
        ui.add_space(4.);
        // The clear button is placed first, from the right, and the field takes exactly what
        // is left: a row wider than the column would make egui widen it on every frame.
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!self.query.is_empty(), egui::Button::new("×"))
                    .on_hover_text(tr(language, "Clear search"))
                    .clicked()
                {
                    self.query.clear();
                }
                let width = ui.available_width();
                ui.add(
                    look.field(&mut self.query, tr(language, "Search icons…"))
                        .desired_width(width),
                );
            });
        });
        let mut save = false;
        let pending = self.pending_save.is_some();
        egui::Panel::bottom("icon-library-controls")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                ui.separator();
                let mut choice = self.choice.unwrap_or_default();
                ui.add_enabled_ui(self.choice.is_some() && !pending, |ui| {
                    let previous = choice;
                    color_row(
                        ui,
                        look,
                        language,
                        "Icon color",
                        &mut choice.foreground,
                        [235, 237, 240],
                    );
                    color_row(
                        ui,
                        look,
                        language,
                        "Background",
                        &mut choice.background,
                        [235, 237, 240],
                    );
                    ui.label(look.muted_text(tr(language, "Size")));
                    ui.add(
                        egui::Slider::new(&mut choice.scale, SCALES)
                            .step_by(f64::from(SCALE_STEP))
                            .suffix("%"),
                    );
                    if choice != previous {
                        self.choose(choice);
                    }
                });
                if let Some(choice) = self.choice {
                    ui.label(look.muted_text(tr(language, ENTRIES[choice.icon].label)));
                } else {
                    ui.label(look.muted_text(tr(language, "Choose an icon")));
                }
                if let Some(error) = self.error.clone() {
                    ui.colored_label(look.warn, tr(language, "Could not draw the icon"));
                    ui.collapsing(tr(language, "View details"), |ui| {
                        ui.label(error);
                    });
                    if ui.button(tr(language, "Try again")).clicked() {
                        self.failed = false;
                        self.error = None;
                        self.bad_thumbnails.clear();
                        self.invalidate();
                    }
                }
                ui.add_space(4.);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!pending, egui::Button::new(tr(language, "Cancel")))
                        .clicked()
                    {
                        self.cancel();
                    }
                    let changed = self.choice != self.recalled;
                    save = ui
                        .add_enabled(
                            can_save && changed && self.ready(),
                            look.primary(tr(language, "Save")),
                        )
                        .clicked();
                    if pending || (self.choice.is_some() && !self.ready() && self.error.is_none()) {
                        ui.spinner();
                    }
                });
            });
        self.wanted_thumbnails.clear();
        let mut found = false;
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("icon-library-grid")
            .auto_shrink(false);
        if self.scroll_query.as_deref() != Some(self.query.as_str()) {
            self.scroll_query = Some(self.query.clone());
            scroll = scroll.vertical_scroll_offset(0.);
        }
        scroll.show(ui, |ui| {
            let columns = ((ui.available_width() + 8.) / 56.).floor().max(1.) as usize;
            let tile = ((ui.available_width() - (columns - 1) as f32 * 8.) / columns as f32)
                .clamp(36., 64.);
            for group in GROUPS {
                let matching: Vec<_> = ENTRIES
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.group == *group && e.matches(&self.query))
                    .collect();
                if matching.is_empty() {
                    continue;
                }
                found = true;
                ui.add_space(8.);
                ui.label(look.section(tr(language, group)));
                egui::Grid::new(("icon-group", group))
                    .spacing([8., 8.])
                    .show(ui, |ui| {
                        for (position, (id, entry)) in matching.iter().enumerate() {
                            let selected = self.choice.is_some_and(|c| c.icon == *id);
                            let (rect, response) = ui
                                .allocate_exact_size(egui::Vec2::splat(tile), egui::Sense::click());
                            let name = tr(language, entry.label);
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::Button,
                                    !pending,
                                    selected,
                                    name,
                                )
                            });
                            if ui.is_rect_visible(rect) {
                                self.wanted_thumbnails.insert(*id);
                                let choice = self.choice.unwrap_or_default();
                                ui.painter().rect_filled(rect, 6., rgb(choice.background));
                                ui.painter().rect_stroke(
                                    rect,
                                    6.,
                                    Stroke::new(
                                        if selected { 2. } else { 1. },
                                        if selected {
                                            look.accent
                                        } else if response.hovered() {
                                            look.muted
                                        } else {
                                            look.line
                                        },
                                    ),
                                    StrokeKind::Inside,
                                );
                                if let Some(texture) = self.thumbnails.get(id) {
                                    egui::Image::new((texture.id(), rect.size()))
                                        .tint(rgb(choice.foreground))
                                        .paint_at(ui, rect.shrink(3.));
                                } else {
                                    ui.painter().circle_filled(rect.center(), 2., look.muted);
                                }
                            }
                            if selected && self.reveal {
                                response.scroll_to_me(Some(egui::Align::Center));
                                self.reveal = false;
                            }
                            if response.on_hover_text(name).clicked() && !pending {
                                self.choose(Choice {
                                    icon: *id,
                                    ..self.choice.unwrap_or_default()
                                });
                            }
                            if (position + 1) % columns == 0 {
                                ui.end_row();
                            }
                        }
                    });
            }
            if !found {
                ui.label(look.muted_text(tr(language, "No matching icons")));
            }
        });
        if !self.failed
            && (self
                .wanted_thumbnails
                .iter()
                .any(|id| !self.thumbnails.contains_key(id) && !self.bad_thumbnails.contains(id))
                || (self.choice.is_some()
                    && self.submitted != Some(self.generation)
                    && !self.ready()))
        {
            ui.ctx().request_repaint();
        }
        save
    }
}
fn texture(ctx: &egui::Context, name: &str, pixels: Pixels) -> egui::TextureHandle {
    ctx.load_texture(
        name,
        egui::ColorImage::from_rgba_unmultiplied(pixels.size, &pixels.rgba),
        Default::default(),
    )
}
fn rgb([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}
pub(super) fn color_row(
    ui: &mut egui::Ui,
    look: &Look,
    language: Language,
    label: &str,
    color: &mut [u8; 3],
    first: [u8; 3],
) {
    const SWATCHES: [[u8; 3]; 6] = [
        [235, 237, 240],
        [24, 26, 31],
        [74, 117, 161],
        [61, 128, 112],
        [179, 139, 71],
        [155, 98, 110],
    ];
    ui.push_id(label, |ui| {
        ui.label(look.muted_text(tr(language, label)));
        ui.horizontal(|ui| {
            for (index, swatch) in SWATCHES.into_iter().enumerate() {
                let value = if index == 0 { first } else { swatch };
                let text = format!("#{:02X}{:02X}{:02X}", value[0], value[1], value[2]);
                let response = ui.add(
                    egui::Button::new(" ")
                        .min_size(egui::vec2(20., 22.))
                        .fill(rgb(value))
                        .stroke(Stroke::new(
                            if *color == value { 2. } else { 1. },
                            if *color == value {
                                look.accent
                            } else {
                                look.line
                            },
                        )),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        *color == value,
                        &text,
                    )
                });
                if response.on_hover_text(text).clicked() {
                    *color = value;
                }
            }
            ui.color_edit_button_srgb(color)
                .on_hover_text(tr(language, "Custom color"));
        });
    });
}
impl App {
    pub(super) fn abandon_appearance(&mut self) {
        self.discard_imports();
        self.image_target = None;
        self.appearance_retry = None;
        self.icon_library.reset();
        self.draft = super::Draft::default();
        self.inspection = None;
    }
    pub(super) fn queue_image_cleanup(&mut self, tokens: impl IntoIterator<Item = String>) {
        if self.discarded_imports.is_empty() {
            self.image_cleanup_due = std::time::Instant::now();
        }
        for token in tokens {
            if !self.discarded_imports.contains(&token) {
                if self.discarded_imports.len() < crate::icon_save::MAX_PREPARED_IMAGES {
                    self.discarded_imports.push(token);
                } else {
                    self.image_cleanup_disconnect = true;
                    self.image_cleanup_due = std::time::Instant::now();
                }
            }
        }
    }
    pub(super) fn discard_imports(&mut self) {
        let tokens: Vec<_> = self
            .prepared_imports
            .drain(..)
            .map(|(token, _)| token)
            .collect();
        self.queue_image_cleanup(tokens);
    }
    pub(super) fn discard_unused_imports(&mut self) {
        let states = self.draft.state_faces();
        let mut unused = vec![];
        self.prepared_imports.retain(|(token, image)| {
            let used = self.draft.image == *image
                || states
                    .values()
                    .any(|face| face.image.as_ref() == Some(image));
            if !used {
                unused.push(token.clone());
            }
            used
        });
        self.queue_image_cleanup(unused);
    }
    pub(super) fn image_cleanup_result(&mut self, result: crate::icon_save::Cleanup) {
        self.discarded_imports
            .retain(|token| !result.released.contains(token));
        self.queue_image_cleanup(result.retry.iter().cloned());
        if result.disconnect {
            // Only unpublished imported digests depend on this connection. Recipes
            // can regenerate; published pictures and all locally edited text survive.
            let images: Vec<_> = self
                .prepared_imports
                .drain(..)
                .map(|(_, image)| image)
                .collect();
            if images.contains(&self.draft.image) {
                self.draft.image = self.draft.published_image.clone();
            }
            for (id, face) in &mut self.draft.states {
                if face
                    .image
                    .as_ref()
                    .is_some_and(|image| images.contains(image))
                {
                    face.image = self
                        .inspection
                        .as_ref()
                        .and_then(|view| view.appearance["states"][id]["image"].as_str())
                        .map(str::to_owned);
                }
            }
            self.image_target = None;
            self.transfer_active = false;
            self.discarded_imports.clear();
            self.image_cleanup_disconnect = false;
            self.appearance_retry = self.inspection.clone();
            self.reconnect_needed = true;
            self.warn(tr(self.es, "The image could not be discarded. Your text is still here; choose the image again."));
        }
        if result.retry.is_empty() || result.disconnect {
            self.image_cleanup_attempt = 0;
        } else {
            self.image_cleanup_attempt = self.image_cleanup_attempt.saturating_add(1);
            self.image_cleanup_due = std::time::Instant::now()
                + std::time::Duration::from_millis(100 << self.image_cleanup_attempt.min(3));
        }
    }
    pub(super) fn reread_appearance(&mut self) {
        self.appearance_retry = self.inspection.clone();
        self.force = true;
        self.inspect_needed = self.selected().is_some();
        self.warn(tr(self.es, "The key changed. Checking its appearance…"));
    }
    pub(super) fn adopt_inspection(&mut self, view: crate::session::Inspection) {
        self.inspect_needed = false;
        if self.inspection.as_ref().is_some_and(|old| {
            old.button == view.button
                && old.appearance == view.appearance
                && old.actions == view.actions
        }) {
            self.appearance_retry = None;
            return;
        }
        let mut fresh = super::Draft::load(&view);
        let same_visual = fresh.id == self.draft.id && fresh.visual_type == self.draft.visual_type;
        if !same_visual {
            self.discard_imports();
            self.icon_library.reset();
            self.image_target = None;
        }
        if same_visual
            && fresh.visual_type.is_some()
            && let Some(state) = &self.draft.state
        {
            fresh.select_state(state.clone());
            fresh.state_initialized = self.draft.state_initialized;
        }
        let retry = self.appearance_retry.take();
        if let Some(before) = &retry {
            if same_visual {
                fresh.retain_appearance_edits(before, &self.draft);
                self.warn(tr(
                    self.es,
                    "The key changed. Your appearance changes are still here. Save again.",
                ));
            } else {
                self.warn(tr(self.es, "The appearance changed and your edit was not saved. Check the key and try again."));
            }
        }
        if retry.is_some()
            || !self.icon_library.owns_draft(&fresh.id)
            || self.draft.action_ids != fresh.action_ids
            || self.draft.published_image != fresh.published_image
            || (fresh.state.is_none() && !view.appearance["visual_type"].is_null())
        {
            if retry.is_none() || !same_visual {
                self.icon_library.reset();
            }
            self.draft = fresh;
        } else if let Some(before) = &self.inspection {
            // The recipe still owns this face, not every field of the old key.
            // Rebase untouched fields before advancing the publication revision.
            fresh.retain_appearance_edits(before, &self.draft);
            self.draft = fresh;
        }
        self.inspection = Some(view);
        self.inspect_needed = false;
    }
    pub(super) fn icon_library_panel(&mut self, ui: &mut egui::Ui, look: &Look, editable: bool) {
        let can_save = editable
            && !self.transfer_active
            && self
                .inspection
                .as_ref()
                .is_some_and(|view| view.button["id"].as_str() == Some(self.draft.id.as_str()));
        if self.icon_library.panel(ui, look, self.es, can_save) {
            self.save();
        }
    }
    pub(super) fn icon_saved(
        &mut self,
        button: &str,
        ticket: u64,
        result: Result<rad_client::response::Response, crate::session::Error>,
    ) {
        let applied = result
            .as_ref()
            .is_ok_and(|r| r.status == rad_client::response::Status::Applied);
        let unknown = result.as_ref().is_err_and(|e| e.unknown);
        let matched = self.icon_library.finish(button, ticket, applied, unknown);
        if matched && (applied || unknown) {
            // Do not offer Save with the pre-upload image while the new view is loading.
            self.inspection = None;
        }
        match result {
            Ok(_) if applied => self.say(tr(self.es, "Saved")),
            Ok(_) => self.warn(tr(self.es, "Not confirmed — draft retained")),
            Err(error) => {
                if error.code == "recovery_required" {
                    self.enter_recovery();
                    return;
                }
                if matched && error.code == "cursor_expired" && !error.unknown {
                    self.reread_appearance();
                } else {
                    self.warn(crate::status::describe_failure(&error, self.es));
                }
                self.message_code = error.cause.as_ref().map_or_else(
                    || error.code.clone(),
                    |cause| format!("{}: {cause}", error.code),
                );
                if unknown {
                    self.resolving_unknown = true;
                }
                if !self.connected {
                    self.reconnect_needed = true;
                }
            }
        }
        self.force = true;
        self.inspect_needed = self.selected().is_some();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    fn app() -> App {
        let root = std::env::temp_dir().join(format!("rad-icons-test-{}", std::process::id()));
        let mut app = App::with_preferences_path(root.clone(), root.join("theme"), None);
        app.adopt_inspection(crate::session::Inspection {
            button: json!({"id":"b7","view_revision":"1","fields":{"name":"Volume","mode":"sequential","gesture":"press"}}),
            appearance: json!({"text":"","image":null,"text_position":"bottom"}), actions: vec![],
        });
        app
    }
    /// Draws the keys screen once and returns the width of the right column.
    fn side_panel_width(ctx: &egui::Context, app: &mut App, look: &Look) -> f32 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1180., 780.),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.keys_screen(ui, look, true));
        output.textures_delta.clear();
        egui::containers::panel::PanelState::load(ctx, egui::Id::new("actions"))
            .expect("the right column was drawn")
            .size()
            .x
    }
    #[test]
    fn open_library_keeps_the_side_panel_width_and_hands_it_back_to_the_palette() {
        let ctx = egui::Context::default();
        let look = Look::new(&Default::default());
        look.apply(&ctx);
        let mut app = app();
        app.connected = true;
        let palette = side_panel_width(&ctx, &mut app, &look);
        app.icon_library.begin("b7", None);
        for frame in 1..=50 {
            assert_eq!(
                side_panel_width(&ctx, &mut app, &look),
                palette,
                "the library widened the column on frame {frame}"
            );
        }
        app.icon_library.cancel();
        assert_eq!(side_panel_width(&ctx, &mut app, &look), palette);
    }
    #[test]
    fn opening_library_and_refreshed_inspection_preserve_unrelated_draft_edits() {
        let mut app = app();
        app.draft.text = "Local text".into();
        app.icon_library.begin("b7", None);
        assert!(app.icon_library.preview("b7").is_none());
        assert!(!app.icon_library.dirty_for("b7"));
        app.adopt_inspection(app.inspection.clone().unwrap());
        assert!(
            app.icon_library.open,
            "a refresh must not dismiss the library"
        );
        app.icon_library.cancel();
        assert_eq!(app.draft.text, "Local text");
    }
    fn prepare(library: &mut Library, ctx: &egui::Context, choice: Choice) {
        library.choose(choice);
        library.accept_preview(
            ctx,
            library.generation,
            choice,
            Renderer::default().render(choice),
        );
    }
    #[test]
    fn refreshed_other_face_and_base_survive_a_library_save_with_the_new_revision() {
        use crate::{
            native::{Event, Job, Reply},
            session::{Board, Grid, KeyView, Refresh},
        };
        let ctx = egui::Context::default();
        let mut app = app();
        app.connected = true;
        app.control.select(Some("b7".into()));
        let mut old = app.inspection.clone().unwrap();
        old.appearance = json!({"text":"Base", "image":null, "visual_type":"org.rad.audio#microphone",
            "states":{"muted":{"text":"Quiet","image":null},"unmuted":{"text":"Live","image":null}}});
        app.adopt_inspection(old.clone());
        let make_board = |view: &crate::session::Inspection| {
            let mut slots = vec![Value::Null; 32];
            slots[0] = json!({"visibility":"visible","id":"b7"});
            Board {
                grid: Grid::XL,
                folder: json!({"id":"f1","view_revision":view.button["view_revision"],"fields":{"name":"Main","parent_id":null,"slots":slots}}),
                buttons: vec![KeyView {
                    entity: view.button.clone(),
                    appearance: view.appearance.clone(),
                }],
            }
        };
        app.board = Some(make_board(&old));
        app.draft.select_state("muted".into());
        app.draft.text = "My quiet label".into();
        app.icon_library.begin("b7", None);
        prepare(&mut app.icon_library, &ctx, Choice::default());
        let recipe = app.icon_library.generation;
        let mut fresh = old;
        fresh.button["view_revision"] = json!("2");
        fresh.appearance["states"]["unmuted"] =
            json!({"text":"Remote live", "image":"remote-face"});
        fresh.appearance["text"] = json!("Remote base");
        fresh.appearance["image"] = json!("remote-base");
        let fresh_board = make_board(&fresh);
        let (sent, received) = std::sync::mpsc::channel();
        app.worker = Worker::spawn(move |job| {
            let result = if matches!(job, Job::Refresh { .. }) {
                Ok(Reply::Refresh(Refresh {
                    status: Value::Null,
                    board: Some(fresh_board.clone()),
                    inspection: Some(fresh.clone()),
                }))
            } else {
                sent.send(job).unwrap();
                Ok(Reply::Theme)
            };
            Event {
                paste: false,
                plugin_apply: false,
                plugin_admin: false,
                plugin_manifest: false,
                plugin_dependencies: false,
                plugin_form: false,
                plugin_visual: false,
                background: false,
                connected: true,
                transfer_job: false,
                palette: Default::default(),
                result,
            }
        });
        app.force = true;
        app.pump_reads();
        while app.worker.busy() {
            let _ = app.pump_replies(&ctx);
            std::thread::yield_now();
        }
        assert_eq!(app.icon_library.generation, recipe);
        assert!(app.icon_library.ready());
        app.save();
        let Job::SaveIcon { request, state, .. } = received
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
        else {
            panic!("library save");
        };
        assert_eq!(state.as_deref(), Some("muted"));
        let args = request.arguments();
        assert_eq!(args["expected_revision"], "2");
        let appearance = args["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|change| change["field"] == "appearance")
            .unwrap()["value"]
            .clone();
        assert_eq!(
            appearance["states"]["unmuted"],
            json!({"text":"Remote live","image":"remote-face"})
        );
        assert_eq!(appearance["states"]["muted"]["text"], "My quiet label");
        assert_eq!(appearance["text"], "Remote base");
        assert_eq!(appearance["image"], "remote-base");
    }
    #[test]
    fn cancel_switch_and_stale_render_never_change_the_next_key() {
        let ctx = egui::Context::default();
        let mut library = Library::default();
        library.begin("b7", None);
        prepare(&mut library, &ctx, Choice::default());
        let old = library.generation;
        library.cancel();
        assert!(!library.dirty_for("b7"));
        assert!(library.preview("b7").is_none());
        library.begin("b9", None);
        let choice = Choice {
            icon: 8,
            foreground: [180, 130, 60],
            ..Default::default()
        };
        prepare(&mut library, &ctx, choice);
        let current = library.preview("b9").unwrap().id();
        library.accept_preview(
            &ctx,
            old,
            Choice::default(),
            Renderer::default().render(Choice::default()),
        );
        assert_eq!(library.preview("b9").unwrap().id(), current);
        assert!(library.preview("b7").is_none());
        library.reset(); // same invalidation used on disconnect and disappearance
        library.accept_preview(
            &ctx,
            old,
            Choice::default(),
            Renderer::default().render(Choice::default()),
        );
        assert!(library.preview("b9").is_none());
    }
    #[test]
    fn save_captures_a_but_late_reply_does_not_clear_b() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.draft.text = "A draft".into();
        app.icon_library.begin("b7", None);
        prepare(&mut app.icon_library, &ctx, Choice::default());
        let (sent, received) = std::sync::mpsc::channel();
        app.worker = Worker::spawn(move |job| {
            sent.send(job).unwrap();
            crate::native::Event {
                paste: false,
                plugin_apply: false,
                plugin_admin: false,
                plugin_manifest: false,
                plugin_dependencies: false,
                plugin_form: false,
                plugin_visual: false,
                background: false,
                transfer_job: false,
                connected: true,
                palette: Default::default(),
                result: Ok(crate::native::Reply::Theme),
            }
        });
        assert!(
            received.try_recv().is_err(),
            "choosing and coloring send no RPC"
        );
        let inspection = app.inspection.take();
        app.save();
        assert!(!app.worker.busy(), "Save waits for a coherent inspection");
        app.inspection = inspection;
        app.save();
        let job = received
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let crate::native::Job::SaveIcon {
            button,
            ticket,
            request,
            png,
            ..
        } = job
        else {
            panic!("captured save")
        };
        assert_eq!(button, "b7");
        assert_eq!(
            request.arguments()["expected_revision"],
            "1",
            "library upload must not publish into a changed visual"
        );
        assert!(request.arguments().to_string().contains("A draft"));
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        app.select(1, Some("b9".into()));
        let mut view = app
            .inspection
            .clone()
            .unwrap_or_else(|| crate::session::Inspection {
                button: json!({"id":"b9","fields":{"name":"Other","mode":"sequential"}}),
                appearance: json!({"image":null,"text":""}),
                actions: vec![],
            });
        view.button["id"] = json!("b9");
        app.adopt_inspection(view.clone());
        app.draft.text = "B local".into();
        app.icon_library.begin("b9", None);
        prepare(
            &mut app.icon_library,
            &ctx,
            Choice {
                icon: 1,
                ..Default::default()
            },
        );
        app.icon_saved(
            &button,
            ticket,
            Ok(rad_client::response::Response {
                v: 1,
                id: Some("1".into()),
                session_id: None,
                status: rad_api::Status::Applied,
                data: rad_api::ResponseData::Applied(rad_api::Applied {
                    changed: true,
                    revision: "2".into(),
                }),
                error: None,
            }),
        );
        assert!(
            app.inspection.is_some(),
            "A confirmation must not invalidate B inspection"
        );
        app.adopt_inspection(view);
        assert_eq!(app.draft.text, "B local");
        assert!(app.icon_library.dirty_for("b9"));
        assert!(
            received.try_recv().is_err(),
            "late reply never replays the captured edit"
        );
    }
    #[test]
    fn refused_save_retains_local_image_unknown_discards_it_without_replay() {
        let ctx = egui::Context::default();
        let mut library = Library::default();
        library.begin("b7", None);
        prepare(&mut library, &ctx, Choice::default());
        let ticket = library.generation;
        library.saving();
        assert!(!library.ready());
        assert!(library.finish("b7", ticket, false, false));
        assert!(library.ready());
        library.saving();
        assert!(library.finish("b7", ticket, false, true));
        assert!(!library.dirty_for("b7"));
        assert!(!library.ready());
    }
    #[test]
    fn changed_published_image_or_visual_invalidates_the_local_choice() {
        let ctx = egui::Context::default();
        for external in [
            json!({"image":"new-published-image","text":""}),
            json!({"image":null,"text":"","visual_type":"plugin:status"}),
        ] {
            let mut app = app();
            app.icon_library.begin("b7", None);
            prepare(&mut app.icon_library, &ctx, Choice::default());
            let old = app.icon_library.generation;
            let mut view = app.inspection.clone().unwrap();
            view.appearance = external;
            app.adopt_inspection(view);
            assert!(
                !app.icon_library.open,
                "new published appearance invalidates local context"
            );
            app.icon_library.accept_preview(
                &ctx,
                old,
                Choice::default(),
                Renderer::default().render(Choice::default()),
            );
            assert!(app.icon_library.preview("b7").is_none());
        }
    }
    #[test]
    fn confirmed_or_unknown_icon_save_cannot_resubmit_the_old_image_before_refresh() {
        let ctx = egui::Context::default();
        for unknown in [false, true] {
            let mut app = app();
            app.draft.text = "Saved text".into();
            app.icon_library.begin("b7", None);
            prepare(&mut app.icon_library, &ctx, Choice::default());
            app.icon_library.saving();
            let ticket = app.icon_library.generation;
            let response = if unknown {
                Err(crate::session::Error {
                    unknown: true,
                    ..crate::session::Error::protocol()
                })
            } else {
                Ok(rad_client::response::Response {
                    v: 1,
                    id: Some("1".into()),
                    session_id: None,
                    status: rad_api::Status::Applied,
                    data: rad_api::ResponseData::Applied(rad_api::Applied {
                        changed: true,
                        revision: "2".into(),
                    }),
                    error: None,
                })
            };
            app.icon_saved("b7", ticket, response);
            assert!(
                app.inspection.is_none(),
                "old inspection cannot offer another Save with the pre-upload image"
            );
            assert!(!app.unsaved());
        }
    }
    #[test]
    fn leaving_keys_or_entering_recovery_invalidates_only_local_icon_context() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.connected = true;
        app.draft.text = "Other local fields".into();
        app.icon_library.begin("b7", None);
        prepare(&mut app.icon_library, &ctx, Choice::default());
        app.navigation.open(crate::screens::Section::General);
        app.pump_auxiliary(&ctx);
        assert!(
            !app.icon_library.open,
            "settings must not retain a competing Save panel"
        );
        assert_eq!(app.draft.text, "Other local fields");
        app.icon_library.begin("b7", None);
        prepare(&mut app.icon_library, &ctx, Choice::default());
        app.icon_library.saving();
        let ticket = app.icon_library.generation;
        app.icon_saved(
            "b7",
            ticket,
            Err(crate::session::Error {
                code: "recovery_required".into(),
                ..crate::session::Error::protocol()
            }),
        );
        assert!(!app.recovery.is_normal());
        assert!(!app.force, "recovery must not schedule normal refreshes");
        assert!(!app.icon_library.open);
    }
    #[test]
    fn changing_visual_with_the_same_state_ids_discards_the_old_library_recipe() {
        let ctx = egui::Context::default();
        let mut app = app();
        let mut view = app.inspection.clone().unwrap();
        view.appearance["visual_type"] = json!("org.rad.audio#microphone");
        view.appearance["states"] = json!({"muted":{"text":"Old","image":null}});
        app.adopt_inspection(view.clone());
        app.draft.select_state("muted".into());
        app.icon_library.begin("b7", None);
        prepare(&mut app.icon_library, &ctx, Choice::default());
        let generation = app.icon_library.generation;
        view.appearance["visual_type"] = json!("org.rad.obs#mute");
        view.appearance["states"] = json!({});
        app.adopt_inspection(view);
        app.icon_library.accept_preview(
            &ctx,
            generation,
            Choice::default(),
            Renderer::default().render(Choice::default()),
        );
        assert!(!app.icon_library.open);
        assert!(!app.icon_library.dirty_for("b7"));
        assert!(app.draft.state.is_none());
        assert!(app.draft.states.is_empty());
        app.draft.select_state("muted".into());
        assert!(app.draft.image.is_empty());
        assert!(app.draft.text.is_empty());
        let mut manual = app.inspection.clone().unwrap();
        manual.appearance["visual_type"] = serde_json::Value::Null;
        app.adopt_inspection(manual);
        app.draft.text = "Manual library".into();
        app.icon_library.begin("b7", None);
        prepare(&mut app.icon_library, &ctx, Choice::default());
        let (sent, received) = std::sync::mpsc::channel();
        app.worker = Worker::spawn(move |job| {
            sent.send(job).unwrap();
            crate::native::Event {
                paste: false,
                plugin_apply: false,
                plugin_admin: false,
                plugin_manifest: false,
                plugin_dependencies: false,
                plugin_form: false,
                plugin_visual: false,
                background: false,
                transfer_job: false,
                connected: true,
                palette: Default::default(),
                result: Ok(crate::native::Reply::Theme),
            }
        });
        app.save();
        let crate::native::Job::SaveIcon { state, request, .. } = received
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
        else {
            panic!()
        };
        assert!(
            state.is_none(),
            "new library choice targets the visible manual face"
        );
        let args = request.arguments();
        let appearance = args["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["field"] == "appearance")
            .unwrap();
        assert_eq!(appearance["value"]["text"], "Manual library");
        assert_eq!(appearance["value"]["states"], json!({}));
    }
}
