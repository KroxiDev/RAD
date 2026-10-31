use rad_client::{
    connection::Failure,
    response::{Response, Status},
};
use rad_editor::session::{Port, Session};
use serde_json::{Value, json};
struct Script(std::collections::VecDeque<(&'static str, Value)>);
impl Port for Script {
    fn call(&mut self, request: rad_api::Request) -> Result<Response, Failure> {
        let op = request.operation().as_str();
        let (expected, data) = self.0.pop_front().expect("extra unbounded read");
        assert_eq!(op, expected);
        Ok(Response {
            v: 1,
            id: Some("1".into()),
            session_id: None,
            status: Status::Ok,
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

#[test]
fn catalog_entries_read_their_plugin_texts_in_the_user_language() {
    use rad_editor::preferences::Language;
    let manifest = json!({"contract":1,"capabilities":["labels","visual_freshness"],"id":"org.rad.fixture",
        "version":{"major":1,"minor":0,"patch":0},"host_min":1,"host_max":1,
        "targets":["x86_64-unknown-linux-gnu"],
        "actions":[{"id":"record","label":"Recording","schema":{"version":1,"fields":[]},"credential_slots":[]}],
        "visuals":[],"translations":{"es":{"Recording":"Grabación","Toggle":"Alternar"}}});
    let item = |key: &str, plugin_version: Value| json!({"type":key,"kind":"action","label":"Recording","version":plugin_version,"ready":true});
    let version = json!({"major":1,"minor":0,"patch":0});
    let mut session = Session::new(Script(
        [
            (
                "plugin_catalog",
                json!({"items":[item("org.rad.fixture:record", version.clone()),
                    item("org.rad.other:run", version)],"next_cursor":null}),
            ),
            (
                "plugin_manifest",
                json!({"plugin_id":"org.rad.fixture","document":manifest}),
            ),
            (
                "plugin_manifest",
                json!({"plugin_id":"org.rad.other","document":{"not":"a manifest"}}),
            ),
        ]
        .into(),
    ));
    let catalog = session.plugin_catalog().unwrap();
    assert_eq!(catalog[0].shown(Language::Es), "Grabación");
    assert_eq!(catalog[0].texts.get(Language::Es, "Toggle"), "Alternar");
    assert_eq!(catalog[0].shown(Language::En), "Recording");
    // A plugin without readable translations keeps its own texts.
    assert_eq!(catalog[1].shown(Language::Es), "Recording");
}
