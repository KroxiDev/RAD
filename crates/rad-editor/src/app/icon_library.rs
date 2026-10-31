//! Contextual library in the actions column. Raster work stays off the GUI thread.
use super::{App, look::Look, tr};
use crate::{
    icon_library::{Choice, ENTRIES, GROUPS, Rendered, Renderer},
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
    pub fn begin(&mut self, owner: &str) {
        if self.owner != owner {
            self.reset();
            self.owner = owner.into();
        }
        if !self.open {
            self.before = self.prepared.clone();
            self.query.clear();
            self.scroll_query = None;
            self.open = true;
        }
    }
    pub fn cancel(&mut self) {
        if self.pending_save.is_some() {
            return;
        }
        self.prepared = self.before.take();
        self.choice = self.prepared.as_ref().map(|p| p.choice);
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
        self.owner == owner && self.choice.is_some()
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
            button: self.owner.clone(),
            ticket: self.generation,
            png: self.prepared.as_ref()?.png.clone(),
            request,
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
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 32.).max(40.);
            ui.add(
                look.field(&mut self.query, tr(language, "Search icons…"))
                    .desired_width(width),
            );
            if ui
                .add_enabled(!self.query.is_empty(), egui::Button::new("×"))
                .on_hover_text(tr(language, "Clear search"))
                .clicked()
            {
                self.query.clear();
            }
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
                    color_row(ui, look, language, "Icon color", &mut choice.foreground);
                    color_row(ui, look, language, "Background", &mut choice.background);
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
                    save = ui
                        .add_enabled(can_save && self.ready(), look.primary(tr(language, "Save")))
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
fn color_row(ui: &mut egui::Ui, look: &Look, language: Language, label: &str, color: &mut [u8; 3]) {
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
            for value in SWATCHES {
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
    pub(super) fn adopt_inspection(&mut self, view: crate::session::Inspection) {
        let fresh = super::Draft::load(&view);
        if !self.icon_library.owns_draft(&fresh.id)
            || self.draft.action_ids != fresh.action_ids
            || self.draft.published_image != fresh.published_image
            || !view.appearance["visual_type"].is_null()
        {
            self.icon_library.reset();
            self.draft = fresh;
        }
        self.inspection = Some(view);
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
                self.warn(crate::status::describe_failure(&error, self.es));
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
    use serde_json::json;
    fn app() -> App {
        let root = std::env::temp_dir().join(format!("rad-icons-test-{}", std::process::id()));
        let mut app = App::with_preferences_path(root.clone(), root.join("theme"), None);
        app.adopt_inspection(crate::session::Inspection {
            button: json!({"id":"b7","fields":{"name":"Volume","mode":"sequential","gesture":"press"}}),
            appearance: json!({"text":"","image":null,"text_position":"bottom"}), actions: vec![],
        });
        app
    }
    #[test]
    fn opening_library_and_refreshed_inspection_preserve_unrelated_draft_edits() {
        let mut app = app();
        app.draft.text = "Local text".into();
        app.icon_library.begin("b7");
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
    fn cancel_switch_and_stale_render_never_change_the_next_key() {
        let ctx = egui::Context::default();
        let mut library = Library::default();
        library.begin("b7");
        prepare(&mut library, &ctx, Choice::default());
        let old = library.generation;
        library.cancel();
        assert!(!library.dirty_for("b7"));
        assert!(library.preview("b7").is_none());
        library.begin("b9");
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
        app.icon_library.begin("b7");
        prepare(&mut app.icon_library, &ctx, Choice::default());
        let (sent, received) = std::sync::mpsc::channel();
        app.worker = Worker::spawn(move |job| {
            sent.send(job).unwrap();
            crate::native::Event {
                plugin_apply: false,
                plugin_admin: false,
                plugin_manifest: false,
                plugin_dependencies: false,
                plugin_form: false,
                plugin_visual: false,
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
        } = job
        else {
            panic!("captured save")
        };
        assert_eq!(button, "b7");
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
        app.icon_library.begin("b9");
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
        library.begin("b7");
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
            app.icon_library.begin("b7");
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
            app.icon_library.begin("b7");
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
        app.icon_library.begin("b7");
        prepare(&mut app.icon_library, &ctx, Choice::default());
        app.navigation.open(crate::screens::Section::General);
        app.pump_auxiliary(&ctx);
        assert!(
            !app.icon_library.open,
            "settings must not retain a competing Save panel"
        );
        assert_eq!(app.draft.text, "Other local fields");
        app.icon_library.begin("b7");
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
}
