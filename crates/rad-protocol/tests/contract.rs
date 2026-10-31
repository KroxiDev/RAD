use rad_authority::Authority;
use rad_core::configuration::*;
use rad_core::{
    configuration::{Configuration, Origin},
    identity::IdentityRegistry,
};
use rad_protocol::{IncomingContext, Protocol, Resolution};
use serde_json::json;
use std::num::NonZeroU128;

fn actor() -> Authority {
    Authority::new(
        Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap(),
        1,
        NonZeroU128::new(7).unwrap(),
    )
}
fn call(a: &mut Authority, origin: Origin, op: &str, args: serde_json::Value) -> Resolution {
    let mut wire = rad_wire::Session::default();
    fn frame(value: serde_json::Value) -> Vec<u8> {
        let v = serde_json::to_vec(&value).unwrap();
        let mut b = (v.len() as u32).to_be_bytes().to_vec();
        b.extend(v);
        b
    }
    let b = frame(json!({"v":1,"id":"1","op":"hello","args":{}}));
    wire.accept(rad_wire::read_frame(&b).unwrap().unwrap())
        .unwrap();
    let b = frame(json!({"v":1,"id":"2","op":op,"args":args}));
    let req = wire
        .accept(rad_wire::read_frame(&b).unwrap().unwrap())
        .unwrap();
    Protocol::new(NonZeroU128::new(7).unwrap()).handle(
        a,
        IncomingContext {
            request_id: 2,
            connection: 4,
            origin,
        },
        &req,
    )
}
fn response(r: Resolution) -> rad_api::response::Response {
    match r {
        Resolution::Immediate { response, .. } => *response,
        _ => panic!("expected response"),
    }
}

fn apply(a: &mut Authority, op: &str, args: serde_json::Value) {
    let Resolution::Command(cmd) = call(a, Origin::Editor, op, args) else {
        panic!("expected {op} command")
    };
    let t = a.reserve(4, Origin::Editor).unwrap();
    let request = t.request();
    let effects = a.submit(t, cmd);
    let (id, revision) = effects
        .iter()
        .find_map(|e| match e {
            rad_authority::Effect::Persist { id, .. } => {
                Some((*id, a.editor_view(Origin::Editor).unwrap().revision + 1))
            }
            _ => None,
        })
        .expect("persist");
    a.complete(
        id,
        rad_authority::Outcome::Applied {
            revision,
            changed: true,
        },
    );
    a.finish(request);
}

#[test]
fn queued_ids_resolve_at_authority_turn_and_batch_failure_has_no_partial_publication() {
    let mut a = actor();
    let Resolution::Command(first) = call(
        &mut a,
        Origin::Editor,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"before"}),
    ) else {
        panic!()
    };
    let t = a.reserve(4, Origin::Editor).unwrap();
    let first_id = t.request();
    let e = a.submit(t, first);
    let rad_authority::Effect::Persist { id, .. } = e[0] else {
        panic!()
    };
    let Resolution::Command(second) = call(
        &mut a,
        Origin::Automation,
        "edit",
        json!({"changes":[{"kind":"set","id":"b1","field":"name","value":"after"}]}),
    ) else {
        panic!()
    };
    let t = a.reserve(4, Origin::Automation).unwrap();
    let second_id = t.request();
    assert!(a.submit(t, second).is_empty());
    let e = a.complete(
        id,
        rad_authority::Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    a.finish(first_id);
    let (commit, candidate) = e
        .iter()
        .find_map(|e| match e {
            rad_authority::Effect::Persist { id, candidate, .. } => Some((*id, candidate)),
            _ => None,
        })
        .unwrap();
    assert_eq!(candidate.configuration().data().buttons[0].name, "after");
    let e = a.complete(
        commit,
        rad_authority::Outcome::Applied {
            revision: 3,
            changed: true,
        },
    );
    let p = Protocol::new(NonZeroU128::new(7).unwrap());
    let c = IncomingContext {
        request_id: 2,
        connection: 4,
        origin: Origin::Automation,
    };
    let r = e
        .iter()
        .find_map(|e| p.response_for(&a, c, second_id, e))
        .unwrap();
    assert_eq!(r.status, rad_api::response::Status::Applied);
    rad_api::Response::decode_value(
        serde_json::to_value(&r).unwrap(),
        2,
        None,
        rad_api::Operation::Edit,
    )
    .unwrap();
    a.finish(second_id);
    let Resolution::Command(cmd) = call(
        &mut a,
        Origin::Automation,
        "edit",
        json!({"changes":[{"kind":"set","id":"b1","field":"name","value":"bad"},{"kind":"set","id":"b999","field":"name","value":"bad"}]}),
    ) else {
        panic!()
    };
    let t = a.reserve(4, Origin::Automation).unwrap();
    let expected = t.request();
    let e = a.submit(t, cmd);
    assert!(matches!(
        e.as_slice(),
        [rad_authority::Effect::Rejected { .. }]
    ));
    let r = p.response_for(&a, c, expected, &e[0]).unwrap();
    assert_eq!(
        r.error.unwrap().outcome,
        Some(rad_api::response::Outcome::NotApplied)
    );
    assert!(p.response_for(&a, c, second_id, &e[0]).is_none());
    assert_eq!(
        serde_json::to_value(
            response(call(&mut a, Origin::Editor, "get", json!({"id":"b1"}))).data
        )
        .unwrap()["fields"]["name"],
        "after"
    );
}

#[test]
fn pages_are_bounded_and_cursor_expires_after_visible_publication() {
    use rad_core::configuration::Edit;
    let mut c = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let folder = c.configuration().data().principal;
    for position in 0..20 {
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::CreateButton {
                    folder,
                    position,
                    name: "n".repeat(4000),
                },
            )
            .unwrap();
    }
    let mut a = Authority::new(c, 1, NonZeroU128::new(7).unwrap());
    let page = response(call(
        &mut a,
        Origin::Editor,
        "list",
        json!({"cursor":null,"limit":32}),
    ));
    assert!(serde_json::to_vec(&page).unwrap().len() <= 49152);
    let count = serde_json::to_value(&page.data).unwrap()["items"]
        .as_array()
        .unwrap()
        .len();
    assert!(count > 0 && count < 20);
    let cursor = serde_json::to_value(&page.data).unwrap()["next_cursor"].clone();
    assert!(cursor.is_string());
    let rest = response(call(
        &mut a,
        Origin::Editor,
        "list",
        json!({"cursor":cursor,"limit":32}),
    ));
    assert_eq!(
        serde_json::to_value(&rest.data).unwrap()["items"]
            .as_array()
            .unwrap()
            .len()
            + count,
        20
    );
    apply(
        &mut a,
        "edit",
        json!({"changes":[{"kind":"set","id":"b1","field":"name","value":"changed"}]}),
    );
    assert_eq!(
        response(call(
            &mut a,
            Origin::Editor,
            "list",
            json!({"cursor":cursor,"limit":32})
        ))
        .error
        .unwrap()
        .code,
        "cursor_expired"
    );
}
#[test]
fn permissions_and_hidden_actions_are_projected_without_payload_or_parent_leaks() {
    use rad_core::permissions::{LocalPermissions, Permissions};
    let mut c = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "hidden".into(),
            },
        )
        .unwrap();
    let button = c.configuration().data().buttons[0].binding.identity();
    c = c
        .configuration()
        .prepare_edits(
            c.registry(),
            Origin::Editor,
            &[
                Edit::AddAction {
                    button,
                    kind: ActionKind::Launcher(DesktopEntry::new("safe.desktop".into()).unwrap()),
                },
                Edit::SetButtonPermissions {
                    button,
                    permissions: LocalPermissions::own(
                        Permissions::new(false, false, false).unwrap(),
                    ),
                },
            ],
        )
        .unwrap();
    let action = c.configuration().data().buttons[0].actions[0]
        .binding
        .identity();
    c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetActionPermissions {
                action,
                permissions: LocalPermissions::own(Permissions::new(true, false, false).unwrap()),
            },
        )
        .unwrap();
    let mut a = Authority::new(c, 1, NonZeroU128::new(7).unwrap());
    let child = response(call(&mut a, Origin::Automation, "get", json!({"id":"a1"})));
    assert_eq!(
        serde_json::to_value(&child.data).unwrap()["fields"]["parameters"],
        json!({"desktop_entry":"safe.desktop"})
    );
    assert!(serde_json::to_value(&child.data).unwrap()["fields"]["parent_id"].is_null());
    assert_eq!(
        response(call(&mut a, Origin::Automation, "get", json!({"id":"b1"})))
            .error
            .unwrap()
            .code,
        "not_found"
    );
    assert_eq!(
        serde_json::to_value(
            response(call(&mut a, Origin::Automation, "get", json!({"id":"f1"}))).data
        )
        .unwrap()["fields"]["slots"][0],
        json!({"visibility":"restricted"})
    );
    let editor = response(call(&mut a, Origin::Editor, "get", json!({"id":"a1"})));
    assert_eq!(
        serde_json::to_value(&editor.data).unwrap()["fields"]["parent_id"],
        "b1"
    );
}
#[test]
fn history_commands_and_uncertain_reply_preserve_correlation_and_outcome() {
    let mut a = actor();
    assert!(matches!(
        call(&mut a, Origin::Editor, "undo", json!({})),
        Resolution::Command(rad_authority::Command::Undo)
    ));
    assert!(matches!(
        call(&mut a, Origin::Editor, "redo", json!({})),
        Resolution::Command(rad_authority::Command::Redo)
    ));
    assert_eq!(
        response(call(&mut a, Origin::Automation, "undo", json!({})))
            .error
            .unwrap()
            .outcome,
        Some(rad_api::response::Outcome::NotApplied)
    );
    let Resolution::Command(cmd) = call(
        &mut a,
        Origin::Editor,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"x"}),
    ) else {
        panic!()
    };
    let t = a.reserve(4, Origin::Editor).unwrap();
    let expected = t.request();
    let e = a.submit(t, cmd);
    let rad_authority::Effect::Persist { id, .. } = e[0] else {
        panic!()
    };
    let e = a.complete(id, rad_authority::Outcome::Uncertain);
    let p = Protocol::new(NonZeroU128::new(7).unwrap());
    let c = IncomingContext {
        request_id: 2,
        connection: 4,
        origin: Origin::Editor,
    };
    let r = e
        .iter()
        .find_map(|e| p.response_for(&a, c, expected, e))
        .unwrap();
    assert_eq!(r.error.as_ref().unwrap().code, "storage_uncertain");
    assert_eq!(
        r.error.as_ref().unwrap().outcome,
        Some(rad_api::response::Outcome::Unknown)
    );
    rad_api::Response::decode_value(
        rad_wire::parse_json(&p.encode_response(&r).unwrap()).unwrap(),
        2,
        None,
        rad_api::Operation::Edit,
    )
    .unwrap();
    assert_eq!(
        response(call(&mut a, Origin::Editor, "get", json!({"id":"b1"})))
            .error
            .unwrap()
            .code,
        "not_found"
    );
}

#[test]
fn credential_operations_are_editor_only_and_secrets_are_redacted() {
    let mut a = actor();
    for (op, args) in [
        (
            "credential_status",
            json!({"plugin_id":"fixture","slot":"password"}),
        ),
        (
            "credential_remove",
            json!({"plugin_id":"fixture","slot":"password"}),
        ),
        (
            "credential_replace",
            json!({"plugin_id":"fixture","slot":"password","secret":"private-canary"}),
        ),
    ] {
        let denied = response(call(&mut a, Origin::Automation, op, args.clone()));
        assert_eq!(denied.error.unwrap().code, "permission_denied");
        let Resolution::External(operation) = call(&mut a, Origin::Editor, op, args) else {
            panic!("editor operation must reach private runtime worker")
        };
        assert!(!format!("{operation:?}").contains("private-canary"));
    }
}

#[test]
fn board_is_one_revision_and_runtime_describes_the_grid() {
    let mut a = actor();
    let r = response(call(
        &mut a,
        Origin::Editor,
        "board",
        json!({"folder_id":null}),
    ));
    assert!(r.error.is_none(), "board query rejected: {:?}", r.error);
    assert_eq!(
        serde_json::to_value(&r.data).unwrap()["grid"],
        json!({"columns":8,"rows":4})
    );
    assert_eq!(
        serde_json::to_value(&r.data).unwrap()["keys"]
            .as_array()
            .unwrap()
            .len(),
        32
    );
    assert_eq!(
        serde_json::to_value(&r.data).unwrap()["folder"]["view_revision"],
        serde_json::to_value(&r.data).unwrap()["view_revision"]
    );
}
