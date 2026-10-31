use rad_codec::*;
use rad_core::{configuration::*, identity::*, permissions::*};
fn initial() -> (Configuration, IdentityRegistry) {
    Configuration::prepare_new(&IdentityRegistry::new(7).unwrap())
        .unwrap()
        .into_parts()
}
fn full() -> (Configuration, IdentityRegistry) {
    let (c, r) = initial();
    let root = c.data().principal;
    let (c, r) = c
        .prepare_edits(
            &r,
            Origin::Editor,
            &[
                Edit::CreateFolder {
                    parent: root,
                    position: 0,
                    name: "child".into(),
                },
                Edit::CreateButton {
                    folder: root,
                    position: 1,
                    name: "launch".into(),
                },
            ],
        )
        .unwrap()
        .into_parts();
    let mut data = c.data().clone();
    let button = data
        .buttons
        .iter_mut()
        .find(|b| b.name == "launch")
        .unwrap();
    button.permissions = LocalPermissions::own(data.global_permissions);
    button.appearance = Appearance {
        text: "á\nquoted\"".into(),
        image: Some("owned-media-reference".into()),
        text_position: None,
    };
    button.visual = VisualSource::External {
        type_key: ExternalType::new("p".into(), "v".into()).unwrap(),
        configuration: vec![0, 255, 42],
    };
    let mut r = r;
    button.actions = vec![
        ActionData {
            binding: r.allocate(EntityKind::Action).unwrap(),
            kind: ActionKind::Launcher(DesktopEntry::new("app.desktop".into()).unwrap()),
            permissions: LocalPermissions::default(),
        },
        ActionData {
            binding: r.allocate(EntityKind::Action).unwrap(),
            kind: ActionKind::External {
                type_key: ExternalType::new("missing".into(), "action".into()).unwrap(),
                parameters: vec![255, 0, 128],
            },
            permissions: LocalPermissions::own(Permissions::default()),
        },
    ];
    button.mode = Mode::Parallel;
    (Configuration::from_data(data, &r).unwrap(), r)
}

#[test]
fn input_limit_is_inclusive_and_checked_before_parser() {
    let (c, _) = initial();
    let mut bytes = encode(&c).unwrap();
    bytes.resize(1024 * 1024, b' ');
    assert_eq!(decode_data(&bytes).unwrap(), *c.data());
    bytes.push(b' ');
    assert_eq!(decode_data(&bytes), Err(CodecError::TooLarge));
    assert_eq!(
        decode_data(&vec![b'!'; 1024 * 1024 + 1]),
        Err(CodecError::TooLarge)
    );
}
#[test]
fn output_limit_is_real_document_boundary_not_a_post_parse_blob() {
    let (c, r) = full();
    let mut data = c.data().clone();
    // Many valid buttons, each value small; grow total document above cap.
    let template = data
        .buttons
        .iter()
        .find(|b| b.name == "launch")
        .unwrap()
        .clone();
    let mut r = r;
    // A tree of folders provides legal slots for the repeated buttons.
    let mut config = c;
    for i in 0..160 {
        let root = config.data().folders.last().unwrap().binding.identity();
        let candidate = config
            .prepare_edit(
                &r,
                Origin::Editor,
                Edit::CreateFolder {
                    parent: root,
                    position: 2,
                    name: format!("folder{i}"),
                },
            )
            .unwrap();
        (config, r) = candidate.into_parts();
    }
    data = config.data().clone();
    for folder in &mut data.folders {
        let mut b = template.clone();
        b.binding = r.allocate(EntityKind::Button).unwrap();
        b.actions.clear();
        b.name = "x".repeat(7000);
        folder.slots[3] = Some(b.binding.identity());
        data.buttons.push(b);
    }
    let config = Configuration::from_data(data, &r).unwrap();
    assert_eq!(encode(&config), Err(CodecError::TooLarge));
    let mut data = config.data().clone();
    for b in &mut data.buttons {
        b.name = "x".repeat(1000);
    }
    let mut remaining = 1024 * 1024
        - encode(&Configuration::from_data(data.clone(), &r).unwrap())
            .unwrap()
            .len();
    for b in &mut data.buttons {
        let grow = remaining.min(8190 - b.name.len());
        b.name.push_str(&"x".repeat(grow));
        remaining -= grow;
    }
    assert_eq!(remaining, 0);
    let exact = Configuration::from_data(data.clone(), &r).unwrap();
    let bytes = encode(&exact).unwrap();
    assert_eq!(bytes.len(), 1024 * 1024);
    assert_eq!(decode_data(&bytes).unwrap(), data);
    data.buttons.last_mut().unwrap().name.push('x');
    assert_eq!(
        encode(&Configuration::from_data(data, &r).unwrap()),
        Err(CodecError::TooLarge)
    );
}

// Critical regressions promoted from the independent codec review.

fn object_paths(value: &serde_json::Value, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match value {
        serde_json::Value::Object(map) => {
            out.push(path.clone());
            for (key, child) in map {
                path.push(key.clone());
                object_paths(child, path, out);
                path.pop();
            }
        }
        serde_json::Value::Array(array) => {
            for (i, child) in array.iter().enumerate() {
                path.push(i.to_string());
                object_paths(child, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}
fn at<'a>(mut value: &'a serde_json::Value, path: &[String]) -> &'a serde_json::Value {
    for key in path {
        value = if value.is_array() {
            &value[key.parse::<usize>().unwrap()]
        } else {
            &value[key]
        };
    }
    value
}
fn render_duplicate(value: &serde_json::Value, path: &[String], key: &str) -> String {
    if path.is_empty() {
        let raw = serde_json::to_string(value).unwrap();
        return format!(
            "{{{}:{},{}",
            serde_json::to_string(key).unwrap(),
            serde_json::to_string(&value[key]).unwrap(),
            &raw[1..]
        );
    }
    match value {
        serde_json::Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!(
                    "{}:{}",
                    serde_json::to_string(k).unwrap(),
                    if k == &path[0] {
                        render_duplicate(v, &path[1..], key)
                    } else {
                        v.to_string()
                    }
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
        serde_json::Value::Array(arr) => format!(
            "[{}]",
            arr.iter()
                .enumerate()
                .map(|(i, v)| if i.to_string() == path[0] {
                    render_duplicate(v, &path[1..], key)
                } else {
                    v.to_string()
                })
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => panic!("invalid test path"),
    }
}
#[test]
fn reviewer_every_nested_object_rejects_unknown_and_duplicate_fields() {
    let (c, _) = full();
    let original: serde_json::Value = serde_json::from_slice(&encode(&c).unwrap()).unwrap();
    let mut paths = vec![];
    object_paths(&original, &mut vec![], &mut paths);
    let mut duplicates = 0;
    for path in paths {
        let object = at(&original, &path).as_object().unwrap();
        for key in object.keys() {
            let raw = render_duplicate(&original, &path, key);
            assert_eq!(
                decode_data(raw.as_bytes()),
                Err(CodecError::InvalidDocument),
                "duplicate {path:?}/{key}"
            );
            duplicates += 1;
        }
        let mut changed = original.clone();
        let mut target = &mut changed;
        for key in &path {
            target = if target.is_array() {
                &mut target[key.parse::<usize>().unwrap()]
            } else {
                &mut target[key]
            };
        }
        target
            .as_object_mut()
            .unwrap()
            .insert("__unknown".into(), serde_json::Value::Null);
        assert_eq!(
            decode_data(&serde_json::to_vec(&changed).unwrap()),
            Err(CodecError::InvalidDocument),
            "unknown {path:?}"
        );
    }
    assert!(duplicates > 80);
    println!("duplicate field cases={duplicates}");
}
#[test]
fn reviewer_scalar_hex_and_escaping_boundaries_both_directions() {
    let (c, r) = full();
    let mut data = c.data().clone();
    let index = data
        .buttons
        .iter()
        .position(|b| b.name == "launch")
        .unwrap();
    for raw in [4095, 4096] {
        data.buttons[index].actions[1].kind = ActionKind::External {
            type_key: ExternalType::new("p".into(), "a".into()).unwrap(),
            parameters: vec![255; raw],
        };
        let c = Configuration::from_data(data.clone(), &r).unwrap();
        if raw == 4095 {
            assert_eq!(decode_data(&encode(&c).unwrap()).unwrap(), data);
        } else {
            assert_eq!(encode(&c), Err(CodecError::InvalidValue));
        }
        let mut wire: serde_json::Value =
            serde_json::from_slice(&encode(&full().0).unwrap()).unwrap();
        wire["buttons"][index]["actions"][1]["kind"]["external"]["parameters"] =
            serde_json::json!("ff".repeat(raw));
        assert_eq!(
            decode_data(&serde_json::to_vec(&wire).unwrap()).is_ok(),
            raw == 4095
        );
    }
    let (c, r) = full();
    let mut data = c.data().clone();
    for (value, valid) in [
        ("\n".repeat(4095), true),
        ("\n".repeat(4096), false),
        ("\0".repeat(1365), true),
        ("\0".repeat(1366), false),
        ("é".repeat(4095), true),
        ("é".repeat(4096), false),
    ] {
        data.buttons[index].appearance.text = value.clone();
        let config = Configuration::from_data(data.clone(), &r).unwrap();
        assert_eq!(encode(&config).is_ok(), valid);
        let mut wire: serde_json::Value = serde_json::from_slice(&encode(&c).unwrap()).unwrap();
        wire["buttons"][index]["appearance"]["text"] = serde_json::json!(value);
        assert_eq!(
            decode_data(&serde_json::to_vec(&wire).unwrap()).is_ok(),
            valid
        );
    }
}

#[test]
fn reviewer_decoded_semantic_invalidity_is_not_valid_domain_and_recursion_is_bounded() {
    let (c, r) = initial();
    let mut value: serde_json::Value = serde_json::from_slice(&encode(&c).unwrap()).unwrap();
    value["principal"]["serial"] = serde_json::json!("99");
    let dto = decode_data(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(Configuration::from_data(dto, &r).is_err());
    let nested = format!("{}0{}", "[".repeat(200), "]".repeat(200));
    assert_eq!(
        decode_data(nested.as_bytes()),
        Err(CodecError::InvalidDocument)
    );
}
