use rad_core::identity::{EntityIdentity, EntityKind, IdentityError, ShortId};

#[test]
fn identity_values_reject_zero_in_explicit_input() {
    assert_eq!(
        ShortId::new(EntityKind::Button, 0),
        Err(IdentityError::InvalidValue)
    );
    assert_eq!(EntityIdentity::new(0, 1), Err(IdentityError::InvalidValue));
    assert_eq!(EntityIdentity::new(7, 0), Err(IdentityError::InvalidValue));
    let id = EntityIdentity::new(7, 17).unwrap();
    assert_eq!((id.installation(), id.serial()), (7, 17));
    let short = ShortId::new(EntityKind::Button, 17).unwrap();
    assert_eq!((short.kind(), short.number()), (EntityKind::Button, 17));
}

use rad_core::identity::{Binding, IdentityRegistry};
fn short(kind: EntityKind, number: u64) -> ShortId {
    ShortId::new(kind, number).unwrap()
}
fn entity(installation: u128, serial: u64) -> EntityIdentity {
    EntityIdentity::new(installation, serial).unwrap()
}

#[test]
fn allocating_principal_and_duplicates_uses_fresh_serials_and_class_counters() {
    let mut registry = IdentityRegistry::new(7).unwrap();
    let principal = registry.allocate(EntityKind::Folder).unwrap();
    let button = registry.allocate(EntityKind::Button).unwrap();
    let duplicate = registry.allocate(EntityKind::Button).unwrap();
    let action = registry.allocate(EntityKind::Action).unwrap();
    assert_eq!(
        principal,
        Binding::new(short(EntityKind::Folder, 1), entity(7, 1))
    );
    assert_eq!(
        button,
        Binding::new(short(EntityKind::Button, 1), entity(7, 2))
    );
    assert_eq!(
        duplicate,
        Binding::new(short(EntityKind::Button, 2), entity(7, 3))
    );
    assert_eq!(
        action,
        Binding::new(short(EntityKind::Action, 1), entity(7, 4))
    );
    assert!(registry.records().all(|record| !record.retired()));
    assert_eq!(registry.records().count(), 4);
    assert_eq!(IdentityRegistry::new(0), Err(IdentityError::InvalidValue));
}

#[test]
fn restoring_backup_retires_b18_but_never_rolls_back_its_reservation() {
    let mut registry = IdentityRegistry::new(7).unwrap();
    let mut b17 = None;
    for _ in 0..17 {
        b17 = Some(registry.allocate(EntityKind::Button).unwrap());
    }
    let b17 = b17.unwrap();
    let b18 = registry.allocate(EntityKind::Button).unwrap();
    let before = registry.clone();
    let empty = registry.restore(&[]).unwrap();
    assert_eq!(registry, before);
    assert!(empty.records().all(|r| r.retired()));
    let mut restored = empty.restore(&[b17]).unwrap();
    assert!(
        !restored
            .records()
            .find(|r| r.binding() == b17)
            .unwrap()
            .retired()
    );
    assert!(
        restored
            .records()
            .find(|r| r.binding() == b18)
            .unwrap()
            .retired()
    );
    assert_eq!(
        restored.allocate(EntityKind::Button).unwrap(),
        Binding::new(short(EntityKind::Button, 19), entity(7, 19))
    );
    let twice = restored.restore(&[b17, b17]).unwrap();
    assert_eq!(twice.records().filter(|r| !r.retired()).count(), 1);
    assert_eq!(twice.restore(&[b17]).unwrap(), twice);
}

#[test]
fn restore_rejects_reusing_retired_short_id_without_applying_a_valid_prefix() {
    let mut registry = IdentityRegistry::new(7).unwrap();
    let mut b17 = None;
    for _ in 0..17 {
        b17 = Some(registry.allocate(EntityKind::Button).unwrap());
    }
    let registry = registry.restore(&[]).unwrap();
    let before = registry.clone();
    let fresh = Binding::new(short(EntityKind::Folder, 1), entity(8, 1));
    let collision = Binding::new(b17.unwrap().short(), entity(8, 2));
    assert_eq!(
        registry.restore(&[fresh, collision]),
        Err(IdentityError::ShortIdCollision)
    );
    assert_eq!(registry, before);
    let empty = IdentityRegistry::new(7).unwrap();
    let other = Binding::new(fresh.short(), entity(9, 1));
    assert_eq!(
        empty.restore(&[fresh, other]),
        Err(IdentityError::ShortIdCollision)
    );
    assert_eq!(empty.records().count(), 0);
}

#[test]
fn restore_rejects_another_alias_or_kind_for_the_same_identity() {
    let mut registry = IdentityRegistry::new(7).unwrap();
    let principal = registry.allocate(EntityKind::Folder).unwrap();
    let before = registry.clone();
    for alias in [short(EntityKind::Folder, 2), short(EntityKind::Button, 1)] {
        let incompatible = Binding::new(alias, principal.identity());
        assert_eq!(
            registry.restore(&[incompatible]),
            Err(IdentityError::IdentityCollision)
        );
        let empty = IdentityRegistry::new(7).unwrap();
        assert_eq!(
            empty.restore(&[principal, incompatible]),
            Err(IdentityError::IdentityCollision)
        );
    }
    assert_eq!(registry, before);
    let foreign_principal = Binding::new(principal.short(), entity(8, 1));
    assert_eq!(
        registry.restore(&[foreign_principal]),
        Err(IdentityError::ShortIdCollision)
    );
}

#[test]
fn reconstruction_preserves_retired_bindings_and_imported_high_water_marks() {
    let local = Binding::new(short(EntityKind::Button, 17), entity(7, 42));
    let foreign = Binding::new(short(EntityKind::Button, 18), entity(8, 900));
    let registry = IdentityRegistry::new(7)
        .unwrap()
        .restore(&[local, foreign])
        .unwrap()
        .restore(&[foreign])
        .unwrap();
    let mut recovered =
        IdentityRegistry::from_records(registry.installation(), registry.records()).unwrap();
    assert_eq!(recovered, registry);
    assert_eq!(
        recovered.allocate(EntityKind::Button).unwrap(),
        Binding::new(short(EntityKind::Button, 19), entity(7, 43))
    );
    assert_eq!(recovered.installation(), 7);
    assert!(
        recovered
            .records()
            .find(|r| r.binding() == local)
            .unwrap()
            .retired()
    );
    assert_eq!(registry.records().count(), 2);
}

use rad_core::identity::IdentityRecord;
#[test]
fn reconstruction_rejects_inconsistent_records_instead_of_repairing_them() {
    let original = Binding::new(short(EntityKind::Button, 17), entity(7, 1));
    for (binding, retired, error) in [
        (
            Binding::new(original.short(), entity(8, 1)),
            false,
            IdentityError::ShortIdCollision,
        ),
        (
            Binding::new(short(EntityKind::Button, 18), original.identity()),
            false,
            IdentityError::IdentityCollision,
        ),
        (original, true, IdentityError::ConflictingRetirement),
    ] {
        assert_eq!(
            IdentityRegistry::from_records(
                7,
                [
                    IdentityRecord::new(original, false),
                    IdentityRecord::new(binding, retired)
                ]
            ),
            Err(error)
        );
    }
    let record = IdentityRecord::new(original, true);
    let registry = IdentityRegistry::from_records(7, [record, record]).unwrap();
    assert_eq!(registry.records().collect::<Vec<_>>(), vec![record]);
    assert_eq!(
        IdentityRegistry::from_records(0, [record]),
        Err(IdentityError::InvalidValue)
    );
}

#[test]
fn exhausted_short_id_counter_rejects_allocation_without_mutation() {
    let last = Binding::new(short(EntityKind::Button, u64::MAX - 1), entity(8, 1));
    let mut registry = IdentityRegistry::new(7).unwrap().restore(&[last]).unwrap();
    assert_eq!(
        registry.allocate(EntityKind::Button).unwrap(),
        Binding::new(short(EntityKind::Button, u64::MAX), entity(7, 1))
    );
    let mut registry = registry.restore(&[]).unwrap();
    let before = registry.clone();
    assert_eq!(
        registry.allocate(EntityKind::Button),
        Err(IdentityError::ShortIdExhausted)
    );
    assert_eq!(registry, before);
    assert_eq!(
        registry.allocate(EntityKind::Action).unwrap(),
        Binding::new(short(EntityKind::Action, 1), entity(7, 2))
    );
}

#[test]
fn exhausted_local_serial_rejects_every_kind_but_not_existing_bindings() {
    let last = Binding::new(short(EntityKind::Button, 17), entity(7, u64::MAX - 1));
    let mut registry = IdentityRegistry::new(7).unwrap().restore(&[last]).unwrap();
    let final_binding = registry.allocate(EntityKind::Action).unwrap();
    assert_eq!(
        final_binding,
        Binding::new(short(EntityKind::Action, 1), entity(7, u64::MAX))
    );
    let mut registry = registry.restore(&[]).unwrap();
    let before = registry.clone();
    for kind in [EntityKind::Folder, EntityKind::Button, EntityKind::Action] {
        assert_eq!(registry.allocate(kind), Err(IdentityError::SerialExhausted));
        assert_eq!(registry, before);
    }
    assert!(registry.restore(&[final_binding]).is_ok());
    let foreign = Binding::new(short(EntityKind::Button, 18), entity(8, u64::MAX));
    let mut another = IdentityRegistry::new(7)
        .unwrap()
        .restore(&[foreign])
        .unwrap();
    assert_eq!(
        another.allocate(EntityKind::Folder).unwrap().identity(),
        entity(7, 1)
    );
}

#[test]
fn visible_identifiers_keep_the_short_class_prefix_and_positive_number() {
    assert_eq!(short(EntityKind::Folder, 3).to_string(), "f3");
    assert_eq!(short(EntityKind::Button, 17).to_string(), "b17");
    assert_eq!(short(EntityKind::Action, 42).to_string(), "a42");
    assert_eq!(
        short(EntityKind::Button, u64::MAX).to_string(),
        "b18446744073709551615"
    );
    for kind in [EntityKind::Folder, EntityKind::Button, EntityKind::Action] {
        assert_eq!(ShortId::new(kind, 0), Err(IdentityError::InvalidValue));
    }
}
