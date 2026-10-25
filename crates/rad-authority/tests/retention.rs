use rad_authority::{Authority, Effect, Outcome};
use rad_core::{
    configuration::{Configuration, Edit, Origin, SettingsData},
    identity::IdentityRegistry,
};
fn actor() -> Authority {
    Authority::new(
        Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap(),
        1,
        std::num::NonZeroU128::new(1).unwrap(),
    )
}
#[test]
fn export_lease_retains_snapshot_after_history_eviction_and_expires_on_idle_deadline() {
    let mut a = actor();
    let lease = a.pin_export(1, Origin::Editor).unwrap();
    assert_eq!(lease.revision(), 1);
    assert!(a.pin_export(1, Origin::Editor).is_err());
    let second = a.pin_export(2, Origin::Editor).unwrap();
    assert!(a.pin_export(3, Origin::Editor).is_err());
    assert!(a.pin_export(4, Origin::Automation).is_err());
    assert!(a.release_export(second));
    for i in 0..30 {
        let t = a.reserve(8, Origin::Editor).unwrap();
        let e = a.submit(
            t,
            vec![Edit::SetSettings(SettingsData {
                brightness_percent: i + 10,
                ..SettingsData::default()
            })],
        );
        let Effect::Persist { id, .. } = e[0] else {
            panic!()
        };
        for e in a.complete(
            id,
            Outcome::Applied {
                revision: i64::from(i) + 2,
                changed: true,
            },
        ) {
            if let Effect::Reply { request, .. } = e {
                a.finish(request);
            }
        }
    }
    assert!(a.retained_revisions().contains(&1));
    a.advance(29999).unwrap();
    a.touch_export(&lease).unwrap();
    assert_eq!(a.next_deadline(), Some(59999));
    a.advance(59999).unwrap();
    assert!(!a.retained_revisions().contains(&1));
    assert!(!a.release_export(lease));
}
#[test]
fn disconnected_export_releases_its_transfer_credit() {
    let c = rad_core::configuration::Configuration::prepare_new(
        &rad_core::identity::IdentityRegistry::new(9).unwrap(),
    )
    .unwrap();
    let mut a = rad_authority::Authority::new(c, 1, std::num::NonZeroU128::new(1).unwrap());
    let old = a
        .pin_export(4, rad_core::configuration::Origin::Editor)
        .unwrap();
    a.disconnect(4);
    assert!(
        a.pin_export(4, rad_core::configuration::Origin::Editor)
            .is_ok()
    );
    assert!(!a.release_export(old));
}
