//! Failures bound to one request, plugin or device are contained where they happen: the
//! turn keeps going and what clients can see stays coherent (ADR 0011).
use super::*;
use rad_api::{DeckState, EventCode, NoticeKind};
use std::{
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
};

pub(super) struct Directory(pub(super) std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn runtime(name: &str, session: u128) -> (Directory, Server) {
    let root = Directory(
        std::env::temp_dir().join(format!("rad-contained-{}-{name}", std::process::id())),
    );
    std::fs::create_dir(&root.0).unwrap();
    std::fs::set_permissions(&root.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let server = Server::new(
        &root.0,
        &root.0.join("data"),
        NonZeroU128::new(session).unwrap(),
    )
    .unwrap();
    (root, server)
}

/// A client with its `hello` taken by the server: a started request, ready to be answered.
pub(super) fn started_request(
    root: &Directory,
    server: &mut Server,
) -> (UnixStream, rad_transport::RequestKey) {
    let mut socket = UnixStream::connect(root.0.join("rad/editor.sock")).unwrap();
    socket.set_nonblocking(true).unwrap();
    let hello = br#"{"v":1,"id":"1","op":"hello","args":{}}"#;
    socket
        .write_all(&(hello.len() as u32).to_be_bytes())
        .unwrap();
    socket.write_all(hello).unwrap();
    server.transport.drive(1).unwrap();
    let incoming = server.transport.take_next().unwrap();
    (socket, incoming.key)
}

pub(super) fn read_frame(server: &mut Server, socket: &mut UnixStream, now: u64) -> Value {
    server.transport.drive(now).unwrap();
    let mut bytes = [0; 8192];
    let read = socket.read(&mut bytes).unwrap();
    let length = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
    assert!(read >= 4 + length, "a whole frame was written");
    serde_json::from_slice(&bytes[4..4 + length]).unwrap()
}

#[test]
fn a_display_clock_overflow_does_not_end_the_runtime_nor_put_the_deck_to_sleep() {
    let (_root, mut server) = runtime("overflow", 901);
    server.effects(vec![Effect::DisplayClockOverflow]).unwrap();
    assert!(!server.asleep);
    server.turn().unwrap();
    assert_eq!(
        recorded(&server).last(),
        Some(&EventCode::DisplayClockOverflow)
    );
}

#[test]
fn a_deck_that_cannot_start_leaves_the_runtime_serving_with_an_inaccessible_deck() {
    let (root, server) = runtime("deck", 902);
    // The user asked for the deck: the setting is on.
    crate::machine_settings::MachineSettings {
        use_stream_deck: true,
        start_with_session: false,
    }
    .save(&root.0.join("data"))
    .unwrap();
    drop(server);
    let mut server = Server::new(
        &root.0,
        &root.0.join("data"),
        NonZeroU128::new(902).unwrap(),
    )
    .unwrap();
    server
        .attach_deck_with(|_, _| Err(rad_deck::DeckError::Unavailable))
        .unwrap();
    assert!(server.deck.is_none());
    server.turn().unwrap();
    // Not "disabled": the user asked for the deck and it could not be had.
    assert_eq!(server.overview().deck, DeckState::Inaccessible);
    assert_eq!(server.deck_state(), DeckState::Inaccessible);
    assert_eq!(recorded(&server).last(), Some(&EventCode::DeckSpawnFailed));
}

#[test]
fn a_response_the_transport_refuses_is_answered_with_a_generic_error() {
    let (root, mut server) = runtime("oversized", 903);
    let (mut socket, key) = started_request(&root, &mut server);
    let oversized = Response {
        v: 1,
        id: Some(key.id().to_string()),
        session_id: Some(format!("{:032x}", server.session)),
        status: Status::Ok,
        data: rad_api::ResponseData::PluginLog(rad_api::PluginLogView {
            plugin_id: "org.rad.test".into(),
            lines: vec!["x".repeat(1000); 100],
        }),
        error: None,
    };
    server.respond(key, oversized).unwrap();
    let frame = read_frame(&mut server, &mut socket, 2);
    assert_eq!(frame["status"], "error");
    assert_eq!(frame["error"]["code"], "resource_exhausted");
    assert_eq!(frame["id"], key.id().to_string());
    assert_eq!(server.pending_requests(), 0);
    assert_eq!(recorded(&server).last(), Some(&EventCode::ResponseRefused));
}

#[test]
fn an_oversized_answer_to_a_change_does_not_claim_it_was_not_applied() {
    let (root, mut server) = runtime("oversized-change", 904);
    let (mut socket, key) = started_request(&root, &mut server);
    let oversized = Response {
        v: 1,
        id: Some(key.id().to_string()),
        session_id: Some(format!("{:032x}", server.session)),
        status: Status::Applied,
        data: rad_api::ResponseData::PluginLog(rad_api::PluginLogView {
            plugin_id: "org.rad.test".into(),
            lines: vec!["x".repeat(1000); 100],
        }),
        error: None,
    };
    server.respond(key, oversized).unwrap();
    let frame = read_frame(&mut server, &mut socket, 2);
    assert_eq!(frame["error"]["code"], "resource_exhausted");
    assert_eq!(frame["error"]["outcome"], "unknown");
}

#[test]
fn a_notice_that_cannot_be_queued_is_dropped_and_the_runtime_goes_on() {
    let (root, mut server) = runtime("notice", 905);
    let (_socket, key) = started_request(&root, &mut server);
    // The peer never subscribed: the transport refuses the notice.
    server
        .send_notice(key.connection(), NoticeKind::Board)
        .unwrap();
    server.turn().unwrap();
    assert_eq!(recorded(&server).last(), Some(&EventCode::NoticeDropped));
}

#[test]
fn answering_a_request_twice_does_not_end_the_runtime() {
    let (root, mut server) = runtime("twice", 906);
    let (mut socket, key) = started_request(&root, &mut server);
    let answer = || server_error(key);
    server.respond(key, answer()).unwrap();
    server.respond(key, answer()).unwrap();
    assert_eq!(read_frame(&mut server, &mut socket, 2)["status"], "error");
    assert_eq!(
        recorded(&server).last(),
        Some(&EventCode::ResponseUnknownRequest)
    );
}

fn context(request_id: u64) -> IncomingContext {
    IncomingContext {
        request_id,
        connection: 7,
        origin: Origin::Editor,
    }
}

#[test]
fn an_answer_that_cannot_be_built_is_a_generic_error_that_keeps_the_outcome_honest() {
    use rad_authority::Rejection;
    let (_root, mut server) = runtime("unbuildable", 920);
    let request = server.actor.reserve(7, Origin::Editor).unwrap().request();
    // What was applied, or is unknown, is never answered "not applied".
    for (effect, outcome) in [
        (
            Effect::Reply {
                request,
                changed: true,
            },
            Some(Outcome::Unknown),
        ),
        (Effect::Unknown { request }, Some(Outcome::Unknown)),
        (
            Effect::Rejected {
                request,
                reason: Rejection::Invalid,
            },
            Some(Outcome::NotApplied),
        ),
    ] {
        let response = server.response_or_generic(None, context(5), &effect);
        assert_eq!(response.status, Status::Error);
        assert_eq!(response.id.as_deref(), Some("5"));
        let error = response.error.unwrap();
        assert_eq!(error.code, rad_api::ErrorCode::ResourceExhausted);
        assert_eq!(error.outcome, outcome, "{effect:?}");
    }
    assert_eq!(
        recorded(&server),
        [
            EventCode::Started,
            EventCode::ResponseUnbuildable,
            EventCode::ResponseUnbuildable,
            EventCode::ResponseUnbuildable
        ]
    );
    // An answer that could be built goes through untouched and says nothing.
    let built = Response {
        v: 1,
        id: Some("5".into()),
        session_id: None,
        status: Status::Ok,
        data: rad_api::ResponseData::Empty(rad_api::Empty {}),
        error: None,
    };
    let kept = server.response_or_generic(Some(built.clone()), context(5), &Effect::Wake);
    assert_eq!(kept, built);
    assert_eq!(recorded(&server).len(), 4);
}

#[test]
fn an_answer_from_a_plugin_that_does_not_decode_is_an_error_not_the_end_of_the_runtime() {
    let (_root, mut server) = runtime("plugin-answer", 921);
    let good = json!({"plugin_id":"org.rad.test","lines":[]});
    let ok = server.plugin_response(context(3), rad_api::Operation::PluginLog, false, good);
    assert_eq!(ok.status, Status::Ok);
    assert_eq!(recorded(&server), [EventCode::Started]);
    // A read says nothing about outcome; a change that was submitted is unknown.
    for (mutation, outcome) in [(false, None), (true, Some(Outcome::Unknown))] {
        let bad = server.plugin_response(
            context(4),
            rad_api::Operation::PluginLog,
            mutation,
            json!({"lines": 7}),
        );
        assert_eq!(bad.status, Status::Error);
        assert_eq!(bad.id.as_deref(), Some("4"));
        let error = bad.error.unwrap();
        assert_eq!(error.code, rad_api::ErrorCode::ExecutorUnavailable);
        assert_eq!(error.outcome, outcome);
    }
    assert_eq!(
        recorded(&server)[1..],
        [
            EventCode::PluginResponseInvalid,
            EventCode::PluginResponseInvalid
        ]
    );
}

#[test]
fn a_catalog_page_that_does_not_decode_is_an_error_not_the_end_of_the_runtime() {
    let (_root, mut server) = runtime("catalog-answer", 922);
    let ok = server.launcher_response(context(3), json!({"items":[],"next_cursor":null}));
    assert_eq!(ok.status, Status::Ok);
    let bad = server.launcher_response(context(4), json!({"items": 3}));
    assert_eq!(bad.status, Status::Error);
    let error = bad.error.unwrap();
    assert_eq!(error.code, rad_api::ErrorCode::ServiceUnavailable);
    assert_eq!(error.outcome, None);
    assert_eq!(recorded(&server)[1..], [EventCode::CatalogResponseInvalid]);
}

#[test]
fn an_export_whose_snapshot_cannot_be_made_is_an_error_for_that_export_only() {
    let (_root, mut server) = runtime("export-failed", 923);
    let lease = server.actor.pin_export(7, Origin::Editor).unwrap();
    let response = server
        .export_begin(
            context(6),
            None,
            lease,
            Err(rad_core::configuration::ConfigurationError::InvalidValue),
        )
        .err()
        .expect("the export is answered with an error");
    assert_eq!(response.status, Status::Error);
    assert_eq!(response.id.as_deref(), Some("6"));
    let error = response.error.unwrap();
    assert_eq!(error.code, rad_api::ErrorCode::ResourceExhausted);
    // Nothing was started, so nothing is left half done.
    assert_eq!(error.outcome, Some(Outcome::NotApplied));
    assert_eq!(recorded(&server).last(), Some(&EventCode::ExportFailed));
    // The lease went back: the same connection can export again.
    assert!(server.actor.pin_export(7, Origin::Editor).is_ok());
}

fn recorded(server: &Server) -> Vec<EventCode> {
    server
        .events
        .view()
        .entries
        .iter()
        .map(|e| e.code)
        .collect()
}

fn server_error(key: rad_transport::RequestKey) -> Response {
    Response {
        v: 1,
        id: Some(key.id().to_string()),
        session_id: None,
        status: Status::Error,
        data: rad_api::ResponseData::None,
        error: Some(Error {
            code: rad_api::ErrorCode::ResourceExhausted,
            message: "Request rejected".into(),
            details: json!({}),
            outcome: None,
        }),
    }
}
