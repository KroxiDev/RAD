// Critical live-entity ceiling; synthetic minimal valid folder tree.
use rad_core::{configuration::*, identity::*};
fn populated(count: usize) -> (Configuration, IdentityRegistry) {
    let mut c = Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap();
    for position in 0..16 {
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::CreateFolder {
                    parent: c.configuration().data().principal,
                    position,
                    name: String::new(),
                },
            )
            .unwrap();
    }
    let mut data = c.configuration().data().clone();
    let mut registry = c.registry().clone();
    let mut total = registry.records().count();
    for folder in &mut data.folders {
        for slot in &mut folder.slots {
            if slot.is_some() || total >= count {
                continue;
            }
            let binding = registry.allocate(EntityKind::Button).unwrap();
            total += 1;
            let mut actions = Vec::new();
            for _ in 0..MAX_ACTIONS.min(count - total) {
                actions.push(ActionData {
                    binding: registry.allocate(EntityKind::Action).unwrap(),
                    kind: ActionKind::Wait(Wait::new(0).unwrap()),
                    permissions: Default::default(),
                });
                total += 1;
            }
            *slot = Some(binding.identity());
            data.buttons.push(ButtonData {
                gesture: Gesture::Press,
                binding,
                name: String::new(),
                appearance: Default::default(),
                visual: VisualSource::Manual,
                permissions: Default::default(),
                mode: Default::default(),
                actions,
            });
        }
    }
    assert_eq!(total, count);
    (Configuration::from_data(data, &registry).unwrap(), registry)
}
#[test]
fn preparing_an_edit_enforces_the_live_entity_ceiling_without_mutation() {
    let (configuration, registry) = populated(7000);
    let before = registry.clone();
    let button = configuration
        .data()
        .buttons
        .last()
        .unwrap()
        .binding
        .identity();
    let result = configuration.prepare_edit(
        &registry,
        Origin::Editor,
        Edit::AddAction {
            button,
            kind: ActionKind::Wait(Wait::new(0).unwrap()),
        },
    );
    assert_eq!(result.unwrap_err(), ConfigurationError::ResourceExhausted);
    assert_eq!(registry, before);
}
