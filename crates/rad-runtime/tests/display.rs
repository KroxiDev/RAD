use rad_authority::{Authority, Command, Effect};
use rad_core::{configuration::*, identity::IdentityRegistry};
use rad_deck::{Desired, KeyVisual, TextPlacement};
use rad_runtime::display::project;
use std::{collections::BTreeMap, num::NonZeroU128};
#[test]
fn physical_projection_follows_active_folder_without_running_selection() {
    let first = Configuration::prepare_new(&IdentityRegistry::new(877).unwrap()).unwrap();
    let root = first.configuration().data().principal;
    let configured = first
        .configuration()
        .prepare_edit(
            first.registry(),
            Origin::Editor,
            Edit::CreateFolder {
                parent: root,
                position: 2,
                name: "Tools".into(),
            },
        )
        .unwrap();
    let mut actor = Authority::new(configured, 1, NonZeroU128::new(1).unwrap());
    let mut expected = std::array::from_fn(|_| KeyVisual::default());
    // Text on a key without image is centred unless a position was chosen for it.
    expected[2] = KeyVisual::new("Tools", None, false, false).text_at(TextPlacement::Middle);
    assert_eq!(
        project(&actor, &BTreeMap::new(), 42, false),
        Desired::new(expected, 42, false).unwrap()
    );
    let folder = actor.status(Origin::Editor).active.unwrap();
    let entry = actor.folder(Origin::Editor, folder.short()).unwrap().slots[2]
        .clone()
        .unwrap();
    let rad_authority::Visible::Visible(entry) = entry else {
        panic!()
    };
    let ticket = actor.reserve(1, Origin::Editor).unwrap();
    let effects = actor.submit(ticket, Command::RunId { id: entry.short() });
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::SelectionChanged { .. }))
    );
    assert!(!effects.iter().any(|e| matches!(e, Effect::Dispatch(_))));
    let mut expected = std::array::from_fn(|_| KeyVisual::default());
    expected[0] = KeyVisual::new("Return", None, false, false).text_at(TextPlacement::Middle);
    assert_eq!(
        project(&actor, &BTreeMap::new(), 42, true),
        Desired::new(expected, 42, true).unwrap()
    );
}

#[test]
fn physical_projection_paints_text_where_the_key_places_it() {
    let first = Configuration::prepare_new(&IdentityRegistry::new(877).unwrap()).unwrap();
    let root = first.configuration().data().principal;
    let created = first
        .configuration()
        .prepare_edit(
            first.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: root,
                position: 5,
                name: "Mute".into(),
            },
        )
        .unwrap();
    let button = created.configuration().data().buttons[0].binding.identity();
    let placed = created
        .configuration()
        .prepare_edit(
            created.registry(),
            Origin::Editor,
            Edit::SetAppearance {
                button,
                appearance: Appearance {
                    text: "Live".into(),
                    image: None,
                    text_position: Some(TextPosition::Top),
                },
            },
        )
        .unwrap();
    let actor = Authority::new(placed, 1, NonZeroU128::new(1).unwrap());
    let mut expected = std::array::from_fn(|_| KeyVisual::default());
    expected[5] = KeyVisual::new("Live", None, false, false).text_at(TextPlacement::Top);
    assert_eq!(
        project(&actor, &BTreeMap::new(), 42, false),
        Desired::new(expected, 42, false).unwrap()
    );
}

#[test]
fn physical_projection_shows_an_image_alone_unless_the_key_has_text_of_its_own() {
    let icon = std::sync::Arc::new(rad_media::normalize(PNG).unwrap());
    let project_with = |text: &str, text_position| {
        let first = Configuration::prepare_new(&IdentityRegistry::new(877).unwrap()).unwrap();
        let root = first.configuration().data().principal;
        let created = first
            .configuration()
            .prepare_edit(
                first.registry(),
                Origin::Editor,
                Edit::CreateButton {
                    folder: root,
                    position: 0,
                    name: "Steam".into(),
                },
            )
            .unwrap();
        let button = created.configuration().data().buttons[0].binding.identity();
        let dressed = created
            .configuration()
            .prepare_edit(
                created.registry(),
                Origin::Editor,
                Edit::SetAppearance {
                    button,
                    appearance: Appearance {
                        text: text.into(),
                        image: Some("icon".into()),
                        text_position,
                    },
                },
            )
            .unwrap();
        let actor = Authority::new(dressed, 1, NonZeroU128::new(1).unwrap());
        let images = BTreeMap::from([("icon".to_owned(), icon.clone())]);
        project(&actor, &images, 42, false)
    };
    let expect = |key: KeyVisual| {
        let mut keys = std::array::from_fn(|_| KeyVisual::default());
        keys[0] = key;
        Desired::new(keys, 42, false).unwrap()
    };
    // The name stands in for missing text only on a key that has no image.
    assert_eq!(
        project_with("", None),
        expect(KeyVisual::new("", Some(icon.clone()), false, false))
    );
    assert_eq!(
        project_with("Play", None),
        expect(
            KeyVisual::new("Play", Some(icon.clone()), false, false).text_at(TextPlacement::Bottom)
        )
    );
    assert_eq!(
        project_with("Play", Some(TextPosition::AboveImage)),
        expect(
            KeyVisual::new("Play", Some(icon.clone()), false, false)
                .text_at(TextPlacement::AbovePicture)
        )
    );
}
/// A one-pixel PNG.
const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240, 31, 0,
    5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
