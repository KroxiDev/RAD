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

fn apply(a: &mut Authority, op: &str, args: serde_json::Value) -> ConfigurationData {
    let Resolution::Command(cmd) = call(a, Origin::Editor, op, args) else {
        panic!("expected {op} command")
    };
    let t = a.reserve(4, Origin::Editor).unwrap();
    let request = t.request();
    let effects = a.submit(t, cmd);
    let (id, revision, data) = effects
        .iter()
        .find_map(|e| match e {
            rad_authority::Effect::Persist { id, candidate, .. } => Some((
                *id,
                a.editor_view(Origin::Editor).unwrap().revision + 1,
                candidate.configuration().data().clone(),
            )),
            _ => None,
        })
        .unwrap_or_else(|| panic!("persist for {op}: {effects:?}"));
    a.complete(
        id,
        rad_authority::Outcome::Applied {
            revision,
            changed: true,
        },
    );
    a.finish(request);
    data
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

#[test]
fn linked_action_visual_and_faces_publish_and_undo_as_one_edit() {
    let mut a = actor();
    let manifest =
        rad_plugin_contract::Document::new(include_bytes!("../../../plugins/audio/manifest.json"))
            .unwrap()
            .manifest(1, "x86_64-unknown-linux-gnu")
            .unwrap();
    a.register_plugin(manifest, "x86_64-unknown-linux-gnu")
        .unwrap();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"Mic"}),
    );
    let before = apply(
        &mut a,
        "set_permissions",
        json!({"target":"b1","value":{"own":{"consult":true,"modify":false,"execute":false}}}),
    );
    let params = json!({"schema_version":1,"values":{"device":"@default","operation":"toggle"}});
    let changes = json!([{"kind":"add_action","button_id":"b1","type_key":"org.rad.audio:microphone","parameters":params},
       {"kind":"set","id":"b1","field":"visual","value":{"type":"org.rad.audio#microphone","parameters":{"schema_version":1,"values":{"device":"@default"}}}},
       {"kind":"set","id":"b1","field":"appearance","value":{"text":"","image":null,"states":{"muted":{"text":"Quiet","image":null},"unmuted":{"text":"Live","image":null}}}}]);
    apply(&mut a, "edit", json!({"changes":changes}));
    let view = serde_json::to_value(
        response(call(
            &mut a,
            Origin::Editor,
            "inspector",
            json!({"id":"b1"}),
        ))
        .data,
    )
    .unwrap();
    assert_eq!(view["appearance"]["states"]["muted"]["text"], "Quiet");
    assert_eq!(
        view["appearance"]["visual_type"],
        "org.rad.audio#microphone"
    );
    let Resolution::Command(command) = call(
        &mut a,
        Origin::Automation,
        "edit",
        json!({"changes":[{"kind":"remove_action","id":"a1"}]}),
    ) else {
        panic!()
    };
    let ticket = a.reserve(4, Origin::Automation).unwrap();
    let id = ticket.request();
    let effects = a.submit(ticket, command);
    assert!(
        effects
            .iter()
            .all(|e| !matches!(e, rad_authority::Effect::Persist { .. }))
    );
    a.finish(id);
    let Resolution::Command(command) = call(&mut a, Origin::Editor, "undo", json!({})) else {
        panic!()
    };
    let ticket = a.reserve(4, Origin::Editor).unwrap();
    let request = ticket.request();
    let effects = a.submit(ticket, command);
    let rad_authority::Effect::ReadRevision { id, revision: 3 } = effects[0] else {
        panic!("{effects:?}")
    };
    let effects = a.loaded(id, Ok(before));
    let rad_authority::Effect::Persist { id, .. } = effects[0] else {
        panic!("{effects:?}")
    };
    a.complete(
        id,
        rad_authority::Outcome::Applied {
            revision: 5,
            changed: true,
        },
    );
    a.finish(request);
    let view = serde_json::to_value(
        response(call(
            &mut a,
            Origin::Editor,
            "inspector",
            json!({"id":"b1"}),
        ))
        .data,
    )
    .unwrap();
    assert!(view["appearance"]["states"].as_object().unwrap().is_empty());
    assert!(view["appearance"]["visual_type"].is_null());
}

#[test]
fn conditional_edit_checks_view_at_its_turn_without_consuming_action_ids() {
    let mut a = actor();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"Before"}),
    );
    let revision = a.view_revision(Origin::Editor).unwrap().value().to_string();
    let Resolution::Command(first) = call(
        &mut a,
        Origin::Editor,
        "edit",
        json!({"changes":[{"kind":"set","id":"b1","field":"name","value":"Concurrent"}]}),
    ) else {
        panic!()
    };
    let ticket = a.reserve(4, Origin::Editor).unwrap();
    let first_request = ticket.request();
    let effects = a.submit(ticket, first);
    let rad_authority::Effect::Persist { id, .. } = effects[0] else {
        panic!()
    };
    let Resolution::Command(second) = call(
        &mut a,
        Origin::Editor,
        "edit",
        json!({"expected_revision":revision,"changes":[{"kind":"add_action","button_id":"b1","type_key":"wait","parameters":{"milliseconds":10}}]}),
    ) else {
        panic!("conditional edit must parse")
    };
    let ticket = a.reserve(4, Origin::Editor).unwrap();
    let second_request = ticket.request();
    assert!(a.submit(ticket, second).is_empty());
    let effects = a.complete(
        id,
        rad_authority::Outcome::Applied {
            revision: 3,
            changed: true,
        },
    );
    assert!(effects.iter().any(|e| matches!(e, rad_authority::Effect::Rejected { request, reason: rad_authority::Rejection::ViewChanged } if *request == second_request)));
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, rad_authority::Effect::Persist { .. }))
    );
    a.finish(first_request);
    a.finish(second_request);
    let data = apply(
        &mut a,
        "add_action",
        json!({"button_id":"b1","type":"wait","parameters":{"milliseconds":10}}),
    );
    assert_eq!(data.buttons[0].actions[0].binding.short().number(), 1);
}

#[test]
fn conditional_edit_rejects_visible_changes_even_without_a_durable_change() {
    let mut a = actor();
    let revision = a.view_revision(Origin::Editor).unwrap().value().to_string();
    let durable = a.editor_view(Origin::Editor).unwrap().revision;
    a.installed_types(vec![
        ExternalType::new("fixture".into(), "action".into()).unwrap(),
    ]);
    assert_eq!(a.editor_view(Origin::Editor).unwrap().revision, durable);
    let Resolution::Command(command) = call(
        &mut a,
        Origin::Editor,
        "edit",
        json!({"expected_revision":revision,"changes":[{"kind":"set","id":"f1","field":"name","value":"Must not change"}]}),
    ) else {
        panic!("conditional edit must parse")
    };
    let ticket = a.reserve(4, Origin::Editor).unwrap();
    let effects = a.submit(ticket, command);
    assert!(matches!(
        effects.as_slice(),
        [rad_authority::Effect::Rejected {
            reason: rad_authority::Rejection::ViewChanged,
            ..
        }]
    ));
}

#[test]
fn changing_visual_identity_clears_faces_atomically_but_parameters_preserve_them() {
    let mut a = actor();
    let manifest =
        rad_plugin_contract::Document::new(include_bytes!("../../../plugins/audio/manifest.json"))
            .unwrap()
            .manifest(1, "x86_64-unknown-linux-gnu")
            .unwrap();
    a.register_plugin(manifest, "x86_64-unknown-linux-gnu")
        .unwrap();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"Mic"}),
    );
    let visual = |device: &str| {
        json!({"kind":"set","id":"b1","field":"visual","value":{
        "type":"org.rad.audio#microphone","parameters":{"schema_version":1,"values":{"device":device}}}})
    };
    let faces = json!({"muted":{"text":"Quiet","image":"old-face"}});
    apply(
        &mut a,
        "edit",
        json!({"changes":[visual("@default"),
        {"kind":"set","id":"b1","field":"appearance","value":{"text":"Base","image":null,"states":faces}}]}),
    );
    let same_type = apply(&mut a, "edit", json!({"changes":[visual("other-device")]}));
    assert_eq!(
        same_type.buttons[0].appearance.states["muted"]
            .image
            .as_deref(),
        Some("old-face")
    );
    let manual = apply(
        &mut a,
        "edit",
        json!({"changes":[
        {"kind":"set","id":"b1","field":"visual","value":{"type":"manual"}}]}),
    );
    assert!(
        manual.buttons[0].appearance.states.is_empty(),
        "Manual must not retain hidden state media"
    );
}

#[test]
fn appearance_omission_preserves_faces_empty_clears_and_null_is_invalid() {
    let mut a = actor();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"Faces"}),
    );
    let edit =
        |value| json!({"changes":[{"kind":"set","id":"b1","field":"appearance","value":value}]});
    apply(
        &mut a,
        "edit",
        edit(json!({"text":"","image":null,"states":{"off":{"text":"Quiet","image":"face"}}})),
    );
    let preserved = apply(
        &mut a,
        "edit",
        edit(json!({"text":"New base","image":null})),
    );
    assert_eq!(
        preserved.buttons[0].appearance.states["off"]
            .image
            .as_deref(),
        Some("face")
    );
    let cleared = apply(
        &mut a,
        "edit",
        edit(json!({"text":"New base","image":null,"states":{}})),
    );
    assert!(cleared.buttons[0].appearance.states.is_empty());
    let invalid = response(call(
        &mut a,
        Origin::Editor,
        "edit",
        edit(json!({"text":"","image":null,"states":null})),
    ));
    assert_eq!(invalid.error.unwrap().code, "invalid_request");
}

#[test]
fn visual_change_and_omitted_faces_compose_in_both_orders_and_undo_restores_faces() {
    for visual_first in [true, false] {
        let mut a = actor();
        let manifest = rad_plugin_contract::Document::new(include_bytes!(
            "../../../plugins/audio/manifest.json"
        ))
        .unwrap()
        .manifest(1, "x86_64-unknown-linux-gnu")
        .unwrap();
        a.register_plugin(manifest, "x86_64-unknown-linux-gnu")
            .unwrap();
        apply(
            &mut a,
            "create_button",
            json!({"folder_id":"f1","position":0,"name":"Mic"}),
        );
        let before = apply(
            &mut a,
            "edit",
            json!({"changes":[
                {"kind":"set","id":"b1","field":"visual","value":{"type":"org.rad.audio#microphone","parameters":{"schema_version":1,"values":{"device":"@default"}}}},
                {"kind":"set","id":"b1","field":"appearance","value":{"text":"Base","image":null,"states":{"muted":{"text":"Quiet","image":"old-face"}}}}
            ]}),
        );
        let visual = json!({"kind":"set","id":"b1","field":"visual","value":{"type":"manual"}});
        let appearance = json!({"kind":"set","id":"b1","field":"appearance","value":{"text":"New base","image":null}});
        let changes = if visual_first {
            vec![visual, appearance]
        } else {
            vec![appearance, visual]
        };
        let after = apply(&mut a, "edit", json!({"changes":changes}));
        assert!(after.buttons[0].appearance.states.is_empty());
        assert_eq!(after.buttons[0].appearance.text, "New base");
        let Resolution::Command(command) = call(&mut a, Origin::Editor, "undo", json!({})) else {
            panic!()
        };
        let ticket = a.reserve(4, Origin::Editor).unwrap();
        let request = ticket.request();
        let effects = a.submit(ticket, command);
        let rad_authority::Effect::ReadRevision { id, .. } = effects[0] else {
            panic!()
        };
        let effects = a.loaded(id, Ok(before.clone()));
        let rad_authority::Effect::Persist {
            id, ref candidate, ..
        } = effects[0]
        else {
            panic!()
        };
        assert_eq!(
            candidate.configuration().data().buttons[0].appearance,
            before.buttons[0].appearance
        );
        a.complete(
            id,
            rad_authority::Outcome::Applied {
                revision: 5,
                changed: true,
            },
        );
        a.finish(request);
    }
}

#[test]
fn captured_state_appearance_cannot_publish_after_visual_changes() {
    let mut a = actor();
    let manifest =
        rad_plugin_contract::Document::new(include_bytes!("../../../plugins/audio/manifest.json"))
            .unwrap()
            .manifest(1, "x86_64-unknown-linux-gnu")
            .unwrap();
    a.register_plugin(manifest, "x86_64-unknown-linux-gnu")
        .unwrap();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"Mic"}),
    );
    apply(
        &mut a,
        "edit",
        json!({"changes":[
            {"kind":"set","id":"b1","field":"visual","value":{"type":"org.rad.audio#microphone","parameters":{"schema_version":1,"values":{"device":"@default"}}}}
        ]}),
    );
    let revision = a.view_revision(Origin::Editor).unwrap().value().to_string();
    let after = apply(
        &mut a,
        "edit",
        json!({"changes":[{"kind":"set","id":"b1","field":"visual","value":{"type":"manual"}}]}),
    );
    let Resolution::Command(command) = call(
        &mut a,
        Origin::Editor,
        "edit",
        json!({"expected_revision":revision,"changes":[
            {"kind":"set","id":"b1","field":"appearance","value":{"text":"","image":null,"states":{"muted":{"text":"old","image":"late-upload"}}}}
        ]}),
    ) else {
        panic!()
    };
    let ticket = a.reserve(4, Origin::Editor).unwrap();
    let request = ticket.request();
    let effects = a.submit(ticket, command);
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, rad_authority::Effect::Persist { .. }))
    );
    let view = serde_json::to_value(
        response(call(
            &mut a,
            Origin::Editor,
            "inspector",
            json!({"id":"b1"}),
        ))
        .data,
    )
    .unwrap();
    assert!(view["appearance"]["states"].as_object().unwrap().is_empty());
    assert!(after.buttons[0].appearance.states.is_empty());
    a.finish(request);
}

#[test]
fn different_plugin_or_visual_does_not_inherit_faces_even_when_state_ids_match() {
    let mut a = actor();
    for bytes in [
        include_bytes!("../../../plugins/audio/manifest.json").as_slice(),
        include_bytes!("../../../plugins/obs/manifest.json").as_slice(),
    ] {
        let manifest = rad_plugin_contract::Document::new(bytes)
            .unwrap()
            .manifest(1, "x86_64-unknown-linux-gnu")
            .unwrap();
        a.register_plugin(manifest, "x86_64-unknown-linux-gnu")
            .unwrap();
    }
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"Key"}),
    );
    for (kind, values, first, second) in [
        (
            "org.rad.audio#microphone",
            json!({"device":"@default"}),
            "muted",
            "unmuted",
        ),
        (
            "org.rad.obs#mute",
            json!({"input":"input"}),
            "muted",
            "unmuted",
        ),
        ("org.rad.obs#record", json!({}), "recording", "stopped"),
    ] {
        let switched = apply(
            &mut a,
            "edit",
            json!({"changes":[{"kind":"set","id":"b1","field":"visual","value":{"type":kind,"parameters":{"schema_version":1,"values":values}}}]}),
        );
        assert!(
            switched.buttons[0].appearance.states.is_empty(),
            "{kind} inherited another visual's images"
        );
        let saved = apply(
            &mut a,
            "edit",
            json!({"changes":[{"kind":"set","id":"b1","field":"appearance","value":{"text":"","image":null,"states":{
                (first):{"text":"First","image":"first-image"},(second):{"text":"Resting","image":"second-image"}
            }}}]}),
        );
        assert_eq!(saved.buttons[0].appearance.states.len(), 2);
    }
}

#[test]
fn text_style_fields_update_preserve_and_reset_independently() {
    let mut a = actor();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"Style"}),
    );
    let edit =
        |value| json!({"changes":[{"kind":"set","id":"b1","field":"appearance","value":value}]});
    apply(
        &mut a,
        "edit",
        edit(
            json!({"text":"Saved","image":null,"text_position":"hidden","text_size":24,"text_color":"#1234AB","text_band":false}),
        ),
    );
    apply(
        &mut a,
        "edit",
        edit(json!({"text":"Still saved","image":null})),
    );
    let view = Protocol::new(NonZeroU128::new(7).unwrap())
        .inspector_view(
            &a,
            IncomingContext {
                request_id: 2,
                connection: 4,
                origin: Origin::Editor,
            },
            "b1",
        )
        .unwrap();
    let appearance = serde_json::to_value(view.appearance.unwrap()).unwrap();
    assert_eq!(appearance["text_position"], "hidden");
    assert_eq!(appearance["text_position_automatic"], false);
    assert_eq!(appearance["text_size"], 24);
    assert_eq!(appearance["text_color"], "#1234AB");
    assert_eq!(appearance["text_band"], false);
    for (field, expected) in [
        ("text_size", json!(13)),
        ("text_color", json!("#FFFFFF")),
        ("text_band", json!(true)),
        ("text_position", json!("middle")),
    ] {
        let mut value = json!({"text":"Still saved","image":null});
        value[field] = serde_json::Value::Null;
        apply(&mut a, "edit", edit(value));
        let view = Protocol::new(NonZeroU128::new(7).unwrap())
            .inspector_view(
                &a,
                IncomingContext {
                    request_id: 2,
                    connection: 4,
                    origin: Origin::Editor,
                },
                "b1",
            )
            .unwrap();
        let appearance = serde_json::to_value(view.appearance.unwrap()).unwrap();
        assert_eq!(appearance[field], expected);
    }
}

#[test]
fn full_state_style_map_preserves_other_face_and_omitted_fields_default() {
    let mut a = actor();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"Faces"}),
    );
    let edit = |states| json!({"changes":[{"kind":"set","id":"b1","field":"appearance","value":{"text":"Base","image":null,"states":states}}]});
    let faces = json!({"on":{"text":"On","image":null,"text_position":"hidden","text_size":24,"text_color":"#010203","text_band":false},"off":{"text":"Off","image":null,"text_position":"top","text_size":8,"text_color":"#AABBCC","text_band":true}});
    let before = apply(&mut a, "edit", edit(faces.clone()));
    let mut changed = faces;
    changed["on"]["text_size"] = 16.into();
    let after = apply(&mut a, "edit", edit(changed));
    assert_eq!(
        after.buttons[0].appearance.states["off"],
        before.buttons[0].appearance.states["off"]
    );
    assert_eq!(after.buttons[0].appearance.states["on"].text_style.size, 16);
    let defaults = apply(
        &mut a,
        "edit",
        edit(json!({"on":{"text":"On","image":null},"off":{"text":"Off","image":null}})),
    );
    for face in defaults.buttons[0].appearance.states.values() {
        assert_eq!(face.text_style, TextStyle::default());
    }
    let view = Protocol::new(NonZeroU128::new(7).unwrap())
        .inspector_view(
            &a,
            IncomingContext {
                request_id: 2,
                connection: 4,
                origin: Origin::Editor,
            },
            "b1",
        )
        .unwrap()
        .appearance
        .unwrap();
    assert!(view.states["on"].text_position_automatic);
    assert_eq!(
        view.states["on"].text_position,
        rad_api::TextPosition::Middle
    );
}

#[test]
fn invalid_style_values_are_rejected_before_publication() {
    let mut a = actor();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"Style"}),
    );
    for state in [false, true] {
        for (field, value) in [
            ("text_size", json!(7)),
            ("text_size", json!(25)),
            ("text_color", json!("#GGFFFF")),
            ("text_color", json!("red")),
        ] {
            let mut face = json!({"text":"Saved","image":null});
            face[field] = value;
            let value = if state {
                json!({"text":"Base","image":null,"states":{"on":face}})
            } else {
                face
            };
            let result = response(call(
                &mut a,
                Origin::Editor,
                "edit",
                json!({"changes":[{"kind":"set","id":"b1","field":"appearance","value":value}]}),
            ));
            assert_eq!(result.error.unwrap().code, "invalid_request");
        }
    }
}
