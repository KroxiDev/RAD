use rad_core::identity::*;
#[test]
fn durable_counters_survive_a_completely_pruned_ledger() {
    let counters = IdentityCounters {
        folders: 7,
        buttons: 10,
        actions: 12,
        serial: 40,
    };
    let mut r = IdentityRegistry::from_records_with_counters(77, [], counters).unwrap();
    let b = r.allocate(EntityKind::Button).unwrap();
    assert_eq!(b.short().number(), 11);
    assert_eq!(b.identity().serial(), 41);
    let retired = r.restore(&[]).unwrap();
    let mut r = retired.clone();
    r.retain_bindings(&Default::default());
    assert_eq!(r.records().count(), 0);
    assert_eq!(r.counters(), retired.counters());
    assert_eq!(r.allocate(EntityKind::Action).unwrap().short().number(), 13);
}
#[test]
fn reconstruction_rejects_counter_rollback_and_reservation_is_monotonic() {
    let mut registry = IdentityRegistry::new(77).unwrap();
    let binding = registry.allocate(EntityKind::Button).unwrap();
    assert_eq!(
        IdentityRegistry::from_records_with_counters(
            77,
            registry.records(),
            IdentityCounters::default()
        ),
        Err(IdentityError::InvalidValue)
    );
    registry.reserve_counters(IdentityCounters {
        folders: 10,
        buttons: 20,
        actions: 30,
        serial: 100,
    });
    registry.reserve_counters(IdentityCounters::default());
    assert_eq!(
        registry
            .allocate(EntityKind::Action)
            .unwrap()
            .identity()
            .serial(),
        101
    );
    let mut retired = registry.restore(&[]).unwrap();
    retired.retain_bindings(&[binding.identity()].into_iter().collect());
    assert_eq!(
        retired.records().map(|r| r.binding()).collect::<Vec<_>>(),
        vec![binding]
    );
    let before = retired.counters();
    let restored = retired.restore(&[binding]).unwrap();
    assert_eq!(restored.counters(), before);
}
#[test]
fn pruned_ledger_with_durable_reservations_is_not_a_new_installation() {
    use rad_core::configuration::Configuration;
    let r = IdentityRegistry::from_records_with_counters(
        77,
        [],
        IdentityCounters {
            folders: 1,
            serial: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(Configuration::prepare_new(&r).is_err());
}
#[test]
fn new_configuration_reserves_the_counters_of_an_unreadable_one() {
    use rad_core::configuration::Configuration;
    let reserved = IdentityCounters {
        folders: 7,
        buttons: 12,
        actions: 30,
        serial: 50,
    };
    let mut registry = IdentityRegistry::new(77).unwrap();
    registry.reserve_counters(reserved);
    let candidate = Configuration::prepare_new_reserving(&registry).unwrap();
    let principal = candidate.configuration().data().folders[0].binding;
    assert_eq!(principal.short().to_string(), "f8");
    assert_eq!(principal.identity().serial(), 51);
    assert_eq!(
        candidate.registry().counters(),
        IdentityCounters {
            folders: 8,
            buttons: 12,
            actions: 30,
            serial: 51
        }
    );
    let mut next = candidate.registry().clone();
    assert_eq!(
        next.allocate(EntityKind::Button)
            .unwrap()
            .short()
            .to_string(),
        "b13"
    );
}
#[test]
fn reserving_preparation_rejects_a_registry_with_records() {
    use rad_core::configuration::{Configuration, ConfigurationError};
    let mut registry = IdentityRegistry::new(77).unwrap();
    registry.allocate(EntityKind::Folder).unwrap();
    assert_eq!(
        Configuration::prepare_new_reserving(&registry).unwrap_err(),
        ConfigurationError::RegistryMismatch
    );
}
