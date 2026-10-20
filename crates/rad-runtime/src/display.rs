//! A render projection only. Authority owns navigation, input and execution.
use rad_authority::{ActionType, Authority, EntityView, Visible, VisualView};
use rad_core::configuration::{Origin, TextPosition};
use rad_deck::{Desired, KeyVisual, TextPlacement};
use std::{collections::BTreeMap, sync::Arc};
pub trait DeckPort {
    fn next_event(&mut self) -> Option<rad_deck::Event>;
    fn set_desired(&mut self, desired: Desired) -> Result<(), rad_deck::DeckError>;
    fn shutdown(&mut self);
    fn is_stopped(&self) -> bool;
}
impl DeckPort for rad_deck::Deck {
    fn next_event(&mut self) -> Option<rad_deck::Event> {
        rad_deck::Deck::next_event(self)
    }
    fn set_desired(&mut self, desired: Desired) -> Result<(), rad_deck::DeckError> {
        rad_deck::Deck::set_desired(self, desired)
    }
    fn shutdown(&mut self) {
        rad_deck::Deck::shutdown(self)
    }
    fn is_stopped(&self) -> bool {
        rad_deck::Deck::is_stopped(self)
    }
}
pub fn project(
    actor: &Authority,
    images: &BTreeMap<String, Arc<rad_media::NormalizedImage>>,
    brightness: u8,
    asleep: bool,
) -> Desired {
    project_with_plugins(actor, images, brightness, asleep, None)
}
pub(crate) fn project_with_plugins(
    actor: &Authority,
    images: &BTreeMap<String, Arc<rad_media::NormalizedImage>>,
    brightness: u8,
    asleep: bool,
    visuals: Option<&crate::plugin_visuals::Visuals>,
) -> Desired {
    let folder = actor
        .status(Origin::Editor)
        .active
        .and_then(|b| actor.folder(Origin::Editor, b.short()));
    let keys = std::array::from_fn(|position| {
        let Some(Some(Visible::Visible(binding))) =
            folder.as_ref().and_then(|f| f.slots.get(position))
        else {
            return KeyVisual::default();
        };
        let Some(EntityView::Button(button)) = actor.get(Origin::Editor, binding.short()) else {
            return KeyVisual::default();
        };
        let Some(view) = actor.appearance(Origin::Editor, binding.short()) else {
            return KeyVisual::default();
        };
        let mut image = view
            .appearance
            .image
            .as_ref()
            .and_then(|key| images.get(key))
            .cloned();
        let external = matches!(view.visual, VisualView::External(_));
        let plugin = if external {
            visuals.map(|v| v.resolved(actor, binding.short()))
        } else {
            None
        };
        if external {
            image = visuals.and_then(|v| v.image(binding.short()));
        }
        let unavailable = plugin.as_ref().is_some_and(|v| {
            v.view.availability == crate::plugin_visuals::Availability::Unavailable
        }) || (external && plugin.is_none())
            || (!external && view.appearance.image.is_some() && image.is_none())
            || button
                .actions
                .iter()
                .any(|a| matches!(a,Visible::Visible(a) if !a.executor_installed));
        let launcher = button
            .actions
            .iter()
            .any(|a| matches!(a,Visible::Visible(a) if a.kind==ActionType::Launcher));
        let text = plugin
            .as_ref()
            .map(|v| v.text.as_str())
            .unwrap_or_else(|| view.appearance.label(&button.name));
        let stale = plugin
            .as_ref()
            .is_some_and(|v| v.view.availability == crate::plugin_visuals::Availability::Stale);
        let color = plugin.as_ref().and_then(|v| v.view.color);
        let position = plugin
            .as_ref()
            .map(|v| v.position)
            .unwrap_or_else(|| view.appearance.effective_text_position());
        let placement = match position {
            TextPosition::Top => TextPlacement::Top,
            TextPosition::Middle => TextPlacement::Middle,
            TextPosition::Bottom => TextPlacement::Bottom,
            TextPosition::AboveImage => TextPlacement::AbovePicture,
            TextPosition::BelowImage => TextPlacement::BelowPicture,
        };
        KeyVisual::new(text, image, unavailable, launcher)
            .text_at(placement)
            .stale(stale)
            .background_color(color)
    });
    Desired::new(keys, brightness, asleep).expect("validated brightness")
}
