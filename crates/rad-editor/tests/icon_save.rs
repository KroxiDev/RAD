use rad_client::{
    connection::Failure,
    response::{Response, Status},
};
use rad_editor::{
    Request,
    session::{Port, Session},
};
use serde_json::json;
use std::{cell::RefCell, rc::Rc};
struct Server(Rc<RefCell<Vec<Request>>>);
impl Port for Server {
    fn call(&mut self, request: Request) -> Result<Response, Failure> {
        let operation = request.operation();
        let (status, data) = match operation.as_str() {
            "transfer_begin" => (Status::Ok, json!({"transfer_id":"t1"})),
            "transfer_write" => (Status::Ok, json!({})),
            "transfer_finish" => (Status::Ok, json!({"media_ref":"prepared-image"})),
            "edit" => (Status::Applied, json!({"changed":true,"revision":"2"})),
            _ => panic!("unexpected operation"),
        };
        self.0.borrow_mut().push(request);
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
fn edit(button: &str) -> Request {
    rad_editor::commands::save_button(&rad_editor::commands::KeySave {
        id: button,
        name: "Original name",
        text: "Keep my text",
        image: None,
        text_position: Some("bottom"),
        mode: "parallel",
        gesture: "hold",
        order: &[2, 0, 1],
        waits: &[("a1".into(), 123)],
        launchers: &[("a2".into(), "org.example.Player.desktop".into())],
        brightness: &[("a3".into(), rad_api::BrightnessDirection::Decrease)],
    })
}
#[test]
fn save_uploads_then_publishes_only_the_captured_key_without_losing_other_fields() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut session = Session::new(Server(calls.clone()));
    let png = rad_editor::icon_library::Renderer::default()
        .render(Default::default())
        .unwrap()
        .png;
    let response = rad_editor::icon_save::save(&mut session, "b7", &png, edit("b7")).unwrap();
    assert_eq!(response.status, Status::Applied);
    let calls = calls.borrow();
    assert_eq!(
        calls.first().unwrap().operation().as_str(),
        "transfer_begin"
    );
    assert_eq!(
        calls[calls.len() - 2].operation().as_str(),
        "transfer_finish"
    );
    let last = calls.last().unwrap().arguments();
    let mut expected = edit("b7").arguments();
    for change in expected["changes"].as_array_mut().unwrap() {
        if change["field"] == "appearance" {
            change["value"]["image"] = json!("prepared-image");
        }
    }
    assert_eq!(
        last, expected,
        "only image changes; text, position, mode, gesture and every action edit survive"
    );
}

#[test]
fn malformed_destination_is_refused_before_upload() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut session = Session::new(Server(calls.clone()));
    let mut request = edit("b7");
    let Request::Edit { changes: foreign } = edit("b9") else {
        unreachable!()
    };
    let Request::Edit { changes } = &mut request else {
        unreachable!()
    };
    changes.extend(foreign.into_iter().filter(|c| {
        matches!(
            c,
            rad_api::Change::Set {
                edit: rad_api::FieldEdit::Appearance(_),
                ..
            }
        )
    }));
    assert!(
        rad_editor::icon_save::save(&mut session, "b7", b"\x89PNG\r\n\x1a\n", request).is_err()
    );
    assert!(
        calls.borrow().is_empty(),
        "a mismatched destination cannot even upload"
    );
}

struct BrokenServer {
    calls: Rc<RefCell<Vec<Request>>>,
    stage: &'static str,
    malformed: bool,
}
impl Port for BrokenServer {
    fn call(&mut self, request: Request) -> Result<Response, Failure> {
        let operation = request.operation();
        if operation.as_str() == self.stage {
            self.calls.borrow_mut().push(request);
            if self.malformed {
                return Ok(Response {
                    v: 1,
                    id: Some("1".into()),
                    session_id: None,
                    status: Status::Ok,
                    data: rad_api::ResponseData::Empty(rad_api::Empty {}),
                    error: None,
                });
            }
            return Err(Failure {
                code: "runtime_unavailable",
                id: None,
                session_id: None,
                outcome: Some(rad_client::response::Outcome::Unknown),
            });
        }
        if operation.as_str() == "transfer_drop" {
            self.calls.borrow_mut().push(request);
            return Ok(Response {
                v: 1,
                id: Some("1".into()),
                session_id: None,
                status: Status::Ok,
                data: rad_api::ResponseData::Empty(rad_api::Empty {}),
                error: None,
            });
        }
        Server(self.calls.clone()).call(request)
    }
}
#[test]
fn upload_failures_and_malformed_finish_cannot_publish_and_unknown_edit_is_not_replayed() {
    for (stage, malformed) in [
        ("transfer_begin", false),
        ("transfer_write", false),
        ("transfer_finish", false),
        ("transfer_finish", true),
        ("edit", false),
    ] {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut session = Session::new(BrokenServer {
            calls: calls.clone(),
            stage,
            malformed,
        });
        let error =
            rad_editor::icon_save::save(&mut session, "b7", b"\x89PNG\r\n\x1a\n", edit("b7"))
                .unwrap_err();
        assert_eq!(error.unknown, !malformed);
        assert_eq!(
            calls
                .borrow()
                .iter()
                .filter(|r| matches!(r, Request::Edit { .. }))
                .count(),
            usize::from(stage == "edit")
        );
        if malformed {
            assert_eq!(
                calls.borrow().last().unwrap().operation().as_str(),
                "transfer_drop"
            );
        }
    }
}
