//! The runtime-described key grid. A click selects; a double click runs the key, exactly like Try.
use super::{
    App, Drag,
    look::{KEY_RADIUS, Look},
    string,
};
use eframe::egui::{self, CornerRadius, Stroke, StrokeKind};

use crate::session::Grid;
const GAP: f32 = 12.;
const KEY_RANGE: std::ops::RangeInclusive<f32> = 52.0..=104.;
/// Room between the keys and the edge of the deck body drawn around them.
const BEZEL: f32 = 14.;
const MARGIN: f32 = BEZEL + 12.;
fn height_for(key: f32, grid: Grid) -> f32 {
    key * grid.rows() as f32 + GAP * grid.rows().saturating_sub(1) as f32 + 2. * MARGIN
}
pub(super) fn min_height(grid: Grid) -> f32 {
    height_for(*KEY_RANGE.start(), grid)
}
pub(super) fn comfortable_height(grid: Grid) -> f32 {
    height_for(84., grid)
}

/// What a key reads: its own text, or its name while it has neither text nor image.
/// The runtime applies the same rule when it paints the deck.
pub(super) fn key_label(text: &str, name: &str, has_image: bool) -> String {
    let label = if !text.is_empty() {
        text
    } else if has_image {
        ""
    } else {
        name
    };
    label.chars().take(40).collect()
}

pub(super) fn native_icon(appearance: &serde_json::Value) -> Option<rad_api::NativeIcon> {
    match appearance["native_icon"].as_str() {
        Some("sleep") => Some(rad_api::NativeIcon::Sleep),
        Some("brightness") => Some(rad_api::NativeIcon::Brightness),
        _ => None,
    }
}

/// Paints one key face the way the deck does. Text at the top, in the middle or at the
/// bottom lies over the picture, which is dimmed behind it; text above or below the
/// picture gets a band of its own and the picture shrinks to make room. The picture comes
/// with how far it has faded in, from 0 to 1.
pub(super) fn paint_key(
    ui: &egui::Ui,
    look: &Look,
    rect: egui::Rect,
    label: &str,
    text_position: &str,
    picture: Option<(&egui::TextureHandle, f32)>,
    native_icon: Option<rad_api::NativeIcon>,
) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let radius = CornerRadius::same(KEY_RADIUS);
    painter.rect_filled(rect, radius, look.key);
    // Same proportions as the 96 px key the deck paints.
    let unit = rect.height() / 96.;
    let area = match (label.is_empty(), text_position) {
        (false, "above_image") => rect.with_min_y(rect.top() + 34. * unit),
        (false, "below_image") => rect.with_max_y(rect.top() + 62. * unit),
        _ => rect,
    };
    if let Some((texture, shown)) = picture {
        let size = texture.size_vec2();
        let fitted = size * (area.width() / size.x).min(area.height() / size.y);
        egui::Image::from_texture((texture.id(), fitted))
            .corner_radius(radius)
            .tint(egui::Color32::WHITE.gamma_multiply(shown))
            .paint_at(ui, egui::Rect::from_center_size(area.center(), fitted));
    }
    if picture.is_none() {
        match native_icon {
            Some(rad_api::NativeIcon::Sleep) => {
                super::look::paint_moon(&painter, area.center(), 22. * unit, look.text);
            }
            Some(rad_api::NativeIcon::Brightness) => {
                super::look::paint_sun(&painter, area.center(), 22. * unit, look.text);
            }
            None => {}
        }
    }
    if label.is_empty() {
        return;
    }
    let font = egui::FontId::proportional(if rect.width() < 70. { 11. } else { 12.5 });
    let mut job =
        egui::text::LayoutJob::simple(label.to_owned(), font, look.text, rect.width() - 6.);
    job.halign = egui::Align::Center;
    job.wrap.max_rows = 2;
    job.wrap.overflow_character = Some('…');
    let galley = painter.layout_job(job);
    let height = galley.size().y;
    // A small key has less room than two rows need: keep them inside it.
    let fit = |top: f32| top.min(rect.bottom() - height - 2.).max(rect.top());
    // Where the text starts and, when it lies over the picture, the rows dimmed behind it.
    let (top, band) = match text_position {
        "top" => {
            let top = fit(rect.top() + 2. * unit);
            (top, Some((rect.top(), top + height + 3.)))
        }
        "bottom" => {
            let top = fit(rect.bottom() - height - 3. * unit);
            (top, Some((top - 3., rect.bottom())))
        }
        "above_image" => (fit(rect.top() + 2. * unit), None),
        "below_image" => (fit(rect.top() + 64. * unit), None),
        _ => {
            let top = rect.center().y - height / 2.;
            (top, Some((top - 3., top + height + 3.)))
        }
    };
    // The band dims what it lies over, so it comes in with the picture.
    let behind = picture.map_or(native_icon.map(|_| 1.), |(_, shown)| Some(shown));
    if let (Some((from, to)), Some(behind)) = (band, behind) {
        let band =
            egui::Rect::from_min_max(egui::pos2(rect.left(), from), egui::pos2(rect.right(), to));
        // Square corners would poke out of the key where the band reaches its edge.
        let corners = CornerRadius {
            nw: if from <= rect.top() { KEY_RADIUS } else { 0 },
            ne: if from <= rect.top() { KEY_RADIUS } else { 0 },
            sw: if to >= rect.bottom() { KEY_RADIUS } else { 0 },
            se: if to >= rect.bottom() { KEY_RADIUS } else { 0 },
        };
        painter.rect_filled(
            band,
            corners,
            egui::Color32::from_black_alpha((153. * behind) as u8),
        );
    }
    painter.galley(egui::pos2(rect.center().x, top), galley, look.text);
}

/// Follows the pointer with the name of whatever is being dragged.
pub(super) fn drag_ghost(ctx: &egui::Context, look: &Look) {
    let (Some(payload), Some(pointer)) = (
        egui::DragAndDrop::payload::<Drag>(ctx),
        ctx.pointer_interact_pos(),
    ) else {
        return;
    };
    let label = match &*payload {
        Drag::Button { label, .. } => label.clone(),
        Drag::Add { label, .. } => label.clone(),
        Drag::Folder => "🗀".into(),
        Drag::Reorder(_) => return,
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("drag-ghost"),
    ));
    let galley = painter.layout_no_wrap(label, egui::FontId::proportional(13.), look.on_accent);
    let rect = egui::Rect::from_min_size(
        pointer + egui::vec2(12., 12.),
        galley.size() + egui::vec2(16., 10.),
    );
    painter.rect_filled(rect, CornerRadius::same(KEY_RADIUS), look.accent);
    painter.galley(rect.min + egui::vec2(8., 5.), galley, look.on_accent);
}

impl App {
    pub(super) fn deck(&mut self, ui: &mut egui::Ui, look: &Look) {
        let Some(model) = self.board.as_ref().map(|board| board.grid) else {
            return;
        };
        let columns = model.columns();
        let rows = model.rows();
        // A tiled window can be narrower than the grid; every key must stay reachable.
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            let available = ui.available_size();
            let fit = |space: f32, count: usize| {
                (space - 2. * MARGIN - GAP * (count - 1) as f32) / count as f32
            };
            let key = fit(available.x, columns)
                .min(fit(available.y, rows))
                .clamp(*KEY_RANGE.start(), *KEY_RANGE.end())
                .floor();
            let grid = egui::vec2(
                key * columns as f32 + GAP * (columns - 1) as f32,
                key * rows as f32 + GAP * (rows - 1) as f32,
            );
            let area = (grid + egui::Vec2::splat(2. * MARGIN)).max(available);
            let (rect, _) = ui.allocate_exact_size(area, egui::Sense::hover());
            let origin = rect.min + (area - grid) / 2.;
            // The keys sit in a body like the hardware's, so the grid reads as the device.
            let body = egui::Rect::from_min_size(origin, grid).expand(BEZEL);
            ui.painter().rect(
                body,
                CornerRadius::same(KEY_RADIUS + 8),
                look.bezel,
                Stroke::new(1., look.line),
                StrokeKind::Inside,
            );
            for position in 0..columns * rows {
                let cell = egui::vec2(
                    (position % columns) as f32 * (key + GAP),
                    (position / columns) as f32 * (key + GAP),
                );
                let key_rect = egui::Rect::from_min_size(origin + cell, egui::Vec2::splat(key));
                self.key(ui, look, position, key_rect);
            }
        });
        if self.pictures.fading(ui.input(|i| i.time)) {
            ui.ctx().request_repaint();
        }
    }
    fn key(&mut self, ui: &mut egui::Ui, look: &Look, position: usize, rect: egui::Rect) {
        let id = self.slot_id(position);
        let view = id.as_ref().and_then(|id| {
            self.board
                .as_ref()?
                .buttons
                .iter()
                .find(|b| b.entity["id"] == id.as_str())
        });
        let label = view
            .map(|b| {
                key_label(
                    &string(&b.appearance["text"]),
                    &string(&b.entity["fields"]["name"]),
                    b.appearance["image"].is_string() || native_icon(&b.appearance).is_some(),
                )
            })
            .unwrap_or_default();
        let name = view.map(|b| string(&b.entity["fields"]["name"]));
        let response = ui.interact(
            rect,
            ui.id().with(("key", position)),
            egui::Sense::click_and_drag(),
        );
        if let Some(id) = &id {
            response.dnd_set_drag_payload(Drag::Button {
                id: id.clone(),
                label: label.clone(),
            });
        }
        let accepts_drop = response
            .dnd_hover_payload::<Drag>()
            .is_some_and(|drag| match &*drag {
                Drag::Button { id: source, .. } => Some(source) != id.as_ref(),
                Drag::Add { .. } => true,
                Drag::Folder => id.is_none(),
                Drag::Reorder(_) => false,
            });
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            let radius = CornerRadius::same(KEY_RADIUS);
            if let Some(view) = view {
                let now = ui.input(|i| i.time);
                let picture = view.appearance["image"]
                    .as_str()
                    .and_then(|reference| self.pictures.get(reference, now));
                let position = string(&view.appearance["text_position"]);
                if !self.plugin_face(ui, look, id.as_deref().unwrap_or(""), rect) {
                    paint_key(
                        ui,
                        look,
                        rect,
                        &label,
                        &position,
                        picture,
                        native_icon(&view.appearance),
                    );
                }
                if response.hovered() {
                    painter.rect_filled(rect, radius, egui::Color32::from_white_alpha(10));
                }
            } else {
                painter.rect_filled(rect, radius, look.empty_key);
                if response.hovered() || accepts_drop {
                    painter.rect_filled(rect, radius, look.raised);
                    let c = rect.center();
                    let stroke = Stroke::new(1.5, look.muted);
                    painter.line_segment([c - egui::vec2(6., 0.), c + egui::vec2(6., 0.)], stroke);
                    painter.line_segment([c - egui::vec2(0., 6.), c + egui::vec2(0., 6.)], stroke);
                }
            }
            if accepts_drop || self.selected_slot == Some(position) {
                painter.rect_stroke(
                    rect,
                    radius,
                    Stroke::new(2., look.accent),
                    StrokeKind::Outside,
                );
            }
        }
        if response.double_clicked() && id.is_some() {
            if self.selected() != id {
                self.select(position, id.clone());
            }
            self.try_selected();
        } else if response.clicked() {
            self.select(position, id.clone());
        }
        if let Some(drag) = response.dnd_release_payload::<Drag>() {
            match &*drag {
                Drag::Button { id: source, .. } => self.send(crate::Request::Edit {
                    changes: vec![rad_api::Change::Move {
                        id: source.clone(),
                        position,
                    }],
                }),
                Drag::Folder => {
                    if id.is_none() {
                        self.create(position, true);
                    }
                }
                Drag::Add { action, .. } => self.place_action(position, action),
                Drag::Reorder(_) => {}
            }
        }
        if let Some(name) = name.filter(|name| !name.is_empty()) {
            response.on_hover_text(name);
        }
    }
}
