use rad_editor::plugins::FormDraft;
use rad_plugin_contract::{Definition, Field, FieldKind, Schema};
fn definition() -> Definition {
    Definition {
        gestures: rad_plugin_contract::default_gestures(),
        response_timeout: None,
        id: "run".into(),
        label: "Run".into(),
        schema: Schema {
            version: 1,
            fields: vec![
                Field {
                    id: "count".into(),
                    label: "Count".into(),
                    required: true,
                    kind: FieldKind::Integer { min: 1, max: 3 },
                },
                Field {
                    id: "flag".into(),
                    label: "Flag".into(),
                    required: false,
                    kind: FieldKind::Boolean,
                },
                Field {
                    id: "text".into(),
                    label: "Text".into(),
                    required: false,
                    kind: FieldKind::Text { max_bytes: 2 },
                },
                Field {
                    id: "choice".into(),
                    label: "Choice".into(),
                    required: false,
                    kind: FieldKind::Choice {
                        options: vec!["one".into(), "two".into()],
                    },
                },
            ],
        },
        credential_slots: vec![],
    }
}
#[test]
fn drafts_preserve_opaque_until_explicit_valid_edit_and_share_contract_validation() {
    let bytes = br#"{ "schema_version":1, "values":{"count":2} }"#.to_vec();
    let mut f = FormDraft::load(definition(), bytes.clone());
    assert_eq!(f.original, bytes);
    assert_eq!(f.publish(), Ok(None));
    f.set(0, Some("9".into()));
    assert!(f.publish().is_err());
    f.set(0, Some("3".into()));
    let value = f.publish().unwrap().unwrap();
    assert_eq!(value["values"]["count"], 3);
    assert!(value["values"].get("flag").is_none());
    for (i, value) in [(1, "not_bool"), (2, "éé"), (3, "unknown")] {
        let mut invalid = f.clone();
        invalid.set(i, Some(value.into()));
        assert!(invalid.publish().is_err());
    }
    let mut wrong = definition();
    wrong.schema.version = 2;
    let mut f = FormDraft::load(wrong, bytes);
    assert_eq!(f.publish(), Ok(None));
    f.set(0, Some("1".into()));
    assert!(f.publish().is_err());
    let mut unknown = FormDraft::load(
        definition(),
        br#"{"schema_version":1,"values":{"count":2,"legacy":true}}"#.to_vec(),
    );
    unknown.set(0, Some("1".into()));
    assert!(unknown.publish().is_err());
    assert!(
        String::from_utf8(unknown.original)
            .unwrap()
            .contains("legacy")
    );
}

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
fn visual_chunk_must_advance_and_never_repeats_empty_chunk() {
    let mut s=Session::new(Script(vec![
 ("plugin_visual",json!({"text_position":"bottom","availability":"current","revision":"1","text":null,"color":null,"image":true})),
 ("plugin_visual_read",json!({"hex":"00","next_offset":"1"})),
 ("plugin_visual_read",json!({"hex":"","next_offset":"1"})),
 ].into()));
    assert!(s.plugin_visuals(&["b1".into()]).is_err());
}

#[test]
fn paged_configuration_is_exact_and_visual_forms_do_not_add_actions() {
    let raw = br#"{ "schema_version":1, "values":{"count":2} }"#;
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let def = definition();
    let mut s=Session::new(Script(vec![
 ("plugin_configuration",json!({"type":"fixture#status","encoding":"hex","items":[&hex[..20]],"next_cursor":"next"})),
 ("plugin_configuration",json!({"type":"fixture#status","encoding":"hex","items":[&hex[20..]],"next_cursor":null})),
 ("schema",json!({"type":"fixture#status","schema_version":1,"items":def.schema.fields,"credential_slots":[],"next_cursor":null})),
 ].into()));
    let loaded = s.plugin_form("", Some("b1")).unwrap();
    assert_eq!(loaded.original, raw);
    let form = loaded.form.unwrap();
    assert_eq!(form.publish(), Ok(None));
    let request = rad_editor::plugins::FormTarget::Visual("b1".into()).request(
        "fixture#status",
        json!({"schema_version":1,"values":{"count":2}}),
    );
    assert_eq!(request.operation().as_str(), "edit");
    assert_eq!(request.arguments()["changes"][0]["field"], "visual");
    assert_eq!(request.arguments()["changes"].as_array().unwrap().len(), 1);
    let mut fresh = FormDraft::new(definition());
    assert!(fresh.publish().is_err());
    fresh.set(0, Some("2".into()));
    assert!(fresh.publish().unwrap().is_some());
    let mut credentials = definition();
    credentials.credential_slots.push("credential".into());
    let mut f = FormDraft::new(credentials);
    f.set(0, Some("2".into()));
    assert_eq!(
        f.publish().unwrap().unwrap(),
        json!({"schema_version":1,"values":{"count":2}})
    );
}
#[test]
fn malformed_visual_png_and_excessive_pixels_are_rejected_off_ui() {
    use image::ImageEncoder;
    assert!(rad_editor::preview::decode(&vec![0; 512 * 1024 + 1]).is_err());
    let mut png = vec![];
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&vec![0; 257 * 4], 257, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    assert!(rad_editor::preview::decode(&png).is_err());
}
#[test]
fn unchanged_visual_revision_reuses_pixels_without_reloading_png() {
    let mut s = Session::new(Script(
        vec![(
            "plugin_visual",
            json!({"text_position":"bottom","availability":"stale","revision":"3","text":"last","color":null,"image":true}),
        )]
        .into(),
    ));
    let revisions = std::collections::BTreeMap::from([("b1".into(), "3".into())]);
    let views = s.plugin_visuals_since(&["b1".into()], &revisions).unwrap();
    assert_eq!(views[0].availability, "stale");
    assert!(views[0].pixels.is_none());
}
#[test]
fn one_bad_visual_does_not_hide_a_healthy_sibling() {
    let mut s=Session::new(Script(vec![
 ("plugin_visual",json!({"text_position":"bottom","availability":"current","revision":"1","text":null,"color":null,"image":true})),
 ("plugin_visual_read",json!({"hex":"00","next_offset":null})),
 ("plugin_visual",json!({"text_position":"bottom","availability":"current","revision":"2","text":"healthy","color":[1,2,3,255],"image":false})),
 ].into()));
    let views = s
        .plugin_visuals_isolated(&["b1".into(), "b2".into()], &Default::default())
        .unwrap();
    assert_eq!(views.len(), 2);
    assert_eq!(views[1].text.as_deref(), Some("healthy"));
    assert_eq!(views[1].availability, "current");
}
#[test]
fn navigation_and_twenty_actions_do_not_offer_plugin_add_forms() {
    use rad_editor::plugins::accepts_action;
    assert!(accepts_action(&[]));
    assert!(!accepts_action(&[json!({"fields":{"type":"return"}})]));
    assert!(!accepts_action(&[
        json!({"fields":{"type":"enter_folder"}})
    ]));
    assert!(!accepts_action(&vec![
        json!({"fields":{"type":"fixture:run"}});
        20
    ]));
    assert!(accepts_action(&vec![
        json!({"fields":{"type":"fixture:run"}});
        19
    ]));
}

#[test]
fn plugin_settings_use_manifest_schema_and_preserve_opaque_unknown_values() {
    let mut manifest: rad_plugin_contract::Manifest = serde_json::from_str(include_str!(
        "../../rad-plugin-host/fixture/manifest-v1.json"
    ))
    .unwrap();
    manifest.settings = Some(definition().schema);
    manifest
        .capabilities
        .push(rad_plugin_contract::SETTINGS.into());
    for values in [json!({"count":2}), json!({"count":2,"future":true})] {
        let bytes = serde_json::to_vec(&json!({"schema_version":1,"values":values})).unwrap();
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut session = Session::new(Script(
            [
                (
                    "plugin_settings",
                    json!({"plugin_id":manifest.id,"encoding":"hex","hex":hex}),
                ),
                (
                    "plugin_manifest",
                    json!({"plugin_id":manifest.id,"document":manifest}),
                ),
            ]
            .into(),
        ));
        let loaded = session.plugin_form(&manifest.id, None).unwrap();
        assert_eq!(loaded.original, bytes);
        let mut form = loaded.form.unwrap();
        assert_eq!(form.publish(), Ok(None));
        assert_eq!(form.compatible(), values.get("future").is_none());
        form.set(0, Some("3".into()));
        assert_eq!(form.publish().is_ok(), values.get("future").is_none());
    }
}

#[test]
fn settings_publication_is_one_typed_edit_without_credentials() {
    let request = rad_editor::plugins::FormTarget::Settings("org.rad.fixture".into()).request(
        "org.rad.fixture",
        json!({"schema_version":1,"values":{"port":4455}}),
    );
    assert!(matches!(
        request,
        rad_api::Request::SetPluginSettings { .. }
    ));
    assert_eq!(
        request.arguments(),
        json!({"plugin_id":"org.rad.fixture","parameters":{"schema_version":1,"values":{"port":4455}}})
    );
}

#[test]
fn missing_plugin_retains_settings_bytes_without_an_editable_form() {
    struct Missing(Script);
    impl Port for Missing {
        fn call(&mut self, request: rad_api::Request) -> Result<Response, Failure> {
            if matches!(request, rad_api::Request::PluginManifest { .. }) {
                Ok(Response {
                    v: 1,
                    id: Some("1".into()),
                    session_id: None,
                    status: rad_api::Status::Error,
                    data: rad_api::ResponseData::None,
                    error: Some(rad_api::Error {
                        code: rad_api::ErrorCode::NotFound,
                        message: String::new(),
                        details: json!({}),
                        outcome: None,
                    }),
                })
            } else {
                self.0.call(request)
            }
        }
    }
    let bytes = b"opaque future plugin settings";
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut session = Session::new(Missing(Script(
        [(
            "plugin_settings",
            json!({"plugin_id":"org.rad.missing","encoding":"hex","hex":hex}),
        )]
        .into(),
    )));
    let loaded = session.plugin_form("org.rad.missing", None).unwrap();
    assert_eq!(loaded.original, bytes);
    assert!(loaded.form.is_none());
    assert!(session.is_open());
}
