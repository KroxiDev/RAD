use rad_authority::{Authority, Effect, Outcome};
use rad_core::{
    configuration::{Configuration, Edit, Origin, SettingsData},
    identity::IdentityRegistry,
};
fn actor() -> Authority {
    Authority::new(
        Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap(),
        1,
        std::num::NonZeroU128::new(1).unwrap(),
    )
}
fn brightness(n: u8) -> Vec<Edit> {
    vec![Edit::SetSettings(SettingsData {
        brightness_percent: n,
        ..SettingsData::default()
    })]
}
#[test]
fn publishes_only_after_matching_applied_and_appends_history() {
    let mut a = actor();
    let effects = a.edit(Origin::Editor, brightness(42));
    let Effect::Persist {
        id,
        candidate,
        document,
    } = &effects[0]
    else {
        panic!("persist expected")
    };
    assert_eq!(
        candidate.configuration().settings().brightness_percent(),
        42
    );
    assert_eq!(
        rad_codec::decode_data(document).unwrap(),
        *candidate.configuration().data()
    );
    assert_eq!(a.editor_view(Origin::Editor).unwrap().brightness, 100);
    assert!(a.editor_view(Origin::Automation).is_none());
    let id = *id;
    assert!(matches!(
        a.complete_without_display(
            id,
            Outcome::Applied {
                revision: 2,
                changed: true
            }
        )
        .as_slice(),
        [Effect::Reply { changed: true, .. }]
    ));
    let v = a.editor_view(Origin::Editor).unwrap();
    assert_eq!((v.revision, v.brightness, v.history_operations), (2, 42, 1));
}
fn commit_id(effects: &[Effect]) -> rad_authority::CommitId {
    let Effect::Persist { id, .. } = effects[0] else {
        panic!("persist expected")
    };
    id
}
#[test]
fn not_applied_keeps_publication_and_duplicate_completion_cannot_publish_another_commit() {
    let mut a = actor();
    let first = commit_id(&a.edit(Origin::Editor, brightness(42)));
    assert!(matches!(
        a.complete_without_display(first, Outcome::NotApplied)
            .as_slice(),
        [Effect::Rejected { .. }]
    ));
    assert_eq!(a.editor_view(Origin::Editor).unwrap().brightness, 100);
    let second = commit_id(&a.edit(Origin::Editor, brightness(55)));
    assert!(
        a.complete_without_display(
            first,
            Outcome::Applied {
                revision: 2,
                changed: true
            }
        )
        .is_empty()
    );
    assert_eq!(a.editor_view(Origin::Editor).unwrap().history_operations, 0);
    a.complete_without_display(
        second,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert_eq!(a.editor_view(Origin::Editor).unwrap().brightness, 55);
    assert!(
        a.complete_without_display(
            second,
            Outcome::Applied {
                revision: 2,
                changed: true
            }
        )
        .is_empty()
    );
    assert_eq!(a.editor_view(Origin::Editor).unwrap().history_operations, 1);
}
#[test]
fn fifo_prepares_next_only_against_new_publication_and_invalid_edit_does_not_block_next() {
    let mut a = actor();
    let first = commit_id(&a.edit(Origin::Editor, brightness(42)));
    assert!(a.edit(Origin::Editor, brightness(101)).is_empty());
    assert!(a.edit(Origin::Editor, brightness(55)).is_empty());
    let e = a.complete_without_display(
        first,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert!(matches!(
        e.as_slice(),
        [
            Effect::Reply { changed: true, .. },
            Effect::Rejected { .. },
            Effect::Persist { .. }
        ]
    ));
    let next = commit_id(&e[2..]);
    a.complete_without_display(
        next,
        Outcome::Applied {
            revision: 3,
            changed: true,
        },
    );
    assert_eq!(a.editor_view(Origin::Editor).unwrap().brightness, 55);
}

#[test]
fn uncertain_keeps_pending_rejects_waiters_and_reconciliation_does_not_replay_commit() {
    let mut a = actor();
    let id = commit_id(&a.edit(Origin::Editor, brightness(42)));
    assert!(a.edit(Origin::Editor, brightness(55)).is_empty());
    let e = a.complete_without_display(id, Outcome::Uncertain);
    assert!(matches!(
        e.as_slice(),
        [Effect::Unknown { .. }, Effect::Rejected { .. }]
    ));
    assert_eq!(a.editor_view(Origin::Editor).unwrap().brightness, 100);
    assert!(matches!(
        a.edit(Origin::Editor, brightness(66)).as_slice(),
        [Effect::Rejected { .. }]
    ));
    let id = reconcile_id(&a.reconcile());
    assert!(a.reconcile().is_empty());
    assert!(matches!(
        a.complete_without_display(id, Outcome::Uncertain)
            .as_slice(),
        [Effect::Unknown { .. }]
    ));
    let id = reconcile_id(&a.reconcile());
    assert!(matches!(
        a.complete_without_display(
            id,
            Outcome::Applied {
                revision: 2,
                changed: true
            }
        )
        .as_slice(),
        [Effect::Reply { changed: true, .. }]
    ));
    assert_eq!(a.editor_view(Origin::Editor).unwrap().history_operations, 1);
}

fn reconcile_id(effects: &[Effect]) -> rad_authority::CommitId {
    let Effect::Reconcile { id } = effects[0] else {
        panic!()
    };
    id
}

#[test]
fn transferred_ticket_is_held_until_terminal_response_consumed_and_commit_cannot_be_cancelled() {
    let mut a = actor();
    let ticket = a.reserve(1, Origin::Editor).unwrap();
    let request = ticket.request();
    let id = commit_id(&a.submit(ticket, brightness(42)));
    assert!(!a.finish(request));
    let _held: Vec<_> = (0..3)
        .map(|_| a.reserve(1, Origin::Editor).unwrap())
        .collect();
    assert!(matches!(
        a.reserve(1, Origin::Editor),
        Err(rad_authority::AdmissionError::Capacity)
    ));
    a.complete_without_display(
        id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert!(a.finish(request));
    assert!(!a.finish(request));
    assert!(a.reserve(1, Origin::Editor).is_ok());
    assert_eq!(a.editor_view(Origin::Editor).unwrap().brightness, 42);
}

trait Edits {
    fn edit(&mut self, origin: Origin, edits: Vec<Edit>) -> Vec<Effect>;
}
impl Edits for Authority {
    fn edit(&mut self, origin: Origin, edits: Vec<Edit>) -> Vec<Effect> {
        let t = self.reserve(999, origin).unwrap();
        let effects = self.submit(t, edits);
        for e in &effects {
            if let Effect::Reply { request, .. } | Effect::Rejected { request, .. } = e {
                self.finish(*request);
            }
        }
        effects
    }
}

#[test]
fn foreign_session_ticket_and_completion_cannot_alias_local_work() {
    let mut a = actor();
    let mut b = Authority::new(
        Configuration::prepare_new(&IdentityRegistry::new(7).unwrap()).unwrap(),
        1,
        std::num::NonZeroU128::new(2).unwrap(),
    );
    let ta = a.reserve(1, Origin::Editor).unwrap();
    let tb = b.reserve(1, Origin::Editor).unwrap();
    assert!(b.submit(ta, brightness(42)).is_empty());
    let b_id = commit_id(&b.submit(tb, brightness(55)));
    let a_id = commit_id(&a.edit(Origin::Editor, brightness(42)));
    assert_ne!(a_id, b_id);
    assert!(
        b.complete_without_display(
            a_id,
            Outcome::Applied {
                revision: 2,
                changed: true
            }
        )
        .is_empty()
    );
    assert_eq!(b.editor_view(Origin::Editor).unwrap().brightness, 100);
    b.complete_without_display(
        b_id,
        Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert_eq!(b.editor_view(Origin::Editor).unwrap().brightness, 55);
}

// Publication assertions isolate persistence responses; display effects are
// independently asserted by runtime::sleep_publication_projects_brightness_and_wake_consumes_press.
trait PublicationEffects {
    fn complete_without_display(
        &mut self,
        id: rad_authority::CommitId,
        outcome: Outcome,
    ) -> Vec<Effect>;
}
impl PublicationEffects for Authority {
    fn complete_without_display(
        &mut self,
        id: rad_authority::CommitId,
        outcome: Outcome,
    ) -> Vec<Effect> {
        self.complete(id, outcome)
            .into_iter()
            .filter(|e| !matches!(e, Effect::Display { .. }))
            .collect()
    }
}
