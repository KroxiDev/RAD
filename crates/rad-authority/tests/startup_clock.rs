use rad_authority::{Authority, Effect};
use rad_core::{configuration::Configuration, identity::IdentityRegistry};
#[test]
fn initial_monotonic_epoch_is_not_prior_user_inactivity() {
    let initial = Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap();
    let sleep = initial.configuration().settings().sleep_ms();
    assert!(sleep > 0);
    // Runtime starts after OS uptime > configured idle interval; no inactivity in RAD yet.
    let start = sleep + 1000;
    let mut actor = Authority::new_at(initial, 1, std::num::NonZeroU128::new(1).unwrap(), start);
    let mut effects = actor.before_event(start).unwrap();
    effects.extend(actor.advance(start).unwrap());
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::Display { asleep: true, .. })),
        "fresh RAD start must not treat time since OS boot as its inactivity"
    );
    assert_eq!(actor.next_deadline(), Some(start + sleep));
    assert!(
        actor
            .advance(start + sleep)
            .unwrap()
            .iter()
            .any(|e| matches!(e, Effect::Display { asleep: true, .. }))
    );
}
