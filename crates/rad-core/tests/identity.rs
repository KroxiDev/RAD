use rad_core::identity::{EntityIdentity, EntityKind, IdentityError, ShortId};

use rad_core::identity::{Binding, IdentityRegistry};
fn short(kind: EntityKind, number: u64) -> ShortId {
    ShortId::new(kind, number).unwrap()
}
fn entity(installation: u128, serial: u64) -> EntityIdentity {
    EntityIdentity::new(installation, serial).unwrap()
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
