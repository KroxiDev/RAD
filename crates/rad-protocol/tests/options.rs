// Dynamic options at the IPC boundary: what a request resolves to, and what it may not.
use rad_api::response::{Response, Status};
use rad_authority::Authority;
use rad_core::{
    configuration::{Configuration, Origin},
    identity::IdentityRegistry,
};
use rad_plugin_contract::{ChoiceList, ChoiceOption, Manifest, OptionsScope};
use rad_protocol::{
    External, IncomingContext, OptionsSubject, PluginOptions, Protocol, Resolution,
};
use serde_json::{Value, json};
use std::num::NonZeroU128;

const TARGET: &str = "x86_64-unknown-linux-gnu";
const PLUGIN: &str = "org.rad.test.options";

fn protocol() -> Protocol {
    Protocol::new(NonZeroU128::new(7).unwrap())
}

fn call(a: &mut Authority, origin: Origin, op: &str, args: Value) -> Resolution {
    fn frame(value: Value) -> Vec<u8> {
        let v = serde_json::to_vec(&value).unwrap();
        let mut b = (v.len() as u32).to_be_bytes().to_vec();
        b.extend(v);
        b
    }
    let mut wire = rad_wire::Session::default();
    let hello = frame(json!({"v":1,"id":"1","op":"hello","args":{}}));
    wire.accept(rad_wire::read_frame(&hello).unwrap().unwrap())
        .unwrap();
    let request = frame(json!({"v":1,"id":"2","op":op,"args":args}));
    let request = wire
        .accept(rad_wire::read_frame(&request).unwrap().unwrap())
        .unwrap();
    protocol().handle(
        a,
        IncomingContext {
            request_id: 2,
            connection: 4,
            origin,
        },
        &request,
    )
}

fn apply(a: &mut Authority, op: &str, args: Value) {
    let Resolution::Command(cmd) = call(a, Origin::Editor, op, args) else {
        panic!("expected {op} command")
    };
    let t = a.reserve(4, Origin::Editor).unwrap();
    let request = t.request();
    let effects = a.submit(t, cmd);
    let id = effects
        .iter()
        .find_map(|e| match e {
            rad_authority::Effect::Persist { id, .. } => Some(*id),
            _ => None,
        })
        .expect("persist");
    let revision = a.editor_view(Origin::Editor).unwrap().revision + 1;
    a.complete(
        id,
        rad_authority::Outcome::Applied {
            revision,
            changed: true,
        },
    );
    a.finish(request);
}

/// A runtime-less Authority with the options test plugin published at `generation` 7 and
/// one button `b1` holding `pick` (a1) and the `probe` visual.
fn configured(saved: Value) -> Authority {
    let mut a = Authority::new(
        Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap(),
        1,
        NonZeroU128::new(7).unwrap(),
    );
    let manifest: Manifest = serde_json::from_slice(include_bytes!(
        "../../rad-plugin-host/test-plugins/options-manifest.json"
    ))
    .unwrap();
    a.install_plugin(manifest, 7, TARGET).unwrap();
    apply(
        &mut a,
        "create_button",
        json!({"folder_id":"f1","position":0,"name":"Options"}),
    );
    apply(
        &mut a,
        "add_action",
        json!({"button_id":"b1","type":format!("{PLUGIN}:pick"),
               "parameters":{"schema_version":1,"values":saved}}),
    );
    apply(
        &mut a,
        "edit",
        json!({"changes":[{"kind":"set","id":"b1","field":"visual","value":{
            "type":format!("{PLUGIN}#probe"),
            "parameters":{"schema_version":1,"values":{"scenario":"normal","group":"a"}}}}]}),
    );
    a
}

fn saved() -> Value {
    json!({"scenario":"normal","group":"a","member":"a2"})
}

fn query(r: Resolution) -> PluginOptions {
    match r {
        Resolution::External(External::PluginOptions(q)) => *q,
        Resolution::Immediate { response, .. } => panic!("not deferred: {:?}", response.error),
        _ => panic!("not an options query"),
    }
}

fn code(r: Resolution) -> String {
    match r {
        Resolution::Immediate { response, .. } => {
            response.error.expect("rejection").code.to_string()
        }
        _ => panic!("expected a rejection"),
    }
}

fn values(q: &PluginOptions) -> Value {
    Value::Object(q.context.values.clone())
}

#[test]
fn an_entity_lists_a_dynamic_field_from_its_saved_values_and_the_draft_overrides_them() {
    let mut a = configured(saved());
    let q = query(call(
        &mut a,
        Origin::Editor,
        "options",
        json!({"id":"a1","field":"member","cursor":null}),
    ));
    assert_eq!(
        (q.plugin.as_str(), q.generation, q.scope),
        (
            PLUGIN,
            a.plugin_manifests().next().unwrap().1,
            OptionsScope::Action
        )
    );
    assert_eq!(
        (q.definition.as_str(), q.field.as_str()),
        ("pick", "member")
    );
    assert_eq!(q.context.schema_version, 1);
    // `member` depends on scenario and group; its own saved value is not context.
    assert_eq!(values(&q), json!({"scenario":"normal","group":"a"}));
    assert_eq!(
        q.subject,
        OptionsSubject::Entity {
            saved: Some("a2".into())
        }
    );

    // An unsaved draft wins over the stored value; the stored one is still reported.
    let q = query(call(
        &mut a,
        Origin::Automation,
        "options",
        json!({"id":"a1","field":"member","cursor":null,"context":{"group":"b"}}),
    ));
    assert_eq!(values(&q), json!({"scenario":"normal","group":"b"}));
    assert_eq!(
        q.subject,
        OptionsSubject::Entity {
            saved: Some("a2".into())
        }
    );
}

#[test]
fn a_visual_source_resolves_in_the_visual_scope() {
    let mut a = configured(saved());
    let q = query(call(
        &mut a,
        Origin::Editor,
        "options",
        json!({"id":"b1","field":"group","cursor":null}),
    ));
    assert_eq!(q.scope, OptionsScope::Visual);
    assert_eq!(
        (q.definition.as_str(), q.field.as_str()),
        ("probe", "group")
    );
    assert_eq!(values(&q), json!({"scenario":"normal"}));
    assert_eq!(
        q.subject,
        OptionsSubject::Entity {
            saved: Some("a".into())
        }
    );
}

#[test]
fn a_draft_without_an_entity_is_asked_by_type_with_the_same_access_as_schema() {
    let mut a = configured(saved());
    for origin in [Origin::Editor, Origin::Automation] {
        let q = query(call(
            &mut a,
            origin,
            "options",
            json!({"type":format!("{PLUGIN}:pick"),"field":"group","cursor":null,
                   "context":{"scenario":"empty"}}),
        ));
        assert_eq!(q.subject, OptionsSubject::Type);
        assert_eq!(
            (q.plugin.as_str(), q.generation, q.scope),
            (
                PLUGIN,
                a.plugin_manifests().next().unwrap().1,
                OptionsScope::Action
            )
        );
        assert_eq!(values(&q), json!({"scenario":"empty"}));
    }
    let q = query(call(
        &mut a,
        Origin::Automation,
        "options",
        json!({"type":format!("{PLUGIN}#probe"),"field":"group","cursor":null}),
    ));
    assert_eq!(
        (q.scope, q.definition.as_str()),
        (OptionsScope::Visual, "probe")
    );
    assert_eq!(values(&q), json!({}));
}

#[test]
fn values_the_current_schema_cannot_read_are_left_out_instead_of_failing_the_query() {
    use rad_core::configuration::*;
    // Stored bytes written for another version of the plugin: configuration is opaque to the
    // domain, so it can hold a scenario the manifest no longer lists.
    let c = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "Old".into(),
            },
        )
        .unwrap();
    let button = c.configuration().data().buttons[0].binding.identity();
    let mut c = c;
    for bytes in [
        &br#"{"schema_version":1,"values":{"scenario":"retired","group":"a"}}"#[..],
        &br#"{"schema_version":9,"values":{"scenario":"normal","group":"a"}}"#[..],
        &b" not json "[..],
    ] {
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::AddAction {
                    button,
                    kind: ActionKind::External {
                        type_key: ExternalType::new(PLUGIN.into(), "pick".into()).unwrap(),
                        parameters: bytes.to_vec(),
                    },
                },
            )
            .unwrap();
    }
    let mut a = Authority::new(c, 1, NonZeroU128::new(7).unwrap());
    let manifest: Manifest = serde_json::from_slice(include_bytes!(
        "../../rad-plugin-host/test-plugins/options-manifest.json"
    ))
    .unwrap();
    a.install_plugin(manifest, 7, TARGET).unwrap();
    for (action, expected, saved) in [
        ("a1", json!({"group":"a"}), None),
        ("a2", json!({}), None),
        ("a3", json!({}), None),
    ] {
        let q = query(call(
            &mut a,
            Origin::Editor,
            "options",
            json!({"id":action,"field":"member","cursor":null}),
        ));
        assert_eq!(values(&q), expected, "{action}");
        assert_eq!(q.subject, OptionsSubject::Entity { saved }, "{action}");
    }
}

#[test]
fn malformed_dynamic_requests_are_invalid_and_do_not_reach_a_plugin() {
    let mut a = configured(saved());
    let mut by = |args: Value| code(call(&mut a, Origin::Editor, "options", args));
    for (args, expected) in [
        // Unknown field, a fixed field with a context, and a field that is not listable.
        (
            json!({"id":"a1","field":"nothing","cursor":null}),
            "invalid_request",
        ),
        (
            json!({"id":"a1","field":"scenario","cursor":null,"context":{"scenario":"normal"}}),
            "invalid_request",
        ),
        // Context outside the field's dependencies or the schema's values.
        (
            json!({"id":"a1","field":"group","cursor":null,"context":{"member":"a1"}}),
            "invalid_request",
        ),
        (
            json!({"id":"a1","field":"group","cursor":null,"context":{"scenario":"bogus"}}),
            "invalid_request",
        ),
        (
            json!({"id":"a1","field":"group","cursor":null,"context":{"scenario":7}}),
            "invalid_request",
        ),
        (
            json!({"id":"a1","field":"group","cursor":null,"context":["normal"]}),
            "invalid_request",
        ),
        (
            json!({"id":"a1","field":"group","cursor":null,"context":null}),
            "invalid_request",
        ),
        // The list always fits one response: there is no second page to ask for.
        (
            json!({"id":"a1","field":"group","cursor":"x"}),
            "cursor_expired",
        ),
        (json!({"id":"a1","field":"group"}), "invalid_request"),
        (
            json!({"id":"a1","field":"group","cursor":null,"extra":1}),
            "invalid_request",
        ),
        // One subject only, and a type must name a dynamic field of a published definition.
        (
            json!({"id":"a1","type":format!("{PLUGIN}:pick"),"field":"group","cursor":null}),
            "invalid_request",
        ),
        (
            json!({"type":format!("{PLUGIN}:pick"),"field":"scenario","cursor":null}),
            "invalid_request",
        ),
        (
            json!({"type":format!("{PLUGIN}:pick"),"field":"group","cursor":null,
                   "context":{"group":"a"}}),
            "invalid_request",
        ),
        (
            json!({"type":format!("{PLUGIN}:nothing"),"field":"group","cursor":null}),
            "executor_unavailable",
        ),
        (
            json!({"type":"org.rad.absent:pick","field":"group","cursor":null}),
            "executor_unavailable",
        ),
        (
            json!({"type":"nothing","field":"group","cursor":null}),
            "executor_unavailable",
        ),
        (
            json!({"id":"a99","field":"group","cursor":null}),
            "not_found",
        ),
    ] {
        assert_eq!(by(args.clone()), expected, "{args}");
    }
}

#[test]
fn a_plugin_that_is_not_ready_cannot_be_asked() {
    let mut a = configured(saved());
    a.plugin_failed(PLUGIN, 7);
    for args in [
        json!({"id":"a1","field":"group","cursor":null}),
        json!({"type":format!("{PLUGIN}:pick"),"field":"group","cursor":null}),
    ] {
        assert_eq!(
            code(call(&mut a, Origin::Editor, "options", args)),
            "executor_unavailable"
        );
    }
}

#[test]
fn fixed_choices_and_the_consultation_filter_keep_their_behavior() {
    let mut a = configured(saved());
    let Resolution::Immediate { response, .. } = call(
        &mut a,
        Origin::Automation,
        "options",
        json!({"id":"a1","field":"scenario","cursor":null}),
    ) else {
        panic!("fixed choices answer immediately")
    };
    assert_eq!(response.status, Status::Ok);
    assert_eq!(
        serde_json::to_value(&response.data).unwrap()["items"][0],
        "normal"
    );
    apply(
        &mut a,
        "set_permissions",
        json!({"target":"a1","value":{"own":{"consult":false,"modify":false,"execute":false}}}),
    );
    assert_eq!(
        code(call(
            &mut a,
            Origin::Automation,
            "options",
            json!({"id":"a1","field":"group","cursor":null})
        )),
        "not_found"
    );
    // The editor still configures what automation may not see.
    assert!(matches!(
        call(
            &mut a,
            Origin::Editor,
            "options",
            json!({"id":"a1","field":"group","cursor":null})
        ),
        Resolution::External(External::PluginOptions(_))
    ));
}

#[test]
fn live_options_by_type_need_global_consultation_for_automation_but_never_for_the_editor() {
    let mut a = configured(saved());
    let by_type = json!({"type":format!("{PLUGIN}:pick"),"field":"group","cursor":null});
    let asked = |a: &mut Authority, origin| call(a, origin, "options", by_type.clone());
    assert!(matches!(
        asked(&mut a, Origin::Automation),
        Resolution::External(External::PluginOptions(_))
    ));
    apply(
        &mut a,
        "set_permissions",
        json!({"target":"global","value":{"own":{"consult":false,"modify":false,"execute":false}}}),
    );
    // Live system state is not manifest metadata: with global Consulta off, automation cannot
    // enumerate it by type, and it is refused before anything about the type is revealed.
    assert_eq!(code(asked(&mut a, Origin::Automation)), "permission_denied");
    let unknown = json!({"type":"org.rad.absent:pick","field":"group","cursor":null});
    assert_eq!(
        code(call(&mut a, Origin::Automation, "options", unknown)),
        "permission_denied"
    );
    // The editor configures regardless, and `schema` metadata stays open to automation.
    assert!(matches!(
        asked(&mut a, Origin::Editor),
        Resolution::External(External::PluginOptions(_))
    ));
    let Resolution::Immediate { response, .. } = call(
        &mut a,
        Origin::Automation,
        "schema",
        json!({"type":format!("{PLUGIN}:pick"),"cursor":null}),
    ) else {
        panic!("schema answers immediately")
    };
    assert_eq!(response.status, Status::Ok);
    // What the user allowed the agent to consult entity by entity still works.
    for target in ["b1", "a1"] {
        apply(
            &mut a,
            "set_permissions",
            json!({"target":target,"value":{"own":{"consult":true,"modify":false,"execute":false}}}),
        );
    }
    assert!(matches!(
        call(
            &mut a,
            Origin::Automation,
            "options",
            json!({"id":"a1","field":"group","cursor":null})
        ),
        Resolution::External(External::PluginOptions(_))
    ));
    assert_eq!(code(asked(&mut a, Origin::Automation)), "permission_denied");
}

#[test]
fn a_query_never_prints_the_values_it_carries() {
    let mut a = configured(json!({"scenario":"normal","group":"a","member":"SAVED-SECRET"}));
    let Resolution::External(external) = call(
        &mut a,
        Origin::Editor,
        "options",
        json!({"id":"a1","field":"member","cursor":null,"context":{"group":"DRAFT-SECRET"}}),
    ) else {
        panic!("not deferred")
    };
    // `DRAFT-SECRET` is no valid group id for the plugin, but any text is a valid label.
    let printed = format!("{external:?}");
    for hidden in ["SAVED-SECRET", "DRAFT-SECRET", "normal"] {
        assert!(!printed.contains(hidden), "{hidden} in {printed}");
    }
    let subject = format!(
        "{:?}",
        OptionsSubject::Entity {
            saved: Some("SAVED-SECRET".into())
        }
    );
    assert!(!subject.contains("SAVED-SECRET"), "{subject}");
}

fn entity_query(saved: Option<&str>) -> PluginOptions {
    let mut a = configured(json!({"scenario":"normal","group":"a","member":"a2"}));
    let mut q = query(call(
        &mut a,
        Origin::Editor,
        "options",
        json!({"id":"a1","field":"member","cursor":null}),
    ));
    q.subject = OptionsSubject::Entity {
        saved: saved.map(Into::into),
    };
    q
}

fn context() -> IncomingContext {
    IncomingContext {
        request_id: 2,
        connection: 4,
        origin: Origin::Editor,
    }
}

fn listed(values: &[(&str, &str)]) -> ChoiceList {
    ChoiceList::Available {
        options: values
            .iter()
            .map(|(value, label)| ChoiceOption {
                value: (*value).into(),
                label: (*label).into(),
            })
            .collect(),
    }
}

fn decoded(response: &Response) -> Response {
    Response::decode_value(
        serde_json::to_value(response).unwrap(),
        2,
        None,
        rad_api::Operation::Options,
    )
    .unwrap()
}

#[test]
fn an_answer_reports_the_list_and_whether_the_saved_value_is_still_on_it() {
    let list = listed(&[("a1", "Member A1"), ("a2", "Member A2")]);
    let response = protocol().options_response(context(), &entity_query(Some("a2")), Some(&list));
    assert_eq!(decoded(&response).status, Status::Ok);
    assert_eq!(
        serde_json::to_value(&response.data).unwrap(),
        json!({"type":format!("{PLUGIN}:pick"),"field":"member","availability":"available",
               "items":[{"value":"a1","label":"Member A1"},{"value":"a2","label":"Member A2"}],
               "next_cursor":null,"saved":"a2","saved_listed":true})
    );
    // The saved value left the list: reported, flagged, never substituted.
    let list = listed(&[("b1", "Member B1")]);
    let response = protocol().options_response(context(), &entity_query(Some("a2")), Some(&list));
    assert_eq!(serde_json::to_value(&response.data).unwrap()["saved"], "a2");
    assert_eq!(
        serde_json::to_value(&response.data).unwrap()["saved_listed"],
        false
    );
    assert_eq!(
        serde_json::to_value(&response.data).unwrap()["items"],
        json!([{"value":"b1","label":"Member B1"}])
    );
    // Nothing saved yet.
    let response = protocol().options_response(context(), &entity_query(None), Some(&list));
    assert_eq!(
        serde_json::to_value(&response.data).unwrap()["saved"],
        Value::Null
    );
    assert_eq!(
        serde_json::to_value(&response.data).unwrap()["saved_listed"],
        false
    );
}

#[test]
fn empty_unavailable_and_late_are_three_different_answers() {
    let empty =
        protocol().options_response(context(), &entity_query(Some("a2")), Some(&listed(&[])));
    assert_eq!(
        serde_json::to_value(&empty.data).unwrap()["availability"],
        "available"
    );
    assert_eq!(
        serde_json::to_value(&empty.data).unwrap()["items"],
        json!([])
    );
    assert_eq!(
        serde_json::to_value(&empty.data).unwrap()["saved_listed"],
        false
    );

    let unavailable = protocol().options_response(
        context(),
        &entity_query(Some("a2")),
        Some(&ChoiceList::Unavailable),
    );
    assert_eq!(decoded(&unavailable).status, Status::Ok);
    assert_eq!(
        serde_json::to_value(&unavailable.data).unwrap()["availability"],
        "unavailable"
    );
    assert_eq!(
        serde_json::to_value(&unavailable.data).unwrap()["items"],
        json!([])
    );
    assert_eq!(
        serde_json::to_value(&unavailable.data).unwrap()["saved"],
        "a2"
    );

    let late = protocol().options_response(context(), &entity_query(Some("a2")), None);
    assert_eq!(decoded(&late).status, Status::Error);
    let error = late.error.unwrap();
    assert_eq!(
        (error.code.as_str(), error.outcome),
        ("options_timeout", None)
    );
}

#[test]
fn a_type_answer_has_no_saved_value_and_an_oversized_list_is_refused_not_truncated() {
    let mut a = configured(saved());
    let by_type = query(call(
        &mut a,
        Origin::Editor,
        "options",
        json!({"type":format!("{PLUGIN}:pick"),"field":"group","cursor":null}),
    ));
    let list = listed(&[("a", "Group A")]);
    let response = protocol().options_response(context(), &by_type, Some(&list));
    assert_eq!(
        serde_json::to_value(&response.data).unwrap(),
        json!({"type":format!("{PLUGIN}:pick"),"field":"group","availability":"available",
               "items":[{"value":"a","label":"Group A"}],"next_cursor":null})
    );
    // 64 options whose escaped text exceeds a response frame (the plugin frame fits).
    let heavy = "\"".repeat(256);
    let options: Vec<(String, String)> = (0..64)
        .map(|n| (format!("{n}{heavy}"), heavy.clone()))
        .collect();
    let refs: Vec<(&str, &str)> = options
        .iter()
        .map(|(v, l)| (&v[..255], l.as_str()))
        .collect();
    let response = protocol().options_response(context(), &by_type, Some(&listed(&refs)));
    assert_eq!(response.error.unwrap().code, "resource_exhausted");
}

#[test]
fn dormant_selected_metadata_still_resolves_dynamic_options() {
    let mut a = configured(saved());
    let selection = a.plugin_manifests().next().unwrap().1;
    a.begin_plugin_stop(PLUGIN, 7).unwrap();
    let q = query(call(
        &mut a,
        Origin::Editor,
        "options",
        json!({"type":format!("{PLUGIN}:pick"),"field":"group","cursor":null,"context":{}}),
    ));
    assert_eq!(q.generation, selection);
    assert_eq!(a.plugin_generation(PLUGIN), None);
}
