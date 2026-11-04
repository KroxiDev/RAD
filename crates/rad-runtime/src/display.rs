//! A render projection only. Authority owns navigation, input and execution.
use rad_authority::{ActionType, Authority, EntityView, Visible, VisualView};
use rad_core::configuration::{Origin, TextPosition};
use rad_deck::{Desired, KeyVisual, TextPlacement};
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
/// What the deck shows now. A picture still loading leaves its key plain; only one that
/// could not be loaded marks the key unavailable.
pub(crate) fn project(
    actor: &Authority,
    pictures: &crate::pictures::Pictures,
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
            .as_deref()
            .and_then(|key| pictures.get(key));
        let external = matches!(view.visual, VisualView::External(_));
        let plugin = if external {
            visuals.map(|v| v.resolved(actor, binding.short()))
        } else {
            None
        };
        if external {
            let custom = plugin
                .as_ref()
                .and_then(|p| p.view.state.as_ref())
                .and_then(|state| view.appearance.states.get(state))
                .and_then(|face| face.image.as_deref())
                .or_else(|| {
                    plugin
                        .as_ref()
                        .filter(|p| p.view.state.is_none())
                        .and(view.appearance.image.as_deref())
                });
            image = custom
                .and_then(|key| pictures.get(key))
                .or_else(|| visuals.and_then(|v| v.image(binding.short())));
        }
        let unavailable = plugin.as_ref().is_some_and(|v| {
            v.view.availability == crate::plugin_visuals::Availability::Unavailable
        }) || (external && plugin.is_none())
            || (!external
                && view
                    .appearance
                    .image
                    .as_deref()
                    .is_some_and(|key| pictures.failed(key)))
            || button
                .actions
                .iter()
                .any(|a| matches!(a,Visible::Visible(a) if !a.executor_installed));
        let launcher = button
            .actions
            .iter()
            .any(|a| matches!(a,Visible::Visible(a) if a.kind==ActionType::Launcher));
        let native_sleep = !external
            && view.appearance.image.is_none()
            && button
                .actions
                .iter()
                .any(|a| matches!(a,Visible::Visible(a) if a.kind==ActionType::Sleep));
        let native_brightness = !native_sleep
            && !external
            && view.appearance.image.is_none()
            && button.actions.iter().any(
                |a| matches!(a,Visible::Visible(a) if matches!(a.kind,ActionType::Brightness(_))),
            );
        let native_picture = native_sleep || native_brightness;
        let native_text = view.appearance.text_style.resolve(
            &view.appearance.text,
            view.appearance.image.is_some() || native_picture,
            None,
            Some(&button.name),
        );
        let text = plugin
            .as_ref()
            .map(|v| v.text.as_str())
            .unwrap_or(native_text.text);
        let stale = plugin
            .as_ref()
            .is_some_and(|v| v.view.availability == crate::plugin_visuals::Availability::Stale);
        let color = plugin.as_ref().and_then(|v| v.view.color);
        let position = plugin
            .as_ref()
            .map(|v| v.position)
            .unwrap_or(native_text.position);
        let size = plugin.as_ref().map(|v| v.size).unwrap_or(native_text.size);
        let text_color = plugin
            .as_ref()
            .map(|v| v.text_color)
            .unwrap_or(native_text.color);
        let band = plugin.as_ref().map(|v| v.band).unwrap_or(native_text.band);
        let placement = match position {
            TextPosition::Top => TextPlacement::Top,
            TextPosition::Middle => TextPlacement::Middle,
            TextPosition::Bottom | TextPosition::Hidden => TextPlacement::Bottom,
            TextPosition::AboveImage => TextPlacement::AbovePicture,
            TextPosition::BelowImage => TextPlacement::BelowPicture,
        };
        KeyVisual::new(text, image, unavailable, launcher)
            .sleep_icon(native_sleep)
            .brightness_icon(native_brightness)
            .text_at(placement)
            .text_style(size, text_color, band)
            .stale(stale)
            .background_color(color)
    });
    Desired::new(keys, brightness, asleep).expect("validated brightness")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rad_core::{configuration::*, identity::IdentityRegistry};

    #[test]
    fn native_icons_resolve_and_project_the_complete_style() {
        for kind in [
            ActionKind::Sleep,
            ActionKind::Brightness(BrightnessDirection::Increase),
        ] {
            let c = Configuration::prepare_new(&IdentityRegistry::new(93).unwrap()).unwrap();
            let c = c
                .configuration()
                .prepare_edit(
                    c.registry(),
                    Origin::Editor,
                    Edit::CreateButton {
                        folder: c.configuration().data().principal,
                        position: 0,
                        name: "Name".into(),
                    },
                )
                .unwrap();
            let button = c.configuration().data().buttons[0].binding.identity();
            let sleep = matches!(kind, ActionKind::Sleep);
            let c = c
                .configuration()
                .prepare_edits(
                    c.registry(),
                    Origin::Editor,
                    &[
                        Edit::AddAction { button, kind },
                        Edit::SetAppearance {
                            button,
                            appearance: Appearance {
                                text: "Written".into(),
                                text_style: TextStyle {
                                    position: Some(TextPosition::Hidden),
                                    size: 24,
                                    color: [10, 20, 30],
                                    band: false,
                                },
                                ..Default::default()
                            },
                        },
                    ],
                )
                .unwrap();
            let actor = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
            let expected = Desired::new(
                std::array::from_fn(|i| {
                    if i == 0 {
                        KeyVisual::new("", None, false, false)
                            .sleep_icon(sleep)
                            .brightness_icon(!sleep)
                            .text_at(TextPlacement::Bottom)
                            .text_style(24, [10, 20, 30], false)
                    } else {
                        KeyVisual::default()
                    }
                }),
                55,
                false,
            )
            .unwrap();
            assert_eq!(
                project(&actor, &crate::pictures::Pictures::new(0), 55, false, None),
                expected
            );
        }
    }
}
