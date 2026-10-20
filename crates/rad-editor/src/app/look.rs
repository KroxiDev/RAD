//! Window look derived from the three palette colours; key images never depend on it.
use crate::theme::Palette;
use eframe::egui::{self, Color32, CornerRadius, Stroke};

pub(super) const RADIUS: u8 = 6;
pub(super) const KEY_RADIUS: u8 = 10;

#[derive(Clone)]
pub(super) struct Look {
    pub background: Color32,
    pub surface: Color32,
    pub field: Color32,
    pub raised: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub key: Color32,
    /// The body of the deck the keys sit in: darker than the window, like the hardware.
    pub bezel: Color32,
    /// A key with nothing on it.
    pub empty_key: Color32,
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    pub ok: Color32,
    pub warn: Color32,
}

/// `amount` of `towards` blended into `base`, per channel.
fn mix(base: [u8; 3], towards: [u8; 3], amount: f32) -> Color32 {
    let channel = |i: usize| {
        (f32::from(base[i]) + (f32::from(towards[i]) - f32::from(base[i])) * amount).round() as u8
    };
    Color32::from_rgb(channel(0), channel(1), channel(2))
}

impl Look {
    pub fn new(palette: &Palette) -> Self {
        let (bg, fg) = (palette.background, palette.foreground);
        let [r, g, b] = palette.accent;
        let accent_is_light =
            0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b) > 150.;
        Self {
            background: Color32::from_rgb(bg[0], bg[1], bg[2]),
            surface: mix(bg, fg, 0.045),
            field: mix(bg, [0, 0, 0], 0.3),
            raised: mix(bg, fg, 0.11),
            hover: mix(bg, fg, 0.17),
            pressed: mix(bg, fg, 0.24),
            key: mix(bg, fg, 0.13),
            bezel: mix(bg, [0, 0, 0], 0.38),
            empty_key: mix(bg, fg, 0.05),
            line: mix(bg, fg, 0.14),
            text: Color32::from_rgb(fg[0], fg[1], fg[2]),
            muted: mix(fg, bg, 0.45),
            accent: Color32::from_rgb(r, g, b),
            on_accent: if accent_is_light {
                Color32::from_rgb(16, 16, 20)
            } else {
                Color32::WHITE
            },
            danger: Color32::from_rgb(229, 83, 75),
            ok: Color32::from_rgb(63, 185, 80),
            warn: Color32::from_rgb(210, 153, 34),
        }
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let look = self.clone();
        ctx.all_styles_mut(move |style| {
            let radius = CornerRadius::same(RADIUS);
            let visuals = &mut style.visuals;
            *visuals = egui::Visuals::dark();
            visuals.panel_fill = look.background;
            visuals.window_fill = look.surface;
            visuals.window_stroke = Stroke::new(1., look.line);
            visuals.window_corner_radius = CornerRadius::same(KEY_RADIUS);
            visuals.extreme_bg_color = look.field;
            visuals.faint_bg_color = look.surface;
            visuals.override_text_color = Some(look.text);
            visuals.hyperlink_color = look.accent;
            visuals.selection.bg_fill = look.accent;
            visuals.selection.stroke = Stroke::new(1., look.on_accent);
            visuals.menu_corner_radius = CornerRadius::same(RADIUS + 2);
            let widgets = &mut visuals.widgets;
            for (state, fill, border) in [
                (&mut widgets.noninteractive, look.surface, look.line),
                (&mut widgets.inactive, look.raised, Color32::TRANSPARENT),
                (&mut widgets.hovered, look.hover, Color32::TRANSPARENT),
                (&mut widgets.active, look.pressed, Color32::TRANSPARENT),
                (&mut widgets.open, look.hover, Color32::TRANSPARENT),
            ] {
                state.bg_fill = fill;
                state.weak_bg_fill = fill;
                state.bg_stroke = Stroke::new(1., border);
                state.fg_stroke = Stroke::new(1., look.text);
                state.corner_radius = radius;
                state.expansion = 0.;
            }
            style.spacing.window_margin = egui::Margin::same(16);
            style.spacing.item_spacing = egui::vec2(8., 8.);
            style.spacing.button_padding = egui::vec2(10., 5.);
            style.spacing.interact_size.y = 28.;
            for (text_style, size) in [
                (egui::TextStyle::Small, 12.),
                (egui::TextStyle::Body, 14.),
                (egui::TextStyle::Button, 14.),
                (egui::TextStyle::Monospace, 13.),
                (egui::TextStyle::Heading, 17.),
            ] {
                if let Some(font) = style.text_styles.get_mut(&text_style) {
                    font.size = size;
                }
            }
        });
    }

    pub fn panel(&self, fill: Color32, margin: i8) -> egui::Frame {
        egui::Frame::new().fill(fill).inner_margin(margin)
    }
    /// Single-line text field with room to breathe; `hint` shows while it is empty.
    pub fn field<'a>(&self, text: &'a mut String, hint: &str) -> egui::TextEdit<'a> {
        egui::TextEdit::singleline(text)
            .hint_text(hint.to_owned())
            .margin(egui::vec2(8., 5.))
    }
    pub fn muted_text(&self, text: impl Into<String>) -> egui::RichText {
        egui::RichText::new(text).color(self.muted)
    }
    pub fn section(&self, text: &str) -> egui::RichText {
        egui::RichText::new(text.to_uppercase())
            .color(self.muted)
            .size(11.)
    }
    pub fn primary<'a>(&self, text: &str) -> egui::Button<'a> {
        egui::Button::new(egui::RichText::new(text).color(self.on_accent)).fill(self.accent)
    }
    pub fn dangerous<'a>(&self, text: &str) -> egui::Button<'a> {
        egui::Button::new(egui::RichText::new(text).color(Color32::WHITE)).fill(self.danger)
    }
    /// A raised block that groups one item, like an action of the selected key.
    pub fn card(&self) -> egui::Frame {
        egui::Frame::new()
            .fill(self.raised)
            .corner_radius(CornerRadius::same(RADIUS))
            .inner_margin(egui::Margin::symmetric(8, 3))
    }
}

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Back,
    Undo,
    Redo,
    Up,
    /// A circular arrow: ask again.
    Refresh,
    /// A magnifying glass: the field searches.
    Search,
    /// Four small squares: an application.
    Apps,
    /// A plug: an action a plugin brings.
    Plug,
    /// Two columns of dots: the row can be dragged.
    Grip,
    /// A thin cross: take this away.
    Close,
}

/// Draws `icon` around `center` with strokes, so it never depends on the fonts' glyphs.
pub(super) fn paint_icon(painter: &egui::Painter, center: egui::Pos2, icon: Icon, color: Color32) {
    let stroke = Stroke::new(1.7, color);
    let at = |x: f32, y: f32| center + egui::vec2(x, y);
    match icon {
        Icon::Back => {
            painter.line(vec![at(2.5, -5.5), at(-3., 0.), at(2.5, 5.5)], stroke);
        }
        Icon::Up => {
            painter.line(vec![at(-5., 2.5), at(0., -2.5), at(5., 2.5)], stroke);
        }
        Icon::Refresh => {
            // An arc around the centre with a gap on the right, and a head where it ends.
            let on_circle = |degrees: f32| {
                let angle = degrees.to_radians();
                (at(6. * angle.cos(), 6. * angle.sin()), angle)
            };
            painter.line(
                (0..=12)
                    .map(|step| on_circle(30. + 300. * step as f32 / 12.).0)
                    .collect(),
                stroke,
            );
            let (end, angle) = on_circle(330.);
            let along = egui::vec2(-angle.sin(), angle.cos());
            let across = egui::vec2(angle.cos(), angle.sin());
            let tip = end + along * 2.;
            painter.line(
                vec![
                    tip - along * 4. + across * 3.5,
                    tip,
                    tip - along * 4. - across * 3.5,
                ],
                stroke,
            );
        }
        Icon::Undo | Icon::Redo => {
            // Hooked arrow: head on the upper shaft, which turns back under itself.
            let side = if matches!(icon, Icon::Undo) { 1. } else { -1. };
            let at = |x: f32, y: f32| at(x * side, y);
            let mut hook = vec![at(-6.5, -2.5), at(2., -2.5)];
            hook.extend((1..=8).map(|step| {
                let angle =
                    -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * (step as f32 / 8.);
                at(2. + 3.75 * angle.cos(), 1.25 + 3.75 * angle.sin())
            }));
            hook.push(at(-3.5, 5.));
            painter.line(hook, stroke);
            painter.line(vec![at(-2.5, -6.5), at(-6.5, -2.5), at(-2.5, 1.5)], stroke);
        }
        Icon::Search => {
            let stroke = Stroke::new(1.5, color);
            painter.circle_stroke(at(-1.5, -1.5), 4.5, stroke);
            painter.line_segment([at(1.8, 1.8), at(5.5, 5.5)], stroke);
        }
        Icon::Apps => {
            let stroke = Stroke::new(1.3, color);
            for (x, y) in [(-3., -3.), (3., -3.), (-3., 3.), (3., 3.)] {
                let square = egui::Rect::from_center_size(at(x, y), egui::Vec2::splat(4.));
                painter.rect_stroke(
                    square,
                    CornerRadius::same(1),
                    stroke,
                    egui::StrokeKind::Middle,
                );
            }
        }
        Icon::Plug => {
            let stroke = Stroke::new(1.4, color);
            painter.line_segment([at(-2.5, -6.5), at(-2.5, -3.)], stroke);
            painter.line_segment([at(2.5, -6.5), at(2.5, -3.)], stroke);
            let body = egui::Rect::from_min_max(at(-4.5, -3.), at(4.5, 2.));
            painter.rect_stroke(
                body,
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.line_segment([at(0., 2.), at(0., 6.5)], stroke);
        }
        Icon::Close => {
            let stroke = Stroke::new(1.5, color);
            painter.line_segment([at(-4., -4.), at(4., 4.)], stroke);
            painter.line_segment([at(-4., 4.), at(4., -4.)], stroke);
        }
        Icon::Grip => {
            for (x, y) in [
                (-2.5, -4.),
                (2.5, -4.),
                (-2.5, 0.),
                (2.5, 0.),
                (-2.5, 4.),
                (2.5, 4.),
            ] {
                painter.circle_filled(at(x, y), 1.3, color);
            }
        }
    }
}

/// Square icon button drawn with strokes, independent of the bundled fonts' glyph coverage.
pub(super) fn icon_button(
    ui: &mut egui::Ui,
    look: &Look,
    icon: Icon,
    hint: &str,
) -> egui::Response {
    icon_button_with_id(ui, look, icon, hint, None)
}

/// [`icon_button`] whose widget id is `id` when given, for a control that must be found
/// again by something other than its place in the layout.
pub(super) fn icon_button_with_id(
    ui: &mut egui::Ui,
    look: &Look,
    icon: Icon,
    hint: &str,
    id: Option<egui::Id>,
) -> egui::Response {
    let size = egui::vec2(28., 28.);
    let (rect, response) = match id {
        None => ui.allocate_exact_size(size, egui::Sense::click()),
        Some(id) => {
            let (_, rect) = ui.allocate_space(size);
            (rect, ui.interact(rect, id, egui::Sense::click()))
        }
    };
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if ui.is_enabled() && response.hovered() {
            painter.rect_filled(rect, CornerRadius::same(RADIUS), look.hover);
        }
        let color = if ui.is_enabled() {
            look.text
        } else {
            look.muted
        };
        paint_icon(painter, rect.center(), icon, color);
    }
    response.on_hover_text(hint)
}

/// A button whose text ends in a down-pointing triangle: it opens more choices.
pub(super) fn menu_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.button(format!("{text}  ⏷"))
}

/// Quiet, borderless button made of a single glyph, for actions that sit next to content.
pub(super) fn glyph_button(
    ui: &mut egui::Ui,
    look: &Look,
    glyph: &str,
    hint: &str,
) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(glyph).size(16.).color(look.muted))
            .frame(false)
            .min_size(egui::vec2(24., 24.)),
    )
    .on_hover_text(hint)
}

/// Coloured dot followed by a quiet label: a state, never a control.
pub(super) fn state_pill(ui: &mut egui::Ui, look: &Look, dot: Color32, label: &str) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, look.muted);
    let size = egui::vec2(14. + galley.size().x, galley.size().y.max(16.));
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(egui::pos2(rect.left() + 4., rect.center().y), 4., dot);
    let text_at = egui::pos2(rect.left() + 14., rect.center().y - galley.size().y / 2.);
    painter.galley(text_at, galley, look.muted);
}
