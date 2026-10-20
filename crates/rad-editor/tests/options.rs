//! Dynamic option lists preserve saved values and reject stale or invalid answers.
//! Tests use the runtime `options` request shape without sockets or windows.
use rad_editor::{
    Request,
    options::{Failure, ListState, Target},
    plugins::FormDraft,
    session::Error,
};
use rad_plugin_contract::{ChoiceList, ChoiceOption, Definition, Field, FieldKind, Schema};
use serde_json::{Value, json};

const TYPE: &str = "org.rad.test.options:pick";

fn field(id: &str, required: bool, kind: FieldKind) -> Field {
    Field {
        id: id.into(),
        label: id.into(),
        required,
        kind,
    }
}

/// The shape of the `pick` action of the options test plugin: a fixed `scenario`, then a
/// `group` that depends on it and a `member` that depends on both.
fn pick() -> Definition {
    let scenario = ["normal", "empty", "unavailable", "slow", "late", "crash"];
    Definition {
        response_timeout: None,
        id: "pick".into(),
        label: "pick".into(),
        schema: Schema {
            version: 1,
            fields: vec![
                field(
                    "scenario",
                    true,
                    FieldKind::Choice {
                        options: scenario.map(String::from).to_vec(),
                    },
                ),
                field(
                    "group",
                    true,
                    FieldKind::DynamicChoice {
                        depends_on: vec!["scenario".into()],
                    },
                ),
                field(
                    "member",
                    false,
                    FieldKind::DynamicChoice {
                        depends_on: vec!["scenario".into(), "group".into()],
                    },
                ),
            ],
        },
        credential_slots: vec![],
    }
}

fn saved() -> Vec<u8> {
    br#"{"schema_version":1,"values":{"scenario":"normal","group":"a","member":"a1"}}"#.to_vec()
}

fn editing() -> Target<'static> {
    Target {
        type_key: TYPE,
        entity: Some("action-1"),
    }
}

fn adding() -> Target<'static> {
    Target {
        type_key: TYPE,
        entity: None,
    }
}

#[test]
fn editing_asks_by_entity_with_the_draft_values_of_the_dependencies() {
    let mut form = FormDraft::load(pick(), saved());
    form.set(0, Some("empty".into()));
    let query = form.next_query(editing(), 7).expect("a question is due");
    assert_eq!(query.ticket, 7);
    assert_eq!(query.field, 1);
    assert_eq!(
        query.request,
        Request::new(
            "options",
            json!({"id":"action-1","field":"group","cursor":null,"context":{"scenario":"empty"}})
        )
    );
}

/// A list that depends on a required choice (`kind`) and on an optional one (`tag`).
fn mixed() -> Definition {
    let choice = |options: &[&str]| FieldKind::Choice {
        options: options.iter().map(|o| o.to_string()).collect(),
    };
    Definition {
        response_timeout: None,
        id: "mixed".into(),
        label: "mixed".into(),
        schema: Schema {
            version: 1,
            fields: vec![
                field("kind", true, choice(&["x", "y"])),
                field("tag", false, choice(&["t1", "t2"])),
                field(
                    "item",
                    true,
                    FieldKind::DynamicChoice {
                        depends_on: vec!["kind".into(), "tag".into()],
                    },
                ),
            ],
        },
        credential_slots: vec![],
    }
}

fn item_query(form: &mut FormDraft, target: Target<'_>) -> Option<Request> {
    form.next_query(target, 1).map(|query| query.request)
}

#[test]
fn adding_asks_by_type_and_sends_only_the_dependencies_that_are_fixed() {
    let mut required_only = FormDraft::new(mixed());
    required_only.set(0, Some("x".into()));
    assert_eq!(
        item_query(&mut required_only, adding()),
        Some(Request::new(
            "options",
            json!({"type":TYPE,"field":"item","cursor":null,"context":{"kind":"x"}}),
        )),
        "the optional `tag` has no value, so it is not in the context"
    );

    let mut both = FormDraft::new(mixed());
    both.set(0, Some("x".into()));
    both.set(1, Some("t2".into()));
    assert_eq!(
        item_query(&mut both, adding()),
        Some(Request::new(
            "options",
            json!({"type":TYPE,"field":"item","cursor":null,
                   "context":{"kind":"x","tag":"t2"}}),
        ))
    );
}

#[test]
fn a_saved_optional_dependency_cleared_in_the_draft_is_asked_by_type_so_the_saved_value_cannot_return()
 {
    // Asked by entity, the runtime would add the saved `tag` back to the context.
    let saved = br#"{"schema_version":1,"values":{"kind":"x","tag":"t1","item":"i"}}"#;
    let mut form = FormDraft::load(mixed(), saved.to_vec());
    form.set(1, Some(String::new()));
    assert_eq!(
        item_query(&mut form, editing()),
        Some(Request::new(
            "options",
            json!({"type":TYPE,"field":"item","cursor":null,"context":{"kind":"x"}})
        ))
    );
    let mut kept = FormDraft::load(mixed(), saved.to_vec());
    assert_eq!(
        item_query(&mut kept, editing()),
        Some(Request::new(
            "options",
            json!({"id":"action-1","field":"item","cursor":null,
                   "context":{"kind":"x","tag":"t1"}})
        )),
        "untouched, it is asked by entity"
    );
}

#[test]
fn a_list_is_not_asked_while_a_required_dependency_has_no_value_and_it_says_which() {
    let mut form = FormDraft::new(pick());
    assert_eq!(form.next_query(adding(), 1), None, "scenario is not chosen");
    assert_eq!(form.waiting_for(1), Some("scenario"));
    assert_eq!(
        form.waiting_for(2),
        Some("scenario"),
        "the first one missing"
    );
    assert_eq!(form.list(1), Some(&ListState::Due), "not asked, not failed");
    assert_eq!(
        form.waiting_for(0),
        None,
        "a fixed choice waits for nothing"
    );

    form.set(0, Some("normal".into()));
    assert_eq!(form.waiting_for(1), None);
    let group = form
        .next_query(adding(), 1)
        .expect("group can be asked now");
    assert_eq!(group.field, 1);
    form.accept_answer(1, Ok(answer("group", "available", &[("a", "A")])));
    assert_eq!(
        form.next_query(adding(), 2),
        None,
        "member still needs the group"
    );
    assert_eq!(form.waiting_for(2), Some("group"));

    form.set(2, Some(String::new())); // the user sets the optional member...
    form.set(1, Some("a".into())); // ...and picks the group
    let member = form
        .next_query(adding(), 2)
        .expect("member can be asked now");
    assert_eq!(member.field, 2);
    assert_eq!(form.waiting_for(2), None);
}

#[test]
fn an_optional_dependency_without_a_value_never_holds_a_list_back() {
    let mut form = FormDraft::new(mixed());
    form.set(0, Some("x".into()));
    assert_eq!(form.waiting_for(2), None, "`tag` is optional");
    assert!(form.next_query(adding(), 1).is_some());
}

#[test]
fn a_context_the_contract_rejects_is_never_sent_and_is_reported_on_its_field() {
    let mut form = FormDraft::load(pick(), saved());
    form.set(0, Some("not-a-scenario".into()));
    assert_eq!(form.next_query(editing(), 1), None);
    let rejected = ListState::Failed(Failure::Other("invalid_request".into()));
    assert_eq!(form.list(1), Some(&rejected));
    assert_eq!(form.list(2), Some(&rejected));
    assert_eq!(form.list(0), None, "a fixed choice has no list to ask for");
}

/// The `ok` data of an `options` answer by type or entity (`OPTIONS-IPC.md` section 1).
fn answer(field: &str, availability: &str, items: &[(&str, &str)]) -> Value {
    json!({
        "type": TYPE, "field": field, "availability": availability,
        "items": items.iter().map(|(value, label)| json!({"value":value,"label":label})).collect::<Vec<_>>(),
        "next_cursor": null,
    })
}

fn available(items: &[(&str, &str)]) -> ChoiceList {
    ChoiceList::Available {
        options: items
            .iter()
            .map(|(value, label)| ChoiceOption {
                value: (*value).into(),
                label: (*label).into(),
            })
            .collect(),
    }
}

fn refusal(code: &str) -> Error {
    Error {
        code: code.into(),
        unknown: false,
    }
}

/// A form with its `group` question in flight, as the editor leaves it right after asking.
fn asking_group() -> FormDraft {
    let mut form = FormDraft::load(pick(), saved());
    form.next_query(editing(), 1).expect("group is due");
    form
}

#[test]
fn an_available_list_keeps_the_order_and_the_labels_the_plugin_gave() {
    let mut form = asking_group();
    let data = answer("group", "available", &[("b", "Group B"), ("a", "Group A")]);
    assert!(form.accept_answer(1, Ok(data)));
    let expected = available(&[("b", "Group B"), ("a", "Group A")]);
    assert_eq!(form.list(1), Some(&ListState::Answered(expected)));
}

#[test]
fn an_empty_list_and_an_unavailable_source_are_different_answers() {
    let mut empty = asking_group();
    empty.accept_answer(1, Ok(answer("group", "available", &[])));
    let mut absent = asking_group();
    absent.accept_answer(1, Ok(answer("group", "unavailable", &[])));
    assert_eq!(empty.list(1), Some(&ListState::Answered(available(&[]))));
    assert_eq!(
        absent.list(1),
        Some(&ListState::Answered(ChoiceList::Unavailable))
    );
}

#[test]
fn each_refusal_is_its_own_state_and_none_changes_the_draft() {
    let cases = [
        ("options_timeout", Failure::NoAnswer),
        ("resource_exhausted", Failure::Busy),
        ("executor_unavailable", Failure::PluginUnavailable),
        ("not_found", Failure::Other("not_found".into())),
        (
            "runtime_unavailable",
            Failure::Other("runtime_unavailable".into()),
        ),
    ];
    for (code, failure) in cases {
        let mut form = asking_group();
        assert!(form.accept_answer(1, Err(refusal(code))));
        assert_eq!(form.list(1), Some(&ListState::Failed(failure)), "{code}");
        assert_eq!(form.publish(), Ok(None), "{code}: nothing was edited");
        assert_eq!(form.fields[1].as_deref(), Some("a"), "{code}: value kept");
    }
}

#[test]
fn an_answer_that_breaks_the_contract_is_an_error_and_never_a_list() {
    let wrong_field = answer("member", "available", &[("a", "A")]);
    let unlisted_but_unavailable = answer("group", "unavailable", &[("a", "A")]);
    let mut unknown_state = answer("group", "available", &[]);
    unknown_state["availability"] = json!("maybe");
    let mut no_items = answer("group", "available", &[]);
    no_items["items"] = json!("none");
    let mut bad_value = answer("group", "available", &[("a", "A")]);
    bad_value["items"][0]["value"] = json!(7);
    let mut bad_label = answer("group", "available", &[("a", "A")]);
    bad_label["items"][0]["label"] = json!("");
    let too_many: Vec<_> = (0..65).map(|n| (n.to_string(), n.to_string())).collect();
    let too_many: Vec<_> = too_many
        .iter()
        .map(|(v, l)| (v.as_str(), l.as_str()))
        .collect();
    let too_many = answer("group", "available", &too_many);
    for (name, data) in [
        ("wrong field", wrong_field),
        ("unavailable with items", unlisted_but_unavailable),
        ("unknown availability", unknown_state),
        ("items not a list", no_items),
        ("value not text", bad_value),
        ("empty label", bad_label),
        ("more than 64 items", too_many),
    ] {
        let mut form = asking_group();
        assert!(form.accept_answer(1, Ok(data)), "{name}");
        let broken = ListState::Failed(Failure::Other("invalid_request".into()));
        assert_eq!(form.list(1), Some(&broken), "{name}");
    }
}

#[test]
fn an_answer_to_another_question_is_ignored() {
    let mut form = asking_group();
    assert!(!form.accept_answer(2, Ok(answer("group", "available", &[("x", "X")]))));
    assert!(!form.accept_answer(0, Err(refusal("options_timeout"))));
    assert_eq!(form.list(1), Some(&ListState::Asking(1)));
    let mut never_asked = FormDraft::load(pick(), saved());
    assert!(!never_asked.accept_answer(1, Ok(answer("group", "available", &[]))));
    assert_eq!(never_asked.list(1), Some(&ListState::Due));
}

#[test]
fn one_question_is_in_flight_and_the_next_follows_its_answer_with_the_draft_context() {
    let mut form = FormDraft::load(pick(), saved());
    form.set(1, Some("b".into()));
    let first = form.next_query(editing(), 1).expect("group is due");
    assert_eq!(first.field, 1);
    assert_eq!(
        form.next_query(editing(), 2),
        None,
        "no queue behind a question"
    );
    assert!(form.accept_answer(
        1,
        Ok(answer("group", "available", &[("a", "A"), ("b", "B")]))
    ));
    let second = form.next_query(editing(), 3).expect("member is due");
    assert_eq!((second.ticket, second.field), (3, 2));
    assert_eq!(
        second.request,
        Request::new(
            "options",
            json!({"id":"action-1","field":"member","cursor":null,
                   "context":{"scenario":"normal","group":"b"}})
        ),
        "the group of the draft, not the saved one"
    );
    assert!(form.accept_answer(3, Ok(answer("member", "available", &[("b1", "B1")]))));
    assert_eq!(
        form.next_query(editing(), 4),
        None,
        "everything is answered"
    );
}

#[test]
fn a_saved_value_the_list_lacks_is_kept_and_published_unless_the_user_picks_another() {
    let mut form = FormDraft::load(pick(), saved());
    form.next_query(editing(), 1);
    form.accept_answer(
        1,
        Ok(answer("group", "available", &[("a", "A"), ("b", "B")])),
    );
    form.next_query(editing(), 2);
    form.accept_answer(2, Ok(answer("member", "available", &[("b1", "B1")])));
    assert!(!form.is_unavailable(1), "the saved group is listed");
    assert!(form.is_unavailable(2), "the saved member is not");
    assert_eq!(form.publish(), Ok(None), "looking at it changes nothing");

    form.set(1, Some("b".into()));
    let kept = form.publish().unwrap().unwrap();
    assert_eq!(
        kept["values"]["member"], "a1",
        "an unlisted value is not dropped"
    );
    assert_eq!(kept["values"]["group"], "b");

    form.set(2, Some("b1".into()));
    let picked = form.publish().unwrap().unwrap();
    assert_eq!(picked["values"]["member"], "b1");
}

#[test]
fn a_source_that_is_unavailable_marks_every_chosen_value_but_an_empty_choice_marks_nothing() {
    let mut form = FormDraft::load(pick(), saved());
    assert!(
        !form.is_unavailable(1),
        "nothing is known before the answer"
    );
    form.next_query(editing(), 1);
    form.accept_answer(1, Ok(answer("group", "unavailable", &[])));
    assert!(form.is_unavailable(1));
    form.set(1, Some(String::new()));
    assert!(!form.is_unavailable(1), "no value, nothing to mark");
    assert!(
        !form.is_unavailable(0),
        "a fixed choice is never marked here"
    );
}

/// A chain where each list depends only on the one before it: `kind` -> `a` -> `b` -> `c`.
fn chain() -> Definition {
    let dynamic = |id: &str, from: &str| {
        field(
            id,
            false,
            FieldKind::DynamicChoice {
                depends_on: vec![from.into()],
            },
        )
    };
    Definition {
        response_timeout: None,
        id: "chain".into(),
        label: "chain".into(),
        schema: Schema {
            version: 1,
            fields: vec![
                field(
                    "kind",
                    true,
                    FieldKind::Choice {
                        options: vec!["x".into(), "y".into()],
                    },
                ),
                dynamic("a", "kind"),
                dynamic("b", "a"),
                dynamic("c", "b"),
            ],
        },
        credential_slots: vec![],
    }
}

/// Answers every question the form has due, as a runtime with one item per list would.
fn answer_everything(form: &mut FormDraft, mut ticket: u64) -> u64 {
    while let Some(query) = form.next_query(editing(), ticket) {
        let field = query.request.arguments["field"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(form.accept_answer(query.ticket, Ok(answer(&field, "available", &[("v", "V")]))));
        ticket += 1;
    }
    ticket
}

#[test]
fn changing_a_dependency_asks_its_dependents_again_and_drops_their_old_lists() {
    let mut form = FormDraft::load(pick(), saved());
    let ticket = answer_everything(&mut form, 1);
    assert!(matches!(form.list(2), Some(ListState::Answered(_))));

    form.set(1, Some("b".into()));
    assert!(
        matches!(form.list(1), Some(ListState::Answered(_))),
        "the group list does not depend on the group"
    );
    assert_eq!(
        form.list(2),
        Some(&ListState::Due),
        "the old member list is gone"
    );
    let query = form
        .next_query(editing(), ticket)
        .expect("member is due again");
    assert_eq!(
        query.request,
        Request::new(
            "options",
            json!({"id":"action-1","field":"member","cursor":null,
                   "context":{"scenario":"normal","group":"b"}})
        ),
        "asked with the new context"
    );
    assert_eq!(
        form.fields[2].as_deref(),
        Some("a1"),
        "the member value stays"
    );
}

#[test]
fn an_answer_for_a_context_that_has_since_changed_is_ignored() {
    let mut form = FormDraft::load(pick(), saved());
    let ticket = answer_everything(&mut form, 1);
    form.set(1, Some("a".into()));
    form.set(0, Some("empty".into()));
    let stale = form.next_query(editing(), ticket).expect("group is due");
    assert_eq!(stale.field, 1);

    // While that question is out, the user changes the scenario again.
    form.set(0, Some("normal".into()));
    assert!(
        !form.accept_answer(
            stale.ticket,
            Ok(answer("group", "available", &[("old", "Old")]))
        ),
        "the answer is for the previous scenario"
    );
    assert_eq!(form.list(1), Some(&ListState::Due));
    let fresh = form.next_query(editing(), ticket + 1).expect("asked again");
    assert_eq!((fresh.ticket, fresh.field), (ticket + 1, 1));
    assert!(form.accept_answer(
        fresh.ticket,
        Ok(answer("group", "available", &[("a", "A")]))
    ));
    assert_eq!(
        form.list(1),
        Some(&ListState::Answered(available(&[("a", "A")])))
    );
}

#[test]
fn the_dependents_of_a_dependent_are_asked_again_in_cascade() {
    let mut form = FormDraft::load(
        chain(),
        br#"{"schema_version":1,"values":{"kind":"x","a":"1","b":"2","c":"3"}}"#.to_vec(),
    );
    answer_everything(&mut form, 1);
    form.set(0, Some("y".into()));
    for (index, name) in [(1, "a"), (2, "b"), (3, "c")] {
        assert_eq!(
            form.list(index),
            Some(&ListState::Due),
            "{name} depends on the change"
        );
    }
}

#[test]
fn a_value_that_leaves_the_context_as_it_was_asks_nothing() {
    let mut form = FormDraft::load(pick(), saved());
    answer_everything(&mut form, 1);
    form.set(1, Some("a".into()));
    form.set(0, Some("normal".into()));
    form.set(2, Some("a2".into()));
    assert_eq!(form.next_query(editing(), 99), None);
    assert!(matches!(form.list(2), Some(ListState::Answered(_))));
}

#[test]
fn asking_again_is_the_users_choice_and_only_for_a_list_that_is_not_in_flight() {
    let mut form = FormDraft::load(pick(), saved());
    let query = form.next_query(editing(), 1).unwrap();
    form.refresh_list(1);
    assert_eq!(
        form.list(1),
        Some(&ListState::Asking(1)),
        "no second question while one is out"
    );
    form.accept_answer(query.ticket, Err(refusal("options_timeout")));
    answer_everything(&mut form, 2);
    assert_eq!(
        form.next_query(editing(), 9),
        None,
        "a failure is not retried by itself"
    );

    form.refresh_list(1);
    assert_eq!(form.list(1), Some(&ListState::Due));
    form.refresh_list(0);
    assert_eq!(
        form.list(0),
        None,
        "a fixed choice has nothing to ask again"
    );
    let again = form
        .next_query(editing(), 10)
        .expect("the user asked again");
    assert_eq!((again.ticket, again.field), (10, 1));
}

#[test]
fn an_optional_field_that_is_not_set_is_not_asked_until_the_user_sets_it() {
    let without_member = br#"{"schema_version":1,"values":{"scenario":"normal","group":"a"}}"#;
    let mut form = FormDraft::load(pick(), without_member.to_vec());
    let next = answer_everything(&mut form, 1);
    assert_eq!(next, 2, "only the group was asked");
    assert_eq!(form.list(2), Some(&ListState::Due));

    form.set(2, Some(String::new()));
    let query = form
        .next_query(editing(), next)
        .expect("member is wanted now");
    assert_eq!(query.field, 2);
}
