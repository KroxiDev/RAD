use rad_authority::{Authority, Command, CommitId, Effect, Outcome};
use rad_core::{
    configuration::{Candidate, Configuration, Edit, Origin, SettingsData},
    identity::IdentityRegistry,
};
fn initial() -> Candidate {
    Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap()
}
fn actor() -> Authority {
    Authority::new(initial(), 1, std::num::NonZeroU128::new(1).unwrap())
}
fn submit(a: &mut Authority, c: Command) -> Vec<Effect> {
    let t = a.reserve(1, Origin::Editor).unwrap();
    a.submit(t, c)
}
fn persist(e: &[Effect]) -> (CommitId, Candidate) {
    let Effect::Persist { id, candidate, .. } = &e[0] else {
        panic!("persist expected")
    };
    (*id, candidate.clone())
}
fn finish(a: &mut Authority, id: CommitId, outcome: Outcome) {
    for e in a.complete(id, outcome) {
        if let Effect::Reply { request, .. } | Effect::Rejected { request, .. } = e {
            assert!(a.finish(request));
        }
    }
}
fn read(e: &[Effect], revision: i64) -> CommitId {
    let Effect::ReadRevision { id, revision: r } = e[0] else {
        panic!("read expected")
    };
    assert_eq!(r, revision);
    id
}
#[test]
fn undo_redo_persist_against_current_registry_move_cursor_only_after_applied() {
    let mut a = actor();
    let (id, changed) = persist(&submit(
        &mut a,
        Command::Edits(vec![Edit::SetSettings(SettingsData {
            brightness_percent: 42,
            ..SettingsData::default()
        })]),
    ));
    finish(
        &mut a,
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    let read_id = read(&submit(&mut a, Command::Undo), 1);
    assert!(
        a.complete(
            read_id,
            Outcome::Applied {
                revision: 3,
                changed: true
            }
        )
        .is_empty()
    );
    let (id, _) = persist(&a.loaded(read_id, Ok(initial().configuration().data().clone())));
    assert_eq!(a.editor_view(Origin::Editor).unwrap().brightness, 42);
    finish(
        &mut a,
        id,
        Outcome::Applied {
            revision: 3,
            changed: true,
        },
    );
    assert_eq!(a.editor_view(Origin::Editor).unwrap().brightness, 100);
    assert_eq!(a.editor_view(Origin::Editor).unwrap().history_operations, 1);
    let rid = read(&submit(&mut a, Command::Redo), 2);
    let (id, _) = persist(&a.loaded(rid, Ok(changed.configuration().data().clone())));
    finish(&mut a, id, Outcome::NotApplied);
    let rid = read(&submit(&mut a, Command::Redo), 2);
    let (id, _) = persist(&a.loaded(rid, Ok(changed.configuration().data().clone())));
    finish(
        &mut a,
        id,
        Outcome::Applied {
            revision: 4,
            changed: true,
        },
    );
    assert_eq!(a.editor_view(Origin::Editor).unwrap().brightness, 42);
    assert_eq!(a.editor_view(Origin::Editor).unwrap().history_operations, 1);
}
#[test]
fn confirmed_restore_is_revalidated_at_fifo_turn_and_keeps_retired_ids() {
    let mut a = actor();
    let principal = initial().configuration().data().principal;
    let (id, created) = persist(&submit(
        &mut a,
        Command::Edits(vec![Edit::CreateButton {
            folder: principal,
            position: 0,
            name: "a".into(),
        }]),
    ));
    // Restore arrives against the old screen; it must use the ledger after create commits.
    assert!(
        submit(
            &mut a,
            Command::Restore {
                data: initial().configuration().data().clone(),
                confirmed: true
            }
        )
        .is_empty()
    );
    let e = a.complete(
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    let (rid, restored) = persist(&e[1..]);
    assert_eq!(restored.registry().records().count(), 2);
    assert_eq!(
        restored
            .registry()
            .records()
            .filter(|r| r.retired())
            .count(),
        1
    );
    assert_eq!(restored.configuration(), initial().configuration());
    let Effect::Reply { request, .. } = e[0] else {
        panic!()
    };
    a.finish(request);
    finish(
        &mut a,
        rid,
        Outcome::Applied {
            revision: 3,
            changed: true,
        },
    );
    let denied = submit(
        &mut a,
        Command::Restore {
            data: created.configuration().data().clone(),
            confirmed: false,
        },
    );
    assert!(matches!(denied.as_slice(), [Effect::Rejected { .. }]));
    assert_eq!(a.editor_view(Origin::Editor).unwrap().history_operations, 2);
}
