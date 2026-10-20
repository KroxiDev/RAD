use rad_client::{
    connection::Failure,
    response::{Outcome, Response},
};
use rad_editor::{
    Request,
    session::{Port, Session},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
struct Failing(Arc<AtomicUsize>);
impl Port for Failing {
    fn request(&mut self, _: &str, _: Value) -> Result<Response, Failure> {
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
    let error = s.request(Request::new("undo", json!({}))).unwrap_err();
    assert!(error.unknown);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(s.request(Request::new("status", json!({}))).is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
struct Script(std::collections::VecDeque<(&'static str, Value)>);
impl Port for Script {
    fn request(&mut self, op: &str, _: Value) -> Result<Response, Failure> {
        let (expected, data) = self.0.pop_front().expect("unexpected request");
        assert_eq!(op, expected);
        Ok(Response {
            v: 1,
            id: Some("1".into()),
            session_id: None,
            status: rad_client::response::Status::Ok,
            data,
            error: None,
        })
    }
}
#[test]
fn unchanged_status_does_not_reload_deck_and_changed_board_is_complete() {
    let status =
        json!({"view_revision":"2","active_folder_id":"f1","settings":{"gui_language":"en"}});
    let mut slots = vec![Value::Null; 32];
    slots[3] = json!({"visibility":"visible","id":"b8"});
    let folder = json!({"id":"f1","kind":"folder","fields":{"slots":slots},"view_revision":"2"});
    let button = json!({"id":"b8","kind":"button","fields":{"name":"Browser"},"view_revision":"2"});
    let mut refreshed = status.clone();
    refreshed["deck_state"] = json!("disconnected");
    refreshed["recovered_previous"] = json!(true);
    let mut s = Session::new(Script(
        [
            ("status", status.clone()),
            ("get", folder.clone()),
            ("get", button.clone()),
            (
                "get_section",
                json!({"view_revision":"2","items":[],"next_cursor":null}),
            ),
            ("status", refreshed.clone()),
        ]
        .into(),
    ));
    let first = s.refresh(false).unwrap().board.unwrap();
    assert_eq!(first.folder, folder);
    assert_eq!(first.buttons[0].entity, button);
    let next = s.refresh(false).unwrap();
    assert!(next.board.is_none());
    assert_eq!(next.status, refreshed);
    assert_eq!(
        rad_editor::status::deck_label(&next.status, false),
        "Deck disconnected — editor available"
    );
    assert!(rad_editor::status::recovery_notice(&next.status, false).is_some());
}
#[test]
fn inspector_assembles_runtime_sections_without_mixing_revisions() {
    let button = json!({"id":"b8","kind":"button","view_revision":"2","fields":{"name":"Browser"},"sections":["appearance","actions"]});
    let appearance = json!({"section":"appearance","items":[{"name":"text","value":"Go"},{"name":"image","value":null}],"next_cursor":null,"view_revision":"2"});
    let action = json!({"id":"a4","kind":"action","fields":{"type":"launcher","parameters":{"desktop_entry":"browser.desktop"}}});
    let actions = json!({"section":"actions","items":[action.clone()],"next_cursor":null,"view_revision":"2"});
    let mut s = Session::new(Script(
        [
            ("get", button.clone()),
            ("get_section", appearance.clone()),
            ("get_section", actions),
        ]
        .into(),
    ));
    let view = s.inspect("b8").unwrap();
    assert_eq!(view.appearance, json!({"text":"Go","image":null}));
    assert_eq!(view.actions, vec![action]);
    let mut changed = appearance;
    changed["view_revision"] = json!("3");
    let mut s = Session::new(Script([("get", button), ("get_section", changed)].into()));
    assert_eq!(s.inspect("b8").unwrap_err().code, "cursor_expired");
}

#[test]
fn board_loads_text_and_owned_image_reference_for_rendering() {
    let mut slots = vec![Value::Null; 32];
    slots[0] = json!({"visibility":"visible","id":"b1"});
    let mut s=Session::new(Script([
        ("status",json!({"view_revision":"2","active_folder_id":"f1"})),
        ("get",json!({"id":"f1","view_revision":"2","fields":{"slots":slots}})),
        ("get",json!({"id":"b1","view_revision":"2","fields":{"name":"App"}})),
        ("get_section",json!({"view_revision":"2","items":[{"name":"text","value":"Go"},{"name":"image","value":"owned-media"},{"name":"text_position","value":"bottom"}],"next_cursor":null})),
    ].into()));
    let board = s.refresh(false).unwrap().board.unwrap();
    assert_eq!(
        board.buttons[0].appearance,
        json!({"text":"Go","image":"owned-media","text_position":"bottom"})
    );
}
#[test]
fn media_preview_uses_entity_authorization_and_one_revision() {
    let entity = json!({"id":"b8","view_revision":"2"});
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
    let inspect = |count: usize| {
        let action = json!({"id":"a4","kind":"action","fields":{"type":"launcher"}});
        let mut s = Session::new(Script(
            [
                ("get", json!({"id":"b8","kind":"button","view_revision":"2","fields":{"name":"B"}})),
                (
                    "get_section",
                    json!({"section":"appearance","items":[],"next_cursor":null,"view_revision":"2"}),
                ),
                (
                    "get_section",
                    json!({"section":"actions","items":vec![action; count],"next_cursor":null,"view_revision":"2"}),
                ),
            ]
            .into(),
        ));
        s.inspect("b8").map(|view| view.actions.len())
    };
    assert_eq!(inspect(20).unwrap(), 20);
    assert!(inspect(21).is_err());
}
