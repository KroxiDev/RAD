//! Right-hand list of what can be put on a key, grouped in categories that fold.
use super::{
    App, Drag, NewAction, SEARCH_DEBOUNCE_MS,
    look::{Icon, Look, RADIUS, paint_icon},
    tr,
};
use eframe::egui::{self, CornerRadius};

const ROW_HEIGHT: f32 = 30.;
const HEADER_HEIGHT: f32 = 24.;

/// A group of actions in the list.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Category {
    Navigation,
    Timing,
    Device,
    Plugins,
    Applications,
}

/// What a row shows before its name: a glyph of the font, or an icon drawn with strokes.
#[derive(Clone, Copy)]
enum Badge {
    Glyph(&'static str),
    Icon(Icon),
}

/// Category title with a chevron: pointing down while its rows show, right while folded.
fn header(ui: &mut egui::Ui, look: &Look, title: &str, open: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEADER_HEIGHT),
        egui::Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if ui.is_enabled() && response.hovered() {
            painter.rect_filled(rect, CornerRadius::same(RADIUS), look.hover);
        }
        painter.text(
            rect.left_center() + egui::vec2(4., 0.),
            egui::Align2::LEFT_CENTER,
            title.to_uppercase(),
            egui::FontId::proportional(11.),
            look.muted,
        );
        let at = |x: f32, y: f32| rect.right_center() + egui::vec2(x - 14., y);
        let chevron = if open {
            vec![at(-4., -2.), at(0., 2.), at(4., -2.)]
        } else {
            vec![at(-2., -4.), at(2., 0.), at(-2., 4.)]
        };
        painter.line(chevron, egui::Stroke::new(1.5, look.muted));
    }
    response
}

/// One draggable, clickable row: its badge, then its name.
fn row(ui: &mut egui::Ui, look: &Look, badge: Badge, name: &str, drag: Drag) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::click_and_drag(),
    );
    response.dnd_set_drag_payload(drag);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if ui.is_enabled() && response.hovered() {
            painter.rect_filled(rect, CornerRadius::same(RADIUS), look.hover);
        }
        let middle = rect.left_center();
        let square =
            egui::Rect::from_center_size(middle + egui::vec2(17., 0.), egui::Vec2::splat(22.));
        painter.rect_filled(square, CornerRadius::same(5), look.raised);
        // The badge stays quiet: the name is what the eye looks for.
        match badge {
            Badge::Glyph(glyph) => {
                painter.text(
                    square.center(),
                    egui::Align2::CENTER_CENTER,
                    glyph,
                    egui::FontId::proportional(13.),
                    look.muted,
                );
            }
            Badge::Icon(icon) => paint_icon(painter, square.center(), icon, look.muted),
        }
        let color = if ui.is_enabled() {
            look.text
        } else {
            look.muted
        };
        let mut job = egui::text::LayoutJob::simple_singleline(
            name.to_owned(),
            egui::FontId::proportional(14.),
            color,
        );
        job.wrap.max_width = rect.width() - 44.;
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = painter.layout_job(job);
        let at = middle + egui::vec2(38., -galley.size().y / 2.);
        painter.galley(at, galley, color);
    }
    response
}

impl App {
    /// Draws a category title and folds or unfolds it on click. Returns whether it is open.
    fn category(&mut self, ui: &mut egui::Ui, look: &Look, category: Category) -> bool {
        let title = match category {
            Category::Navigation => tr(self.es, "Navigation"),
            Category::Timing => tr(self.es, "Timing"),
            Category::Device => tr(self.es, "Stream Deck"),
            Category::Plugins => tr(self.es, "Plugins"),
            Category::Applications => tr(self.es, "Applications"),
        };
        let open = !self.folded.contains(&category);
        let hint = if open {
            tr(self.es, "Hide")
        } else {
            tr(self.es, "Show")
        };
        if header(ui, look, title, open).on_hover_text(hint).clicked() {
            if open {
                self.folded.insert(category);
            } else {
                self.folded.remove(&category);
            }
        }
        open
    }
    pub(super) fn actions(&mut self, ui: &mut egui::Ui, look: &Look) {
        ui.label(egui::RichText::new(tr(self.es, "Actions")).heading());
        let hint = tr(self.es, "Search applications");
        let search = ui.add(
            look.field(&mut self.catalog.query, hint)
                .margin(egui::Margin {
                    left: 30,
                    right: 8,
                    top: 5,
                    bottom: 5,
                })
                .desired_width(f32::INFINITY),
        );
        paint_icon(
            ui.painter(),
            search.rect.left_center() + egui::vec2(15., 0.),
            Icon::Search,
            look.muted,
        );
        if search.changed() {
            self.catalog_due = Some(self.now_ms() + SEARCH_DEBOUNCE_MS);
            if let Some(wake) = &self.search_wake {
                wake.schedule(std::time::Duration::from_millis(SEARCH_DEBOUNCE_MS));
            }
        }
        ui.add_space(4.);
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.;
                if self.category(ui, look, Category::Navigation) {
                    self.navigation_rows(ui, look);
                }
                ui.add_space(6.);
                if self.category(ui, look, Category::Timing) {
                    self.list_action(ui, look, Badge::Glyph("⏱"), NewAction::Wait);
                }
                ui.add_space(6.);
                if self.category(ui, look, Category::Device) {
                    self.list_action(ui, look, Badge::Icon(Icon::Moon), NewAction::Sleep);
                }
                self.plugin_rows(ui, look);
                ui.add_space(6.);
                if self.category(ui, look, Category::Applications) {
                    self.application_rows(ui, look);
                }
            });
    }
    /// The actions installed plugins bring, under a category of their own that only shows
    /// when there is something in it.
    fn plugin_rows(&mut self, ui: &mut egui::Ui, look: &Look) {
        let entries: Vec<_> = self
            .plugins
            .catalog
            .iter()
            .filter(|e| !e.visual && e.ready)
            .cloned()
            .collect();
        if entries.is_empty() || !self.connected {
            return;
        }
        ui.add_space(6.);
        if !self.category(ui, look, Category::Plugins) {
            return;
        }
        // While a plugin form is being opened or saved, another one cannot start.
        let usable = !self.plugins.form_pending && !self.plugins.form_loading;
        ui.add_enabled_ui(usable, |ui| {
            for entry in entries {
                self.list_action(ui, look, Badge::Icon(Icon::Plug), NewAction::Plugin(entry));
            }
        });
    }
    /// A row that puts `action` on a key: dragged there, or double-clicked with one selected.
    fn list_action(&mut self, ui: &mut egui::Ui, look: &Look, badge: Badge, action: NewAction) {
        let label = self.action_name(&action);
        let drag = Drag::Add {
            action: action.clone(),
            label: label.clone(),
        };
        let response = row(ui, look, badge, &label, drag);
        if response.double_clicked()
            && let Some(position) = self.selected_slot
        {
            self.place_action(position, &action);
        } else if response.clicked() {
            self.say(tr(
                self.es,
                "Drag it onto a key, or double-click to add it to the selected key",
            ));
        }
    }
    fn navigation_rows(&mut self, ui: &mut egui::Ui, look: &Look) {
        let folder = row(
            ui,
            look,
            Badge::Glyph("🗀"),
            tr(self.es, "Folder"),
            Drag::Folder,
        );
        if folder.double_clicked()
            && let (Some(position), None) = (self.selected_slot, self.selected())
        {
            self.create(position, true);
        } else if folder.clicked() {
            self.say(tr(
                self.es,
                "Drag the folder onto an empty key, or double-click with one selected",
            ));
        }
    }
    fn application_rows(&mut self, ui: &mut egui::Ui, look: &Look) {
        if self.catalog.entries.is_empty() {
            ui.add_space(4.);
            ui.label(look.muted_text(tr(self.es, "No applications found")));
        }
        for entry in self.catalog.entries.clone() {
            self.list_action(
                ui,
                look,
                Badge::Icon(Icon::Apps),
                NewAction::Launcher(entry),
            );
        }
        if self.catalog.next.is_some() {
            ui.add_space(6.);
            if ui.button(tr(self.es, "More…")).clicked() {
                self.send(self.catalog.request(true));
            }
        }
    }
}
