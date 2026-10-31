use rad_client::{connection::Failure, response::Response};
use rad_editor::session::{Port, Session};
use serde_json::{Value, json};

struct Script(std::collections::VecDeque<(&'static str, Value)>);
impl Port for Script {
    fn call(&mut self, request: rad_api::Request) -> Result<Response, Failure> {
        let op = request.operation().as_str();
        let (expected, data) = self.0.pop_front().expect("unexpected request");
        assert_eq!(op, expected);
        Ok(Response {
            v: 1,
            id: Some("1".into()),
            session_id: None,
            status: rad_client::response::Status::Ok,
            data: rad_api::ResponseData::decode(
                rad_api::Operation::parse(op).unwrap(),
                rad_api::Status::Ok,
                data,
            )
            .map_err(|_| Failure {
                code: "invalid_request",
                id: None,
                session_id: None,
                outcome: None,
            })?,
            error: None,
        })
    }
}
fn overview() -> Value {
    json!({"view_revision":"2","active_folder_id":"f1","storage":"ready","deck":"connected","plugins":[],"last_restart":"clean","settings":{"brightness_percent":100,"sleep_ms":"0","press_threshold_ms":"0"},"global_permissions":{"consult":true,"modify":true,"execute":false},"history_operations":0})
}
fn entity(id: &str, kind: &str, fields: Value) -> Value {
    json!({"visibility":"visible","id":id,"kind":kind,"fields":fields,"view_revision":"2","sections":[]})
}
fn button() -> Value {
    entity(
        "b8",
        "button",
        json!({"gesture":"press","name":"Browser","mode":"sequential","permissions":{"inherit":true},"effective_permissions":{"consult":true,"modify":true,"execute":false},"folder_id":"f1","position":3}),
    )
}
fn action() -> Value {
    entity(
        "a4",
        "action",
        json!({"type":"launcher","permissions":{"inherit":true},"effective_permissions":{"consult":true,"modify":true,"execute":false},"parent_id":"b8","parameters":{"desktop_entry":"browser.desktop"},"folder_id":null}),
    )
}
fn appearance() -> Value {
    json!({"text":"Go","image":"owned-media","text_position":"bottom","visual_type":null})
}
fn board() -> Value {
    let mut slots = vec![Value::Null; 32];
    slots[3] = json!({"visibility":"visible","id":"b8"});
    let folder = entity(
        "f1",
        "folder",
        json!({"name":"Main","entry_button_id":null,"parent_id":null,"slots":slots}),
    );
    let mut keys = vec![Value::Null; 32];
    keys[3] = json!({"entity":button(),"appearance":appearance(),"visual":null});
    json!({"view_revision":"2","grid":{"columns":8,"rows":4},"folder":folder,"keys":keys})
}
fn inspector(actions: Vec<Value>) -> Value {
    json!({"view_revision":"2","entity":button(),"appearance":appearance(),"actions":actions,"plugin":null})
}

#[test]
fn inspector_assembles_runtime_sections_without_mixing_revisions() {
    let mut s = Session::new(Script([("inspector", inspector(vec![action()]))].into()));
    let view = s.inspect("b8").unwrap();
    assert_eq!(view.appearance, appearance());
    assert_eq!(view.actions, vec![action()]);
    let mut changed = inspector(vec![action()]);
    changed["entity"]["view_revision"] = json!("3");
    let mut s = Session::new(Script([("inspector", changed)].into()));
    assert_eq!(s.inspect("b8").unwrap_err().code, "cursor_expired");
}

#[test]
fn independent_composed_views_must_share_a_revision_before_display() {
    let mut session = Session::new(Script(
        [
            ("overview", overview()),
            ("board", board()),
            ("inspector", inspector(vec![action()])),
        ]
        .into(),
    ));
    let board = session.refresh(true).unwrap().board.unwrap();
    let mut inspection = session.inspect("b8").unwrap();
    assert!(rad_editor::session::coherent(&board, &inspection));
    inspection.button["view_revision"] = json!("3");
    assert!(!rad_editor::session::coherent(&board, &inspection));
}
