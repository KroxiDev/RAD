use rad_core::identity::*;
fn binding(kind: EntityKind, n: u64, installation: u128, serial: u64) -> Binding {
    Binding::new(
        ShortId::new(kind, n).unwrap(),
        EntityIdentity::new(installation, serial).unwrap(),
    )
}
#[test]
fn ranges_isolate_every_class_and_extreme_namespace_without_pruning() {
    let kinds = [EntityKind::Folder, EntityKind::Button, EntityKind::Action];
    for local in [1, 2, u128::MAX] {
        let foreign = if local == 1 { u128::MAX } else { 1 };
        for kind in kinds {
            let mut records = Vec::new();
            for (i, other) in kinds.into_iter().filter(|k| *k != kind).enumerate() {
                records.push(IdentityRecord::new(
                    binding(other, u64::MAX, foreign, i as u64 + 1),
                    true,
                ));
            }
            let old = binding(kind, 17, local, 41);
            records.push(IdentityRecord::new(old, true));
            let mut registry = IdentityRegistry::from_records(local, records.clone()).unwrap();
            records.reverse();
            let reversed = IdentityRegistry::from_records(local, records).unwrap();
            assert_eq!(registry, reversed); // insertion order is not semantic state
            let next = registry.allocate(kind).unwrap();
            assert_eq!(next, binding(kind, 18, local, 42));
            let before = registry.clone();
            let fresh = binding(kind, 19, foreign, 100);
            let alias = binding(kind, 20, local, 41);
            assert_eq!(
                registry.restore(&[fresh, alias]),
                Err(IdentityError::IdentityCollision)
            );
            assert_eq!(registry, before);
            let collision = binding(kind, 17, foreign, 101);
            assert_eq!(
                registry.restore(&[fresh, collision]),
                Err(IdentityError::ShortIdCollision)
            );
            assert_eq!(registry, before);
            let restored = registry.restore(&[old, old, next]).unwrap();
            assert_eq!(restored.records().count(), 4);
            assert_eq!(restored.records().filter(|r| !r.retired()).count(), 2);
            let reconstructed = IdentityRegistry::from_records(local, restored.records()).unwrap();
            assert_eq!(reconstructed, restored);
        }
    }
}
