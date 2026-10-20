use rad_client::{
    connection::Failure,
    response::{Outcome, Response},
};
use rad_editor::session::{Port, Session};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
struct Failing(Arc<AtomicUsize>);
impl Port for Failing {
    fn call(&mut self, _: rad_api::Request) -> Result<Response, Failure> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(Failure {
            code: "runtime_unavailable",
            id: Some(2),
            session_id: None,
            outcome: Some(Outcome::Unknown),
        })
    }
}
#[test]
fn unknown_mutation_is_not_replayed_or_reconnected() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut s = Session::new(Failing(calls.clone()));
    let error = s.request(request("undo", json!({}))).unwrap_err();
    assert!(error.unknown);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(s.request(request("status", json!({}))).is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
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
fn unchanged_status_does_not_reload_deck_and_changed_board_is_complete() {
    let mut refreshed = overview();
    refreshed["deck"] = json!("disconnected");
    refreshed["last_restart"] = json!("recovered_previous");
    let mut s = Session::new(Script(
        [
            ("overview", overview()),
            ("board", board()),
            ("overview", refreshed),
        ]
        .into(),
    ));
    let first = s.refresh(false).unwrap().board.unwrap();
    assert_eq!(first.folder, board()["folder"]);
    assert_eq!(first.buttons[0].entity, button());
    let next = s.refresh(false).unwrap();
    assert!(next.board.is_none());
    assert_eq!(
        rad_editor::status::deck_label(&next.status, false),
        "Deck disconnected — editor available"
    );
    assert!(rad_editor::status::recovery_notice(&next.status, false).is_some());
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
fn board_loads_text_and_owned_image_reference_for_rendering() {
    let mut s = Session::new(Script(
        [("overview", overview()), ("board", board())].into(),
    ));
    assert_eq!(
        s.refresh(false).unwrap().board.unwrap().buttons[0].appearance,
        appearance()
    );
}
#[test]
fn media_preview_uses_entity_authorization_and_one_revision() {
    let entity = button();
    let mut s = Session::new(Script(
        [
            ("get", entity.clone()),
            ("media_read", json!({"base64":"AAEC/w==","eof":true})),
            ("get", entity),
        ]
        .into(),
    ));
    assert_eq!(s.media_bytes("b8", "2").unwrap(), vec![0, 1, 2, 255]);
}
#[test]
fn inspector_accepts_twenty_actions_and_rejects_twenty_one() {
    for count in [20, 21] {
        let mut s = Session::new(Script(
            [("inspector", inspector(vec![action(); count]))].into(),
        ));
        assert_eq!(s.inspect("b8").is_ok(), count == 20);
    }
}
#[test]
fn refresh_rejects_missing_or_unbounded_grid_before_showing_board() {
    for grid in [
        Value::Null,
        json!({"columns":0,"rows":3}),
        json!({"columns":1000,"rows":3}),
        json!([8, 4]),
    ] {
        let mut b = board();
        b["grid"] = grid;
        let mut s = Session::new(Script([("overview", overview()), ("board", b)].into()));
        assert!(s.refresh(false).is_err());
    }
}
#[test]
fn grid_uses_the_runtime_descriptor_instead_of_assuming_xl() {
    let grid =
        rad_editor::session::Grid::from_status(&json!({"grid":{"columns":5,"rows":3}})).unwrap();
    assert_eq!((grid.columns(), grid.rows(), grid.key_count()), (5, 3, 15));
}

struct Refused(&'static str);
impl Port for Refused {
    fn call(&mut self, _: rad_api::Request) -> Result<Response, Failure> {
        Ok(Response {
            v: 1,
            id: Some("1".into()),
            session_id: None,
            status: rad_client::response::Status::Error,
            data: rad_api::ResponseData::None,
            error: Some(rad_client::response::Error {
                code: rad_api::ErrorCode::ResourceExhausted,
                message: "Limit".into(),
                details: json!({"cause":self.0}),
                outcome: Some(Outcome::NotApplied),
            }),
        })
    }
}
#[test]
fn capacity_failure_retains_wire_cause_without_replaying_mutation() {
    for cause in [
        "entity_limit",
        "identity_counter_exhausted",
        "document_limit",
        "storage_capacity",
    ] {
        let mut s = Session::new(Refused(cause));
        let error = s
            .request(request(
                "create_button",
                json!({"folder_id":"f1","position":0,"name":"Button"}),
            ))
            .unwrap_err();
        assert_eq!(error.code, "resource_exhausted");
        assert!(!error.unknown);
        assert_eq!(error.cause.as_deref(), Some(cause));
        let message = rad_editor::status::describe_failure(&error, false);
        assert_ne!(message, "Limit reached");
        assert_ne!(message, rad_editor::status::describe_failure(&error, true));
        assert!(s.is_open());
    }
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

fn request(op: &str, args: serde_json::Value) -> rad_api::Request {
    rad_api::Request::parse(op, args).expect("valid typed fixture request")
}

#[test]
fn overview_notice_does_not_fetch_board_when_configuration_revision_changes() {
    let mut changed = overview();
    changed["view_revision"] = json!("3");
    let mut session = Session::new(Script(
        [
            ("overview", overview()),
            ("board", board()),
            ("overview", changed),
        ]
        .into(),
    ));
    session.refresh(true).unwrap();
    let result = session.refresh(false).unwrap();
    assert!(result.board.is_none());
    assert_eq!(result.status["view_revision"], "3");
}

#[test]
fn board_notice_fetches_only_one_coherent_board() {
    let mut session = Session::new(Script([("board", board())].into()));
    let result = session.refresh_views(true, false).unwrap();
    assert!(result.status.is_null());
    assert_eq!(result.board.unwrap().folder, board()["folder"]);
}
