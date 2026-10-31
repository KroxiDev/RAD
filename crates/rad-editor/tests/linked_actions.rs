use rad_client::{
    connection::Failure,
    response::{Response, Status},
};
use rad_editor::{
    Request,
    linked_actions::{LinkedAction, Mutation},
    session::{Port, Session},
};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
fn parameters(device: &str) -> Value {
    json!({"schema_version":1,"values":{"device":device,"operation":"toggle"}})
}
fn plugin(device: &str, visual: bool) -> Value {
    json!({"type":if visual {"org.rad.audio#microphone"} else {"org.rad.audio:microphone"},"parameters":{"schema_version":1,"values":if visual {json!({"device":device})} else {json!({"device":device,"operation":"toggle"})}}})
}
fn action(id: &str) -> Value {
    json!({"visibility":"visible","id":id,"kind":"action","fields":{"type":"org.rad.audio:microphone","permissions":{"inherit":true},"effective_permissions":{"consult":true,"modify":true,"execute":true},"parent_id":"b1","parameters":null,"folder_id":null},"view_revision":"2","sections":[]})
}
fn views(current: Option<&str>, actions: &[(&str, &str)]) -> BTreeMap<String, Value> {
    let mut result = BTreeMap::new();
    result.insert("b1".into(), json!({"view_revision":"2","entity":{"visibility":"visible","id":"b1","kind":"button","fields":{"name":"Mic","mode":"sequential","permissions":{"inherit":true},"effective_permissions":{"consult":true,"modify":true,"execute":true},"folder_id":"f1","position":0},"view_revision":"2","sections":[]},"appearance":{"text":"","image":null,"text_position":"bottom","visual_type":current.map(|_|"org.rad.audio#microphone")},"actions":actions.iter().map(|(id,_)|action(id)).collect::<Vec<_>>(),"plugin":current.map(|d|plugin(d,true))}));
    for (id, device) in actions {
        result.insert((*id).into(),json!({"view_revision":"2","entity":action(id),"appearance":null,"actions":[],"plugin":plugin(device,false)}));
    }
    result
}
struct Fake {
    views: BTreeMap<String, Value>,
    calls: Rc<RefCell<Vec<Request>>>,
    fail: bool,
}
impl Port for Fake {
    fn call(&mut self, request: Request) -> Result<Response, Failure> {
        let operation = request.operation();
        self.calls.borrow_mut().push(request.clone());
        let (status, data) = match request {
            Request::PluginCatalog { .. } => (
                Status::Ok,
                json!({"items":[{"type":"org.rad.audio:microphone","kind":"action","label":"Microphone","version":{"major":0,"minor":4,"patch":0},"ready":true}],"next_cursor":null}),
            ),
            Request::PluginManifest { .. } => (
                Status::Ok,
                json!({"plugin_id":"org.rad.audio","document":serde_json::from_slice::<Value>(include_bytes!("../../../plugins/audio/manifest.json")).unwrap()}),
            ),
            Request::Inspector { id } => (Status::Ok, self.views[&id].clone()),
            Request::Edit { .. } if self.fail => {
                return Err(Failure {
                    code: "runtime_unavailable",
                    id: Some(9),
                    session_id: Some("s".into()),
                    outcome: Some(rad_api::Outcome::Unknown),
                });
            }
            Request::Edit { .. } => (Status::Applied, json!({"revision":"3","changed":true})),
            _ => panic!("unexpected request"),
        };
        Ok(Response {
            v: 1,
            id: Some("1".into()),
            session_id: None,
            status,
            data: rad_api::ResponseData::decode(operation, status, data).unwrap(),
            error: None,
        })
    }
}
fn run(current: Option<&str>, actions: &[(&str, &str)], mutation: Mutation) -> Vec<Request> {
    let calls = Rc::new(RefCell::new(vec![]));
    let mut session = Session::new(Fake {
        views: views(current, actions),
        calls: calls.clone(),
        fail: false,
    });
    let texts = session.plugin_catalog().unwrap()[0].texts.clone();
    LinkedAction {
        button: "b1".into(),
        type_key: "org.rad.audio:microphone".into(),
        texts,
        mutation,
    }
    .publish(&mut session)
    .unwrap();
    calls.borrow().clone()
}
fn changes(calls: &[Request]) -> Value {
    let edit = calls.last().unwrap();
    assert_eq!(edit.arguments()["expected_revision"], "2");
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.operation().as_str() == "edit")
            .count(),
        1
    );
    edit.arguments()["changes"].clone()
}
#[test]
fn adding_or_editing_another_target_does_not_steal_existing_visual() {
    for mutation in [
        Mutation::Add {
            parameters: parameters("B"),
        },
        Mutation::Edit {
            id: "a2".into(),
            parameters: parameters("C"),
        },
    ] {
        let calls = run(Some("A"), &[("a1", "A"), ("a2", "B")], mutation);
        assert_eq!(changes(&calls).as_array().unwrap().len(), 1);
    }
}
#[test]
fn removing_another_target_or_one_of_shared_bindings_preserves_visual() {
    for actions in [
        vec![("a1", "A"), ("a2", "B")],
        vec![("a1", "A"), ("a2", "A")],
    ] {
        let calls = run(Some("A"), &actions, Mutation::Remove { id: "a2".into() });
        assert_eq!(changes(&calls).as_array().unwrap().len(), 1);
    }
}
#[test]
fn owning_action_can_update_or_remove_and_missing_visual_is_linked_atomically() {
    for (current, mutation, expected) in [
        (
            Some("A"),
            Mutation::Edit {
                id: "a1".into(),
                parameters: parameters("B"),
            },
            json!("org.rad.audio#microphone"),
        ),
        (
            Some("A"),
            Mutation::Remove { id: "a1".into() },
            json!("manual"),
        ),
        (
            None,
            Mutation::Add {
                parameters: parameters("B"),
            },
            json!("org.rad.audio#microphone"),
        ),
    ] {
        let calls = run(current, &[("a1", "A")], mutation);
        let changes = changes(&calls);
        assert_eq!(changes.as_array().unwrap().len(), 2);
        assert_eq!(changes[1]["value"]["type"], expected);
    }
}

#[test]
fn shared_original_binding_is_not_retargeted_by_editing_one_action() {
    let calls = run(
        Some("A"),
        &[("a1", "A"), ("a2", "A")],
        Mutation::Edit {
            id: "a2".into(),
            parameters: parameters("B"),
        },
    );
    assert_eq!(changes(&calls).as_array().unwrap().len(), 1);
}
#[test]
fn mixed_read_revisions_abort_before_edit_and_unknown_is_never_replayed() {
    for stale in [true, false] {
        let calls = Rc::new(RefCell::new(vec![]));
        let mut snapshots = views(Some("A"), &[("a1", "A")]);
        if stale {
            snapshots.get_mut("a1").unwrap()["view_revision"] = json!("3");
            snapshots.get_mut("a1").unwrap()["entity"]["view_revision"] = json!("3");
        }
        let mut session = Session::new(Fake {
            views: snapshots,
            calls: calls.clone(),
            fail: !stale,
        });
        let texts = session.plugin_catalog().unwrap()[0].texts.clone();
        let error = LinkedAction {
            button: "b1".into(),
            type_key: "org.rad.audio:microphone".into(),
            texts,
            mutation: Mutation::Edit {
                id: "a1".into(),
                parameters: parameters("B"),
            },
        }
        .publish(&mut session)
        .unwrap_err();
        assert_eq!(error.unknown, !stale);
        if stale {
            assert_eq!(error.code, "cursor_expired");
        }
        assert_eq!(
            calls
                .borrow()
                .iter()
                .filter(|r| r.operation().as_str() == "edit")
                .count(),
            usize::from(!stale)
        );
    }
}
