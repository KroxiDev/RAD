use rad_authority::{Authority, Command, CommitId, Effect, Outcome};
use rad_core::{configuration::*, identity::IdentityRegistry};
fn persist(e: &[Effect]) -> CommitId {
    e.iter()
        .find_map(|e| match e {
            Effect::Persist { id, .. } => Some(*id),
            _ => None,
        })
        .unwrap()
}
fn brightness(n: u8) -> Command {
    vec![Edit::SetSettings(SettingsData {
        brightness_percent: n,
        ..SettingsData::default()
    })]
    .into()
}
#[test]
fn current_commit_maps_to_original_request_across_drain_and_history_read() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let first = a.reserve(1, Origin::Editor).unwrap();
    let first_request = first.request();
    let first_id = persist(&a.submit(first, brightness(42)));
    assert_eq!(a.commit_request(first_id), Some(first_request));
    let next = a.reserve(2, Origin::Editor).unwrap();
    let next_request = next.request();
    assert!(a.submit(next, brightness(55)).is_empty());
    let second_id = persist(&a.complete(
        first_id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    ));
    assert_eq!(a.commit_request(first_id), None);
    assert_eq!(a.commit_request(second_id), Some(next_request));
    a.complete(
        second_id,
        Outcome::Applied {
            revision: 3,
            changed: true,
        },
    );
    assert_eq!(a.commit_request(second_id), None);
    let undo = a.reserve(3, Origin::Editor).unwrap();
    let request = undo.request();
    let effects = a.submit(undo, Command::Undo);
    let Effect::ReadRevision { id, .. } = effects[0] else {
        panic!()
    };
    assert_eq!(a.commit_request(id), Some(request));
}
