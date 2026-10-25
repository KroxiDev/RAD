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
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Category {
    Native,
    Plugin(String),
    Applications,
}

/// What a row shows before its name: a glyph of the font, or an icon drawn with strokes.
#[derive(Clone, Copy)]
enum Badge {
    Glyph(&'static str),
    Icon(Icon),
}

/// Category title with a chevron: pointing down while its rows show, right while folded.
fn header(
    ui: &mut egui::Ui,
    look: &Look,
    title: &str,
    open: bool,
    foldable: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEADER_HEIGHT),
        if foldable {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if foldable && ui.is_enabled() && response.hovered() {
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
    fn matches_query(&self, label: &str) -> bool {
        label
            .to_lowercase()
            .contains(&self.catalog.query.trim().to_lowercase())
    }
    fn native_matches(&self, label: &str) -> bool {
        self.matches_query(label) || self.matches_query(tr(self.es, "RAD"))
    }
    fn action_matches(&self, action: &NewAction) -> bool {
        let label = self.action_name(action);
        match action {
            NewAction::Plugin(entry) => {
                self.matches_query(&label)
                    || crate::plugins::plugin_id(&entry.type_key)
                        .is_some_and(|id| self.matches_query(&self.plugin_group_name(id)))
            }
            NewAction::Launcher(_) => self.matches_query(&label),
            _ => self.native_matches(&label),
        }
    }
    /// Draws a category title and folds or unfolds it on click. Returns whether it is open.
    fn category(&mut self, ui: &mut egui::Ui, look: &Look, category: Category) -> bool {
        let title = match &category {
            Category::Native => tr(self.es, "RAD").to_owned(),
            Category::Plugin(id) => self.plugin_group_name(id),
            Category::Applications => tr(self.es, "Applications").to_owned(),
        };
        let searching = !self.catalog.query.trim().is_empty();
        let open = !self.folded.contains(&category) || searching;
        let response = header(ui, look, &title, open, !searching);
        if !searching
            && response
                .on_hover_text(if open {
                    tr(self.es, "Hide")
                } else {
                    tr(self.es, "Show")
                })
                .clicked()
        {
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
        let hint = tr(self.es, "Search actions");
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
            self.catalog.entries.clear();
            self.catalog.next = None;
            self.catalog_more = false;
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
                // None is the folder row; all other native entries become plan actions.
                let natives: Vec<_> = [
                    (Badge::Glyph("🗀"), None),
                    (Badge::Glyph("⏱"), Some(NewAction::Wait)),
                    (Badge::Icon(Icon::Moon), Some(NewAction::Sleep)),
                    (Badge::Icon(Icon::Sun), Some(NewAction::Brightness)),
                    (Badge::Icon(Icon::Apps), Some(NewAction::OpenApp)),
                ]
                .into_iter()
                .filter(|(_, action)| {
                    let label = action
                        .as_ref()
                        .map(|action| self.action_name(action))
                        .unwrap_or_else(|| tr(self.es, "Folder").to_owned());
                    self.native_matches(&label)
                })
                .collect();
                if !natives.is_empty() && self.category(ui, look, Category::Native) {
                    for (badge, action) in natives {
                        match action {
                            Some(action) => self.list_action(ui, look, badge, action),
                            None => self.navigation_rows(ui, look, badge),
                        }
                    }
                }
                self.plugin_rows(ui, look);
                ui.add_space(6.);
                if self.category(ui, look, Category::Applications) {
                    self.application_rows(ui, look);
                }
            });
    }
    fn plugin_group_name(&self, id: &str) -> String {
        let item = self.plugins.status["items"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["id"] == id))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"id": id}));
        super::plugins::plugin_name(&item, self.es)
    }
    /// Each installed plugin owns a separately collapsible group.
    fn plugin_rows(&mut self, ui: &mut egui::Ui, look: &Look) {
        if !self.connected {
            return;
        }
        let mut groups = std::collections::BTreeMap::<String, Vec<_>>::new();
        for entry in self.plugins.catalog.iter().filter(|entry| {
            entry.ready && self.action_matches(&NewAction::Plugin((*entry).clone()))
        }) {
            if let Some(id) = crate::plugins::plugin_id(&entry.type_key) {
                groups.entry(id.to_owned()).or_default().push(entry.clone());
            }
        }
        // While a plugin form is being opened or saved, another one cannot start.
        let usable = !self.plugins.form_pending && !self.plugins.form_loading;
        for (id, entries) in groups {
            ui.add_space(6.);
            if self.category(ui, look, Category::Plugin(id)) {
                ui.add_enabled_ui(usable, |ui| {
                    for entry in entries {
                        self.list_action(
                            ui,
                            look,
                            Badge::Icon(Icon::Plug),
                            NewAction::Plugin(entry),
                        );
                    }
                });
            }
        }
    }
    /// A row that puts `action` on a key: dragged there, or double-clicked with one selected.
    fn list_action(&mut self, ui: &mut egui::Ui, look: &Look, badge: Badge, action: NewAction) {
        if !self.action_matches(&action) {
            return;
        }
        let label = self.action_name(&action);
        let drag = Drag::Add {
            action: action.clone(),
            label: label.clone(),
        };
        let hint = if matches!(&action, NewAction::Plugin(entry) if entry.visual) {
            tr(
                self.es,
                "Visual — drag onto a key or double-click to change its appearance, not its actions",
            )
        } else {
            tr(
                self.es,
                "Drag it onto a key, or double-click to add it to the selected key",
            )
        };
        let response = row(ui, look, badge, &label, drag).on_hover_text(hint);
        if response.double_clicked()
            && let Some(position) = self.selected_slot
        {
            self.place_action(position, &action);
        } else if response.clicked() {
            self.say(hint);
        }
    }
    fn navigation_rows(&mut self, ui: &mut egui::Ui, look: &Look, badge: Badge) {
        let folder = row(ui, look, badge, tr(self.es, "Folder"), Drag::Folder);
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
        let loading = self.application_catalog_loading(super::launcher::CatalogTarget::Actions);
        if loading {
            ui.spinner();
        } else if self.catalog.entries.is_empty() {
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
        if self.catalog.next.is_some() && !loading {
            ui.add_space(6.);
            if ui.button(tr(self.es, "More…")).clicked() {
                self.catalog_more = true;
                self.catalog_due = Some(0);
            }
        }
    }
}

#[cfg(test)]
mod brightness_tests {
    use super::*;

    fn texts(app: &mut App) -> Vec<String> {
        egui::Context::default()
            .run_ui(Default::default(), |ui| {
                app.actions(ui, &Look::new(&Default::default()));
            })
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn brightness_belongs_to_the_native_rad_group_and_can_be_searched() {
        for (language, label) in [
            (crate::preferences::Language::En, "Brightness"),
            (crate::preferences::Language::Es, "Brillo"),
        ] {
            let root =
                std::env::temp_dir().join(format!("rad-brightness-palette-{}", std::process::id()));
            let mut app = App::with_preferences_path(root.clone(), root.join("theme"), None);
            app.es = language;
            assert!(texts(&mut app).iter().any(|text| text == label));
            app.folded.insert(Category::Native);
            assert!(!texts(&mut app).iter().any(|text| text == label));
            app.catalog.query = label.into();
            assert!(texts(&mut app).iter().any(|text| text == label));
        }
    }
}
