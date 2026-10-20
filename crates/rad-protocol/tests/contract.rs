use rad_authority::Authority;
use rad_core::{
    configuration::{Configuration, Origin},
    identity::IdentityRegistry,
};
use rad_protocol::{IncomingContext, Protocol, Resolution};
use serde_json::{Value, json};
use std::num::NonZeroU128;
#[test]
fn hello_only_announces_implemented_operations_and_correlates_session() {
    let value = serde_json::to_vec(&json!({"v":1,"id":"1","op":"hello","args":{}})).unwrap();
    let mut bytes = (value.len() as u32).to_be_bytes().to_vec();
    bytes.extend(value);
    let request = rad_wire::Session::default()
        .accept(rad_wire::read_frame(&bytes).unwrap().unwrap())
        .unwrap();
    let session = NonZeroU128::new(7).unwrap();
    let candidate = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let mut authority = Authority::new(candidate, 1, session);
    let result = Protocol::new(session).handle(
        &mut authority,
        IncomingContext {
            request_id: 1,
            connection: 4,
            origin: Origin::Automation,
        },
        &request,
    );
    let Resolution::Immediate { response, effects } = result else {
        panic!("hello must be immediate")
    };
    assert!(effects.is_empty());
    assert_eq!(response.data["protocol"], 1);
    assert_eq!(
        response.data["session_id"],
        "00000000000000000000000000000007"
    );
    assert!(
        response.data["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("status"))
    );
    assert!(
        !response.data["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("transfer_begin"))
    );
    assert!(
        !response.data["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("create_button"))
    );
    assert_eq!(response.data["limits"]["frame_bytes"], 65536);
    rad_wire::response::Response::decode(&serde_json::to_vec(&response).unwrap(), 1, None).unwrap();
}
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
fn response(r: Resolution) -> rad_wire::response::Response {
    match r {
        Resolution::Immediate { response, .. } => response,
        _ => panic!("expected response"),
    }
}
#[test]
fn status_is_origin_filtered_and_closed_dto_rejects_role() {
    let mut a = actor();
    let r = response(call(&mut a, Origin::Editor, "status", json!({})));
    assert_eq!(r.data["active_folder_id"], "f1");
    assert_eq!(r.data["settings"]["brightness_percent"], 100);
    let r = response(call(&mut a, Origin::Automation, "status", json!({})));
    assert!(r.data.get("settings").is_none());
    assert!(r.data["active_folder_id"].is_null());
    let r = response(call(
        &mut a,
        Origin::Automation,
        "status",
        json!({"role":"editor"}),
    ));
    assert_eq!(r.status, rad_wire::response::Status::Error);
}
#[test]
fn visible_get_list_and_sections_share_revision_and_restricted_markers() {
    use rad_core::configuration::*;
    let c = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 2,
                name: "Test".into(),
            },
        )
        .unwrap();
    let mut a = Authority::new(c, 1, NonZeroU128::new(7).unwrap());
    let b = response(call(&mut a, Origin::Editor, "get", json!({"id":"b1"})));
    assert_eq!(b.data["fields"]["name"], "Test");
    assert_eq!(b.data["sections"], json!(["appearance", "actions"]));
    let f = response(call(&mut a, Origin::Editor, "get", json!({"id":"f1"})));
    assert_eq!(f.data["fields"]["slots"][2]["id"], "b1");
    let list = response(call(
        &mut a,
        Origin::Editor,
        "list",
        json!({"cursor":null,"limit":32}),
    ));
    assert_eq!(list.data["items"][0], b.data);
    assert_eq!(list.data["view_revision"], b.data["view_revision"]);
    let appearance = response(call(
        &mut a,
        Origin::Editor,
        "get_section",
        json!({"id":"b1","section":"appearance","cursor":null}),
    ));
    assert_eq!(
        appearance.data["items"][0],
        json!({"name":"text","value":""})
    );
    let actions = response(call(
        &mut a,
        Origin::Editor,
        "get_section",
        json!({"id":"b1","section":"actions","cursor":null}),
    ));
    assert_eq!(actions.data["items"], json!([]));
    assert_eq!(
        response(call(&mut a, Origin::Editor, "get", json!({"id":"b01"}))).status,
        rad_wire::response::Status::Error
    );
}
#[test]
fn editor_create_is_deferred_and_automation_cannot_administer_even_with_role() {
    let mut a = actor();
    let denied = response(call(
        &mut a,
        Origin::Automation,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"x"}),
    ));
    assert_eq!(
        denied.error.unwrap().outcome,
        Some(rad_wire::response::Outcome::NotApplied)
    );
    let Resolution::Command(cmd) = call(
        &mut a,
        Origin::Editor,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"x"}),
    ) else {
        panic!("command")
    };
    let t = a.reserve(4, Origin::Editor).unwrap();
    let effects = a.submit(t, cmd);
    let rad_authority::Effect::Persist { id, candidate, .. } = &effects[0] else {
        panic!("persist")
    };
    assert_eq!(candidate.configuration().data().buttons[0].name, "x");
    assert!(
        a.get(
            Origin::Editor,
            rad_core::identity::ShortId::new(rad_core::identity::EntityKind::Button, 1).unwrap()
        )
        .is_none()
    );
    a.complete(
        *id,
        rad_authority::Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    assert_eq!(
        response(call(&mut a, Origin::Editor, "get", json!({"id":"b1"}))).data["fields"]["name"],
        "x"
    );
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
fn atomic_edits_resolve_current_shadow_and_support_launcher_plan_and_appearance() {
    let mut a = actor();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"x"}),
    );
    apply(
        &mut a,
        "add_action",
        json!({"button_id":"b1","type":"launcher","parameters":{"desktop_entry":"one.desktop"}}),
    );
    apply(
        &mut a,
        "add_action",
        json!({"button_id":"b1","type":"launcher","parameters":{"desktop_entry":"two.desktop"}}),
    );
    apply(
        &mut a,
        "edit",
        json!({"changes":[{"kind":"set","id":"b1","field":"mode","value":"parallel"},{"kind":"order","id":"b1","indices":[1,0]},{"kind":"set","id":"a1","field":"desktop_entry","value":"new.desktop"},{"kind":"set","id":"b1","field":"appearance","value":{"text":"Hi","image":null}},{"kind":"move","id":"b1","position":3}]}),
    );
    let r = response(call(&mut a, Origin::Editor, "get", json!({"id":"b1"})));
    assert_eq!(r.data["fields"]["mode"], "parallel");
    assert_eq!(r.data["fields"]["position"], 3);
    let r = response(call(
        &mut a,
        Origin::Editor,
        "get_section",
        json!({"id":"b1","section":"actions","cursor":null}),
    ));
    assert_eq!(r.data["items"][0]["id"], "a2");
    assert_eq!(
        r.data["items"][1]["fields"]["parameters"]["desktop_entry"],
        "new.desktop"
    );
}
#[test]
fn administrative_tree_settings_and_permissions_use_domain_commands() {
    let mut a = actor();
    apply(
        &mut a,
        "create_folder",
        json!({"parent_id":"f1","position":0,"name":"Folder"}),
    );
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":1,"name":"x"}),
    );
    apply(
        &mut a,
        "duplicate",
        json!({"id":"b3","folder_id":"f2","position":2}),
    );
    apply(
        &mut a,
        "set_settings",
        json!({"changes":{"brightness_percent":0,"sleep_ms":"0","press_threshold_ms":"18446744073709551615"}}),
    );
    apply(
        &mut a,
        "set_permissions",
        json!({"target":"b3","value":{"own":{"consult":false,"modify":false,"execute":false}}}),
    );
    assert_eq!(
        response(call(&mut a, Origin::Automation, "get", json!({"id":"b3"})))
            .error
            .unwrap()
            .code,
        "not_found"
    );
    let s = response(call(&mut a, Origin::Editor, "status", json!({})));
    assert!(s.data["settings"].get("gui_language").is_none());
    apply(
        &mut a,
        "move_folder",
        json!({"id":"f2","parent_id":"f1","position":3}),
    );
    apply(
        &mut a,
        "delete",
        json!({"id":"b3","confirm_contents":false}),
    );
    apply(&mut a, "delete", json!({"id":"f2","confirm_contents":true}));
    let f = response(call(&mut a, Origin::Editor, "get", json!({"id":"f1"})));
    assert!(
        f.data["fields"]["slots"]
            .as_array()
            .unwrap()
            .iter()
            .all(Value::is_null)
    );
}
#[test]
fn run_is_admission_only_and_navigation_keeps_selection_effects() {
    let mut a = actor();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"x"}),
    );
    let Resolution::Command(cmd) = call(&mut a, Origin::Editor, "run", json!({"id":"b1"})) else {
        panic!("run command")
    };
    let t = a.reserve(4, Origin::Editor).unwrap();
    let expected = t.request();
    let effects = a.submit(t, cmd);
    let p = Protocol::new(NonZeroU128::new(7).unwrap());
    let context = IncomingContext {
        request_id: 2,
        connection: 4,
        origin: Origin::Editor,
    };
    let r = effects
        .iter()
        .find_map(|e| p.response_for(&a, context, expected, e))
        .expect("admitted");
    assert_eq!(r.status, rad_wire::response::Status::Admitted);
    let result = response(call(
        &mut a,
        Origin::Editor,
        "result",
        json!({"execution_id":r.data["execution_id"]}),
    ));
    assert_eq!(result.data["state"], "succeeded");
    let Resolution::Immediate { response, effects } = call(
        &mut a,
        Origin::Editor,
        "navigate",
        json!({"folder_id":"f1"}),
    ) else {
        panic!()
    };
    assert_eq!(response.status, rad_wire::response::Status::Ok);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, rad_authority::Effect::SelectionChanged { .. }))
    );
}
#[test]
fn external_catalog_and_transfers_are_typed_and_enabled_only_by_runtime() {
    use rad_protocol::{Capabilities, External, TransferKind};
    fn request(op: &str, args: Value) -> rad_wire::Request {
        let mut s = rad_wire::Session::default();
        let mut last = None;
        for (i, op, args) in [(1, "hello", json!({})), (2, op, args)] {
            let v =
                serde_json::to_vec(&json!({"v":1,"id":i.to_string(),"op":op,"args":args})).unwrap();
            let mut b = (v.len() as u32).to_be_bytes().to_vec();
            b.extend(v);
            last = Some(
                s.accept(rad_wire::read_frame(&b).unwrap().unwrap())
                    .unwrap(),
            );
        }
        last.unwrap()
    }
    let mut a = actor();
    let p = Protocol::with_capabilities(
        NonZeroU128::new(7).unwrap(),
        Capabilities {
            launcher: true,
            transfers: true,
            media: true,
        },
    );
    let c = IncomingContext {
        request_id: 2,
        connection: 4,
        origin: Origin::Editor,
    };
    match p.handle(
        &mut a,
        c,
        &request("launcher_catalog", json!({"cursor":null,"query":"term"})),
    ) {
        Resolution::External(External::LauncherCatalog { cursor, query }) => {
            assert!(cursor.is_none());
            assert_eq!(query, "term")
        }
        _ => panic!("catalog external"),
    }
    assert_eq!(
        response(p.handle(
            &mut a,
            IncomingContext {
                origin: Origin::Automation,
                ..c
            },
            &request("launcher_catalog", json!({"cursor":null,"query":""}))
        ))
        .error
        .unwrap()
        .code,
        "permission_denied"
    );
    match p.handle(
        &mut a,
        c,
        &request(
            "transfer_begin",
            json!({"kind":"image_import","size":"8388608"}),
        ),
    ) {
        Resolution::External(External::TransferBegin {
            kind: TransferKind::ImageImport,
            size: Some(8388608),
        }) => {}
        _ => panic!("begin"),
    }
    match p.handle(
        &mut a,
        c,
        &request(
            "transfer_write",
            json!({"transfer_id":"t1","offset":"0","base64":"YWJj"}),
        ),
    ) {
        Resolution::External(External::TransferWrite { bytes, .. }) => assert_eq!(bytes, b"abc"),
        _ => panic!("write"),
    }
    assert_eq!(
        response(p.handle(
            &mut a,
            c,
            &request(
                "transfer_begin",
                json!({"kind":"image_import","size":"8388609"})
            )
        ))
        .status,
        rad_wire::response::Status::Error
    );
}
#[test]
fn launcher_schema_is_concrete_without_external_host_and_hello_is_endpoint_specific() {
    let mut a = actor();
    let r = response(call(
        &mut a,
        Origin::Automation,
        "schema",
        json!({"type":"launcher","cursor":null}),
    ));
    assert_eq!(r.data["items"][0]["name"], "desktop_entry");
    assert_eq!(r.data["items"][0]["type"], "string");
    assert_eq!(r.data["items"][0]["required"], true);
    assert_eq!(r.data["next_cursor"], Value::Null);
    let e = response(call(
        &mut a,
        Origin::Editor,
        "schema",
        json!({"type":"external:host","cursor":null}),
    ));
    assert_eq!(e.error.unwrap().code, "executor_unavailable");
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
    assert_eq!(r.status, rad_wire::response::Status::Applied);
    rad_wire::response::Response::decode(&serde_json::to_vec(&r).unwrap(), 2, None).unwrap();
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
        Some(rad_wire::response::Outcome::NotApplied)
    );
    assert!(p.response_for(&a, c, second_id, &e[0]).is_none());
    assert_eq!(
        response(call(&mut a, Origin::Editor, "get", json!({"id":"b1"}))).data["fields"]["name"],
        "after"
    );
}
#[test]
fn scalar_input_limit_is_encoded_bytes_and_output_is_bounded_before_transport() {
    let mut a = actor();
    let too_large = "x".repeat(8191); // quotes make encoded scalar8193
    let r = response(call(
        &mut a,
        Origin::Editor,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":too_large}),
    ));
    assert_eq!(
        r.error.unwrap().outcome,
        Some(rad_wire::response::Outcome::NotApplied)
    );
    let p = Protocol::new(NonZeroU128::new(7).unwrap());
    let mut r = response(call(&mut a, Origin::Editor, "status", json!({})));
    r.data = json!({"huge":"x".repeat(49152)});
    assert!(p.encode_response(&r).is_err());
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
    let count = page.data["items"].as_array().unwrap().len();
    assert!(count > 0 && count < 20);
    let cursor = page.data["next_cursor"].clone();
    assert!(cursor.is_string());
    let rest = response(call(
        &mut a,
        Origin::Editor,
        "list",
        json!({"cursor":cursor,"limit":32}),
    ));
    assert_eq!(rest.data["items"].as_array().unwrap().len() + count, 20);
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
    use rad_core::configuration::*;
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
        child.data["fields"]["parameters"],
        json!({"desktop_entry":"safe.desktop"})
    );
    assert!(child.data["fields"]["parent_id"].is_null());
    assert_eq!(
        response(call(&mut a, Origin::Automation, "get", json!({"id":"b1"})))
            .error
            .unwrap()
            .code,
        "not_found"
    );
    assert_eq!(
        response(call(&mut a, Origin::Automation, "get", json!({"id":"f1"}))).data["fields"]["slots"]
            [0],
        json!({"visibility":"restricted"})
    );
    let editor = response(call(&mut a, Origin::Editor, "get", json!({"id":"a1"})));
    assert_eq!(editor.data["fields"]["parent_id"], "b1");
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
        Some(rad_wire::response::Outcome::NotApplied)
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
        Some(rad_wire::response::Outcome::Unknown)
    );
    rad_wire::response::Response::decode(&p.encode_response(&r).unwrap(), 2, None).unwrap();
    assert_eq!(
        response(call(&mut a, Origin::Editor, "get", json!({"id":"b1"})))
            .error
            .unwrap()
            .code,
        "not_found"
    );
}
#[test]
fn automation_modify_denial_retains_permission_error_at_fifo_turn() {
    let mut a = actor();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"x"}),
    );
    apply(
        &mut a,
        "set_permissions",
        json!({"target":"b1","value":{"own":{"consult":true,"modify":false,"execute":false}}}),
    );
    let Resolution::Command(cmd) = call(
        &mut a,
        Origin::Automation,
        "edit",
        json!({"changes":[{"kind":"set","id":"b1","field":"name","value":"denied"}]}),
    ) else {
        panic!()
    };
    let t = a.reserve(4, Origin::Automation).unwrap();
    let id = t.request();
    let e = a.submit(t, cmd);
    let r = Protocol::new(NonZeroU128::new(7).unwrap())
        .response_for(
            &a,
            IncomingContext {
                request_id: 2,
                connection: 4,
                origin: Origin::Automation,
            },
            id,
            &e[0],
        )
        .unwrap();
    assert_eq!(r.error.unwrap().code, "permission_denied");
}

#[test]
fn unknown_upload_length_is_bounded_by_transfer_worker_not_forbidden_by_protocol() {
    let bytes = br#"{"v":1,"id":"1","op":"hello","args":{}}"#;
    let mut b = (bytes.len() as u32).to_be_bytes().to_vec();
    b.extend(bytes);
    let mut s = rad_wire::Session::default();
    s.accept(rad_wire::read_frame(&b).unwrap().unwrap())
        .unwrap();
    let bytes = br#"{"v":1,"id":"2","op":"transfer_begin","args":{"kind":"restore","size":null}}"#;
    let mut b = (bytes.len() as u32).to_be_bytes().to_vec();
    b.extend(bytes);
    let req = s
        .accept(rad_wire::read_frame(&b).unwrap().unwrap())
        .unwrap();
    let p = Protocol::with_capabilities(
        NonZeroU128::new(7).unwrap(),
        rad_protocol::Capabilities {
            transfers: true,
            ..Default::default()
        },
    );
    assert!(matches!(
        p.handle(
            &mut actor(),
            IncomingContext {
                request_id: 2,
                connection: 4,
                origin: Origin::Editor
            },
            &req
        ),
        Resolution::External(rad_protocol::External::TransferBegin {
            kind: rad_protocol::TransferKind::Restore,
            size: None
        })
    ));
}
fn io_request(op: &str, args: Value) -> rad_wire::Request {
    let mut session = rad_wire::Session::default();
    let mut result = None;
    for (id, op, args) in [(1, "hello", json!({})), (2, op, args)] {
        let bytes =
            serde_json::to_vec(&json!({"v":1,"id":id.to_string(),"op":op,"args":args})).unwrap();
        let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
        frame.extend(bytes);
        result = Some(
            session
                .accept(rad_wire::read_frame(&frame).unwrap().unwrap())
                .unwrap(),
        );
    }
    result.unwrap()
}
#[test]
fn transfer_chunks_are_canonical_bounded_and_media_reads_require_current_entity() {
    use rad_core::configuration::*;
    use rad_protocol::{Capabilities, External};
    let mut c = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "image".into(),
            },
        )
        .unwrap();
    let button = c.configuration().data().buttons[0].binding.identity();
    c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetAppearance {
                button,
                appearance: Appearance {
                    text: "".into(),
                    image: Some("fixture-reference".into()),
                    text_position: None,
                },
            },
        )
        .unwrap();
    let mut a = Authority::new(c, 1, NonZeroU128::new(7).unwrap());
    let p = Protocol::with_capabilities(
        NonZeroU128::new(7).unwrap(),
        Capabilities {
            transfers: true,
            media: true,
            launcher: true,
        },
    );
    let ctx = IncomingContext {
        request_id: 2,
        connection: 4,
        origin: Origin::Editor,
    };
    let exact = format!("{}YWE=", "YWFh".repeat(2730));
    let result = p.handle(
        &mut a,
        ctx,
        &io_request(
            "transfer_write",
            json!({"transfer_id":"t","offset":"0","base64":exact}),
        ),
    );
    assert!(
        matches!(result,Resolution::External(External::TransferWrite{bytes,..}) if bytes.len()==8192)
    );
    for invalid in ["YWFh".repeat(2731), "YR==".into()] {
        assert_eq!(
            response(p.handle(
                &mut a,
                ctx,
                &io_request(
                    "transfer_write",
                    json!({"transfer_id":"t","offset":"0","base64":invalid})
                )
            ))
            .status,
            rad_wire::response::Status::Error
        );
    }
    assert!(
        matches!(p.handle(&mut a,ctx,&io_request("media_read",json!({"id":"b1","offset":"0"}))),Resolution::External(External::MediaRead{media_ref,offset:0,..}) if media_ref=="fixture-reference")
    );
    assert_eq!(
        response(p.handle(
            &mut a,
            ctx,
            &io_request(
                "media_read",
                json!({"media_ref":"fixture-reference","offset":"0"})
            )
        ))
        .error
        .unwrap()
        .code,
        "invalid_request"
    );
    apply(
        &mut a,
        "set_permissions",
        json!({"target":"b1","value":{"own":{"consult":false,"modify":false,"execute":false}}}),
    );
    assert_eq!(
        response(p.handle(
            &mut a,
            IncomingContext {
                origin: Origin::Automation,
                ..ctx
            },
            &io_request("media_read", json!({"id":"b1","offset":"0"}))
        ))
        .error
        .unwrap()
        .code,
        "not_found"
    );
}

#[test]
fn wait_action_is_added_consulted_and_retimed_within_its_bounds() {
    let mut a = actor();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"x"}),
    );
    apply(
        &mut a,
        "add_action",
        json!({"button_id":"b1","type":"launcher","parameters":{"desktop_entry":"one.desktop"}}),
    );
    apply(
        &mut a,
        "add_action",
        json!({"button_id":"b1","type":"wait","parameters":{"milliseconds":1500}}),
    );
    let actions = |a: &mut Authority| {
        response(call(
            a,
            Origin::Editor,
            "get_section",
            json!({"id":"b1","section":"actions","cursor":null}),
        ))
        .data["items"]
            .clone()
    };
    let items = actions(&mut a);
    assert_eq!(items[1]["fields"]["type"], "wait");
    assert_eq!(
        items[1]["fields"]["parameters"],
        json!({"milliseconds":1500})
    );
    assert_eq!(
        items[0]["fields"]["parameters"],
        json!({"desktop_entry":"one.desktop"})
    );
    apply(
        &mut a,
        "edit",
        json!({"changes":[{"kind":"set","id":"a2","field":"milliseconds","value":600000}]}),
    );
    assert_eq!(
        actions(&mut a)[1]["fields"]["parameters"]["milliseconds"],
        600000
    );
    apply(
        &mut a,
        "edit",
        json!({"changes":[{"kind":"set","id":"a2","field":"parameters","value":{"milliseconds":0}}]}),
    );
    assert_eq!(
        actions(&mut a)[1]["fields"]["parameters"]["milliseconds"],
        0
    );
    for (op, args) in [
        (
            "add_action",
            json!({"button_id":"b1","type":"wait","parameters":{"milliseconds":600001}}),
        ),
        (
            "add_action",
            json!({"button_id":"b1","type":"wait","parameters":{"milliseconds":"5"}}),
        ),
        (
            "add_action",
            json!({"button_id":"b1","type":"wait","parameters":{"desktop_entry":"one.desktop"}}),
        ),
        (
            "edit",
            json!({"changes":[{"kind":"set","id":"a2","field":"milliseconds","value":-1}]}),
        ),
        (
            "edit",
            json!({"changes":[{"kind":"set","id":"a2","field":"milliseconds","value":600001}]}),
        ),
    ] {
        let rejected = response(call(&mut a, Origin::Editor, op, args.clone()));
        assert_eq!(rejected.error.unwrap().code, "invalid_request", "{args}");
    }
    let schema = response(call(
        &mut a,
        Origin::Automation,
        "schema",
        json!({"type":"wait","cursor":null}),
    ));
    assert_eq!(
        schema.data["items"],
        json!([{"name":"milliseconds","type":"integer","required":true,"minimum":0,"maximum":600000}])
    );
}

#[test]
fn text_position_is_reported_resolved_and_changed_only_when_the_edit_names_it() {
    let mut a = actor();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"x"}),
    );
    let section = |a: &mut Authority| {
        response(call(
            a,
            Origin::Editor,
            "get_section",
            json!({"id":"b1","section":"appearance","cursor":null}),
        ))
        .data["items"]
            .clone()
    };
    let set = |a: &mut Authority, value: Value| {
        apply(
            a,
            "edit",
            json!({"changes":[{"kind":"set","id":"b1","field":"appearance","value":value}]}),
        )
    };
    // Nothing chosen: centred without an image, at the bottom with one.
    assert_eq!(
        section(&mut a),
        json!([{"name":"text","value":""},{"name":"image","value":null},{"name":"text_position","value":"middle"}])
    );
    set(&mut a, json!({"text":"Go","image":"owned-media"}));
    assert_eq!(section(&mut a)[2]["value"], "bottom");
    set(
        &mut a,
        json!({"text":"Go","image":"owned-media","text_position":"top"}),
    );
    assert_eq!(section(&mut a)[2]["value"], "top");
    // An edit that does not name the position keeps the one chosen before.
    set(&mut a, json!({"text":"Stop","image":null}));
    assert_eq!(section(&mut a)[0]["value"], "Stop");
    assert_eq!(section(&mut a)[2]["value"], "top");
    // Null hands the choice back to the image.
    set(
        &mut a,
        json!({"text":"Stop","image":null,"text_position":null}),
    );
    assert_eq!(section(&mut a)[2]["value"], "middle");
    // Beside the image instead of over it.
    for beside in ["above_image", "below_image"] {
        set(
            &mut a,
            json!({"text":"Stop","image":null,"text_position":beside}),
        );
        assert_eq!(section(&mut a)[2]["value"], beside);
    }
    let rejected = response(call(
        &mut a,
        Origin::Editor,
        "edit",
        json!({"changes":[{"kind":"set","id":"b1","field":"appearance","value":{"text":"x","image":null,"text_position":"left"}}]}),
    ));
    assert_eq!(rejected.error.unwrap().code, "invalid_request");
}

#[test]
fn order_is_only_a_sequence_now_that_gaps_between_actions_are_gone() {
    let mut a = actor();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"x"}),
    );
    for entry in ["one.desktop", "two.desktop"] {
        apply(
            &mut a,
            "add_action",
            json!({"button_id":"b1","type":"launcher","parameters":{"desktop_entry":entry}}),
        );
    }
    apply(
        &mut a,
        "edit",
        json!({"changes":[{"kind":"order","id":"b1","indices":[1,0]}]}),
    );
    let actions = response(call(
        &mut a,
        Origin::Editor,
        "get_section",
        json!({"id":"b1","section":"actions","cursor":null}),
    ));
    assert_eq!(actions.data["items"][0]["id"], "a2");
    let button = response(call(&mut a, Origin::Editor, "get", json!({"id":"b1"})));
    assert!(button.data["fields"].get("pauses_ms").is_none());
    let rejected = response(call(
        &mut a,
        Origin::Editor,
        "edit",
        json!({"changes":[{"kind":"order","id":"b1","indices":[1,0],"pauses_ms":[0]}]}),
    ));
    assert_eq!(rejected.error.unwrap().code, "invalid_request");
}

#[test]
fn plugin_administration_is_editor_only_and_schema_uses_contract_fields() {
    let mut a = actor();
    let manifest: rad_plugin_contract::Manifest = serde_json::from_value(json!({
        "contract":1,"id":"fixture","version":{"major":1,"minor":0,"patch":0},
        "host_min":3,"host_max":3,"targets":["x86_64-unknown-linux-gnu"],
        "actions":[{"id":"run","label":"Run","schema":{"version":1,"fields":[
            {"id":"mode","label":"Mode","required":true,"kind":{"type":"choice","options":["echo","fail"]}}
        ]},"credential_slots":["token"]}],"visuals":[]
    })).unwrap();
    a.install_plugin(manifest.clone(), 1, "x86_64-unknown-linux-gnu")
        .unwrap();
    for operation in ["install", "update", "reinstall", "uninstall", "rollback"] {
        let r = response(call(
            &mut a,
            Origin::Automation,
            "plugin_admin",
            json!({"operation":operation,"plugin_id":"fixture","source":"https://github.com/KroxiDev/rad-plugin-fixture","trusted":true}),
        ));
        assert_eq!(r.error.unwrap().code, "permission_denied");
    }
    let r = response(call(
        &mut a,
        Origin::Automation,
        "schema",
        json!({"type":"fixture:run","cursor":null}),
    ));
    assert_eq!(r.status, rad_wire::response::Status::Ok);
    assert_eq!(
        r.data["items"],
        serde_json::to_value(&manifest.actions[0].schema.fields).unwrap()
    );
    assert!(r.data.get("credentials").is_none());
    let catalog = response(call(
        &mut a,
        Origin::Editor,
        "plugin_catalog",
        json!({"cursor":null}),
    ));
    assert_eq!(catalog.data["items"][0]["type"], "fixture:run");
}

#[test]
fn opaque_plugin_configuration_is_preserved_and_consultation_filtered() {
    use rad_core::configuration::*;
    use rad_core::permissions::{LocalPermissions, Permissions};
    let c = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "Opaque".into(),
            },
        )
        .unwrap();
    let b = c.configuration().data().buttons[0].binding.identity();
    let raw = b" future\0unknown bytes ".to_vec();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::AddAction {
                button: b,
                kind: ActionKind::External {
                    type_key: ExternalType::new("missing".into(), "run".into()).unwrap(),
                    parameters: raw.clone(),
                },
            },
        )
        .unwrap();
    let action = c.configuration().data().buttons[0].actions[0]
        .binding
        .identity();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetActionPermissions {
                action,
                permissions: LocalPermissions::own(Permissions::new(false, false, false).unwrap()),
            },
        )
        .unwrap();
    let mut a = Authority::new(c, 1, NonZeroU128::new(7).unwrap());
    let r = response(call(
        &mut a,
        Origin::Editor,
        "plugin_configuration",
        json!({"id":"a1","cursor":null}),
    ));
    assert_eq!(r.data["type"], "missing:run");
    assert_eq!(
        r.data["items"][0],
        json!(raw.iter().map(|b| format!("{b:02x}")).collect::<String>())
    );
    let r = response(call(
        &mut a,
        Origin::Automation,
        "plugin_configuration",
        json!({"id":"a1","cursor":null}),
    ));
    assert_eq!(r.error.unwrap().code, "not_found");
    let r = response(call(
        &mut a,
        Origin::Automation,
        "options",
        json!({"id":"a1","field":"mode","cursor":null}),
    ));
    assert_eq!(r.error.unwrap().code, "not_found");
}

#[test]
fn plugin_action_and_independent_visual_edits_validate_before_publication() {
    let mut a = actor();
    let m: rad_plugin_contract::Manifest = serde_json::from_value(json!({
        "contract":1,"id":"fixture","version":{"major":1,"minor":0,"patch":0},
        "host_min":1,"host_max":1,"targets":["x86_64-unknown-linux-gnu"],
        "actions":[{"id":"run","label":"Run","schema":{"version":1,"fields":[]},"credential_slots":[]}],
        "visuals":[{"id":"status","label":"Status","schema":{"version":1,"fields":[{"id":"mode","label":"Mode","required":false,"kind":{"type":"choice","options":["compact","full"]}}]},"credential_slots":[]}]
    })).unwrap();
    a.install_plugin(m, 1, "x86_64-unknown-linux-gnu").unwrap();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"Plugin"}),
    );
    apply(
        &mut a,
        "add_action",
        json!({"button_id":"b1","type":"fixture:run","parameters":{"schema_version":1,"values":{}}}),
    );
    apply(
        &mut a,
        "edit",
        json!({"changes":[{"kind":"set","id":"b1","field":"visual","value":{"type":"fixture#status","parameters":{"schema_version":1,"values":{}}}}]}),
    );
    let Resolution::Command(cmd) = call(
        &mut a,
        Origin::Editor,
        "edit",
        json!({"changes":[{"kind":"set","id":"a1","field":"parameters","value":{"schema_version":1,"values":{"unknown":true}}}]}),
    ) else {
        panic!("typed edit");
    };
    let ticket = a.reserve(9, Origin::Editor).unwrap();
    let e = a.submit(ticket, cmd);
    assert!(
        e.iter()
            .any(|e| matches!(e, rad_authority::Effect::Rejected { .. }))
    );
    assert!(
        !e.iter()
            .any(|e| matches!(e, rad_authority::Effect::Persist { .. }))
    );
    let r = response(call(
        &mut a,
        Origin::Editor,
        "plugin_configuration",
        json!({"id":"b1","cursor":null}),
    ));
    assert_eq!(r.data["type"], "fixture#status");
    let r = response(call(
        &mut a,
        Origin::Editor,
        "get_section",
        json!({"id":"b1","section":"appearance","cursor":null}),
    ));
    assert_eq!(r.data["visual_type"], "fixture#status");
    let r = response(call(
        &mut a,
        Origin::Automation,
        "options",
        json!({"id":"b1","field":"mode","cursor":null}),
    ));
    assert!(
        r.error.is_none(),
        "visual options use source schema: {:?}",
        r.error
    );
    assert_eq!(r.data["items"], json!(["compact", "full"]));
    apply(
        &mut a,
        "set_permissions",
        json!({"target":"b1","value":{"own":{"consult":false,"modify":false,"execute":false}}}),
    );
    let r = response(call(
        &mut a,
        Origin::Automation,
        "options",
        json!({"id":"b1","field":"mode","cursor":null}),
    ));
    assert_eq!(r.error.unwrap().code, "not_found");
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
fn credential_ipc_preserves_empty_and_fully_escaped_values_but_rejects_overlong_and_open_shapes() {
    let mut a = actor();
    for secret in [String::new(), "\u{1}".repeat(4096)] {
        assert!(matches!(
            call(
                &mut a,
                Origin::Editor,
                "credential_replace",
                json!({"plugin_id":"fixture","slot":"password","secret":secret})
            ),
            Resolution::External(_)
        ));
    }
    for args in [
        json!({"plugin_id":"fixture","slot":"password","secret":"x".repeat(4097)}),
        json!({"plugin_id":"../other","slot":"password","secret":"x"}),
        json!({"plugin_id":"fixture","slot":"password","secret":"x","role":"editor"}),
    ] {
        assert_eq!(
            response(call(&mut a, Origin::Editor, "credential_replace", args))
                .error
                .unwrap()
                .code,
            "invalid_request"
        );
    }
}

#[test]
fn status_exposes_grid_without_editor_language() {
    let mut a = actor();
    let status = response(call(&mut a, Origin::Editor, "status", json!({})));
    assert_eq!(status.data["grid"], json!({"columns":8,"rows":4}));
    assert!(status.data["settings"].get("gui_language").is_none());
    let rejected = response(call(
        &mut a,
        Origin::Editor,
        "set_settings",
        json!({"changes":{"gui_language":"es"}}),
    ));
    assert_eq!(rejected.error.unwrap().code, "invalid_request");
}

#[test]
fn exhausted_identity_response_preserves_machine_readable_cause() {
    let mut a = actor();
    let ticket = a.reserve(4, Origin::Editor).unwrap();
    let request = ticket.request();
    let effect = rad_authority::Effect::Rejected {
        request,
        reason: rad_authority::Rejection::Configuration(
            rad_core::configuration::ConfigurationError::Identity(
                rad_core::identity::IdentityError::ShortIdExhausted,
            ),
        ),
    };
    let response = Protocol::new(NonZeroU128::new(7).unwrap())
        .response_for(
            &a,
            IncomingContext {
                request_id: 2,
                connection: 4,
                origin: Origin::Editor,
            },
            request,
            &effect,
        )
        .unwrap();
    let error = response.error.unwrap();
    assert_eq!(error.code, "resource_exhausted");
    assert_eq!(error.details["cause"], "identity_counter_exhausted");
}

#[test]
fn plugin_dependencies_are_editor_only_and_paginated() {
    let mut a = actor();
    let page = response(call(
        &mut a,
        Origin::Editor,
        "plugin_dependencies",
        json!({"cursor":null}),
    ));
    assert!(page.error.is_none(), "{page:?}");
    assert_eq!(page.data["items"], json!([]));
    assert!(page.data["next_cursor"].is_null());
    let forbidden = response(call(
        &mut a,
        Origin::Automation,
        "plugin_dependencies",
        json!({"cursor":null}),
    ));
    assert_eq!(forbidden.error.unwrap().code, "permission_denied");
}

#[test]
fn oversized_document_reports_resource_exhausted_with_document_cause() {
    use rad_core::configuration::PluginSettings;
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let mut data = initial.configuration().data().clone();
    for i in 0..140 {
        data.plugin_settings.insert(
            format!("plugin{i}"),
            PluginSettings::new(vec![0xff; 4095]).unwrap(),
        );
    }
    let mut a = actor();
    let ticket = a.reserve(4, Origin::Editor).unwrap();
    let request = ticket.request();
    let effects = a.submit(
        ticket,
        rad_authority::Command::Restore {
            data,
            confirmed: true,
        },
    );
    let effect = effects
        .iter()
        .find(|e| matches!(e, rad_authority::Effect::Rejected { .. }))
        .unwrap();
    let response = Protocol::new(NonZeroU128::new(7).unwrap())
        .response_for(
            &a,
            IncomingContext {
                request_id: 2,
                connection: 4,
                origin: Origin::Editor,
            },
            request,
            effect,
        )
        .unwrap();
    let error = response.error.unwrap();
    assert_eq!(error.code, "resource_exhausted");
    assert_eq!(error.details["cause"], "document_limit");
}

#[test]
fn large_dependency_list_fits_real_frames_without_losing_tail_and_cursor_expires() {
    use rad_core::configuration::{Edit, PluginDependency, PluginSettings, SettingsData};
    let initial = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let mut data = initial.configuration().data().clone();
    for n in 0..40 {
        let id = format!("plugin-{n:02}");
        data.plugin_settings
            .insert(id.clone(), PluginSettings::new(vec![]).unwrap());
        data.plugin_dependencies.insert(
            id,
            PluginDependency {
                version: None,
                origin: Some("x".repeat(8190)),
            },
        );
    }
    let candidate = initial
        .configuration()
        .prepare_restore(initial.registry(), Origin::Editor, true, data)
        .unwrap();
    let mut a = Authority::new(candidate, 1, NonZeroU128::new(7).unwrap());
    let protocol = Protocol::new(NonZeroU128::new(7).unwrap());
    let mut cursor = Value::Null;
    let mut seen = std::collections::BTreeSet::new();
    let mut old_cursor = Value::Null;
    loop {
        let page = response(call(
            &mut a,
            Origin::Editor,
            "plugin_dependencies",
            json!({"cursor":cursor}),
        ));
        assert!(page.error.is_none(), "{page:?}");
        let encoded = protocol.encode_response(&page).unwrap();
        assert!(encoded.len() <= 65536);
        for item in page.data["items"].as_array().unwrap() {
            assert!(seen.insert(item["id"].as_str().unwrap().to_owned()));
            assert!(item["needs_credentials"].is_null());
        }
        cursor = page.data["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
        old_cursor = cursor.clone();
    }
    assert_eq!(seen.len(), 40);
    let ticket = a.reserve(4, Origin::Editor).unwrap();
    let effects = a.submit(
        ticket,
        vec![Edit::SetSettings(SettingsData {
            brightness_percent: 12,
            ..Default::default()
        })],
    );
    let id = effects
        .iter()
        .find_map(|e| match e {
            rad_authority::Effect::Persist { id, .. } => Some(*id),
            _ => None,
        })
        .unwrap();
    a.complete(
        id,
        rad_authority::Outcome::Applied {
            revision: 2,
            changed: true,
        },
    );
    let page = response(call(
        &mut a,
        Origin::Editor,
        "plugin_dependencies",
        json!({"cursor":old_cursor}),
    ));
    assert_eq!(page.error.unwrap().code, "cursor_expired");
}
