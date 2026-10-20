use rad_codec::*;
use rad_core::{configuration::*, identity::*, permissions::*};
fn initial() -> (Configuration, IdentityRegistry) {
    Configuration::prepare_new(&IdentityRegistry::new(7).unwrap())
        .unwrap()
        .into_parts()
}
#[test]
fn initial_configuration_roundtrips_and_encoding_is_deterministic() {
    let (config, registry) = initial();
    let bytes = encode(&config).unwrap();
    assert_eq!(bytes, encode(&config).unwrap());
    let decoded = decode_data(&bytes).unwrap();
    assert_eq!(&decoded, config.data());
    assert_eq!(
        Configuration::from_data(decoded, &registry).unwrap(),
        config
    );
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
fn full_model_preserves_navigation_overrides_external_bytes_and_image_reference() {
    let (config, registry) = full();
    let data = decode_data(&encode(&config).unwrap()).unwrap();
    assert_eq!(&data, config.data());
    assert_eq!(Configuration::from_data(data, &registry).unwrap(), config);
}
#[test]
fn unsupported_version_is_distinct_from_malformed_document() {
    let (c, _) = initial();
    let json = String::from_utf8(encode(&c).unwrap()).unwrap();
    assert_eq!(
        decode_data(
            json.replacen("\"version\":3", "\"version\":4", 1)
                .as_bytes()
        ),
        Err(CodecError::UnsupportedVersion)
    );
}
#[test]
fn integer_strings_are_canonical_and_keep_u64_u128_maxima() {
    let (c, _) = initial();
    let json = String::from_utf8(encode(&c).unwrap()).unwrap();
    for invalid in ["01", "+1", " 1", "-1", "1.0", "1e0", "18446744073709551616"] {
        let bytes = json.replacen("\"serial\":\"1\"", &format!("\"serial\":\"{invalid}\""), 1);
        assert_eq!(
            decode_data(bytes.as_bytes()),
            Err(CodecError::InvalidValue),
            "{invalid}"
        );
    }
    let id = EntityIdentity::new(u128::MAX, u64::MAX).unwrap();
    let binding = Binding::new(ShortId::new(EntityKind::Folder, u64::MAX).unwrap(), id);
    let registry =
        IdentityRegistry::from_records(u128::MAX, [IdentityRecord::new(binding, false)]).unwrap();
    let data = ConfigurationData {
        grid: Grid::XL,
        plugin_settings: Default::default(),
        plugin_dependencies: Default::default(),
        settings: SettingsData::default(),
        principal: id,
        global_permissions: Permissions::default(),
        folders: vec![FolderData {
            binding,
            parent: None,
            slots: vec![None; 32],
        }],
        buttons: vec![],
    };
    let config = Configuration::from_data(data, &registry).unwrap();
    assert_eq!(
        decode_data(&encode(&config).unwrap()).unwrap(),
        *config.data()
    );
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
#[test]
fn duplicate_unknown_nested_fields_malformed_and_untrusted_semantics() {
    let (c, r) = full();
    let json = String::from_utf8(encode(&c).unwrap()).unwrap();
    for bad in [
        json.replacen("\"version\":3", "\"version\":3,\"version\":3", 1),
        json.replacen("\"version\":3", "\"version\":3,\"secret\":null", 1),
        json.replacen("\"serial\":\"1\"", "\"serial\":\"1\",\"serial\":\"1\"", 1),
        json.replacen("\"parent\":null", "\"parent\":null,\"parent\":null", 1),
        json.replacen("\"consult\":true", "\"consult\":true,\"extra\":false", 1),
        json.replacen("\"parameters\":", "\"unknown\":0,\"parameters\":", 1),
        json.replacen("\"number\":\"1\"", "\"number\":1", 1),
        format!("{json}[]"),
        "{".into(),
        "".into(),
    ] {
        assert!(decode_data(bad.as_bytes()).is_err());
    }
    assert_eq!(decode_data(&[255]), Err(CodecError::InvalidDocument));
    let (initial, r0) = initial();
    let mut value: serde_json::Value = serde_json::from_slice(&encode(&initial).unwrap()).unwrap();
    value["folders"][0]["slots"] = serde_json::json!([]);
    let data = decode_data(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(Configuration::from_data(data, &r0).is_err());
    assert!(Configuration::from_data(decode_data(json.as_bytes()).unwrap(), &r).is_ok());
}
#[test]
fn ipc_scalar_limit_counts_json_escaping_and_preserves_own_equal_parent() {
    let (c, r) = full();
    let mut data = c.data().clone();
    let index = data
        .buttons
        .iter()
        .position(|b| b.name == "launch")
        .unwrap();
    data.buttons[index].name = "x".repeat(8190); // +2 JSON quotes
    let at = Configuration::from_data(data.clone(), &r).unwrap();
    assert_eq!(decode_data(&encode(&at).unwrap()).unwrap(), data);
    data.buttons[index].name.push('x');
    assert_eq!(
        encode(&Configuration::from_data(data, &r).unwrap()),
        Err(CodecError::InvalidValue)
    );
    let mut value: serde_json::Value = serde_json::from_slice(&encode(&c).unwrap()).unwrap();
    value["buttons"][index]["name"] = serde_json::json!("\n".repeat(4096)); //8194 escaped
    assert_eq!(
        decode_data(&serde_json::to_vec(&value).unwrap()),
        Err(CodecError::InvalidValue)
    );
}

#[test]
fn invalid_basic_values_are_rejected_without_leaking_payload_in_errors() {
    let (c, _) = full();
    let original: serde_json::Value = serde_json::from_slice(&encode(&c).unwrap()).unwrap();
    let mut bad = original.clone();
    bad["principal"]["installation"] = serde_json::json!("0");
    assert_eq!(
        decode_data(&serde_json::to_vec(&bad).unwrap()),
        Err(CodecError::InvalidValue)
    );
    bad = original.clone();
    bad["global_permissions"] = serde_json::json!({"consult":false,"modify":true,"execute":false});
    assert_eq!(
        decode_data(&serde_json::to_vec(&bad).unwrap()),
        Err(CodecError::InvalidValue)
    );
    let index = c
        .data()
        .buttons
        .iter()
        .position(|b| b.name == "launch")
        .unwrap();
    for invalid in ["../secret.desktop", "not-desktop", "\nprivate.desktop"] {
        bad = original.clone();
        bad["buttons"][index]["actions"][0]["kind"]["launcher"] = serde_json::json!(invalid);
        let error = decode_data(&serde_json::to_vec(&bad).unwrap()).unwrap_err();
        assert_eq!(error, CodecError::InvalidValue);
        assert_eq!(format!("{error:?}"), "InvalidValue");
    }
    for invalid in ["f", "FF", "zz"] {
        bad = original.clone();
        bad["buttons"][index]["actions"][1]["kind"]["external"]["parameters"] =
            serde_json::json!(invalid);
        assert_eq!(
            decode_data(&serde_json::to_vec(&bad).unwrap()),
            Err(CodecError::InvalidValue)
        );
    }
    let json = String::from_utf8(encode(&c).unwrap()).unwrap();
    let duplicate = json.replacen(
        "\"parameters\":\"ff0080\"",
        "\"parameters\":\"ff0080\",\"parameters\":\"ff0080\"",
        1,
    );
    assert_eq!(
        decode_data(duplicate.as_bytes()),
        Err(CodecError::InvalidDocument)
    );
    assert_eq!(encode(&c).unwrap(), encode(&c).unwrap()); // failure did not mutate source
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
fn reviewer_ids_overflow_noncanonical_and_wrong_json_types_rejected() {
    let (c, _) = full();
    let original: serde_json::Value = serde_json::from_slice(&encode(&c).unwrap()).unwrap();
    for (field, bad) in [
        ("installation", "340282366920938463463374607431768211456"),
        ("installation", "00"),
        ("installation", "-0"),
        ("serial", "18446744073709551616"),
        ("serial", "١"),
        ("serial", ""),
    ] {
        let mut v = original.clone();
        v["principal"][field] = serde_json::json!(bad);
        assert_eq!(
            decode_data(&serde_json::to_vec(&v).unwrap()),
            Err(CodecError::InvalidValue)
        );
    }
    for bad in [
        serde_json::json!(1),
        serde_json::json!(null),
        serde_json::json!(true),
        serde_json::json!([]),
    ] {
        let mut v = original.clone();
        v["principal"]["serial"] = bad;
        assert_eq!(
            decode_data(&serde_json::to_vec(&v).unwrap()),
            Err(CodecError::InvalidDocument)
        );
    }
}
#[test]
fn reviewer_getter_returns_copy_and_explicit_equal_parent_remains_own() {
    let own = LocalPermissions::own(Permissions::default());
    let mut copy = own.own_value().unwrap();
    copy.set_consult(false);
    assert_eq!(own.own_value(), Some(Permissions::default()));
    let denied = Permissions::new(false, false, false).unwrap();
    assert_eq!(own.resolve(denied), Permissions::default());
    assert_eq!(LocalPermissions::default().resolve(denied), denied);
    let (c, r) = full();
    let data = decode_data(&encode(&c).unwrap()).unwrap();
    let rebuilt = Configuration::from_data(data, &r).unwrap();
    let own = rebuilt
        .data()
        .buttons
        .iter()
        .find(|b| b.name == "launch")
        .unwrap()
        .permissions;
    assert_eq!(own.own_value(), Some(c.data().global_permissions));
    assert_ne!(own.resolve(denied), denied);
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

#[test]
fn wait_action_roundtrips_as_canonical_decimal_and_rejects_out_of_range_durations() {
    let (c, r) = initial();
    let (c, r) = c
        .prepare_edit(
            &r,
            Origin::Editor,
            Edit::CreateButton {
                folder: c.data().principal,
                position: 0,
                name: "pause".into(),
            },
        )
        .unwrap()
        .into_parts();
    let (c, r) = c
        .prepare_edit(
            &r,
            Origin::Editor,
            Edit::AddAction {
                button: c.data().buttons[0].binding.identity(),
                kind: ActionKind::Wait(Wait::new(600_000).unwrap()),
            },
        )
        .unwrap()
        .into_parts();
    let bytes = encode(&c).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains(r#""kind":{"wait":"600000"}"#), "{text}");
    let decoded = decode_data(&bytes).unwrap();
    assert_eq!(Configuration::from_data(decoded, &r).unwrap(), c);
    for invalid in [r#""600001""#, r#""0600""#, r#""""#, "600000", r#""-1""#] {
        let changed = text.replace(r#"{"wait":"600000"}"#, &format!(r#"{{"wait":{invalid}}}"#));
        assert_ne!(changed, text);
        assert!(decode_data(changed.as_bytes()).is_err(), "{invalid}");
    }
}

#[test]
fn text_position_is_stored_only_when_chosen_so_earlier_documents_still_read() {
    let (c, r) = initial();
    let (c, r) = c
        .prepare_edit(
            &r,
            Origin::Editor,
            Edit::CreateButton {
                folder: c.data().principal,
                position: 0,
                name: "key".into(),
            },
        )
        .unwrap()
        .into_parts();
    // A key whose position follows its image is written exactly as before the field existed.
    let automatic = String::from_utf8(encode(&c).unwrap()).unwrap();
    assert!(
        automatic.contains(r#""appearance":{"text":"","image":null}"#),
        "{automatic}"
    );
    assert_eq!(decode_data(automatic.as_bytes()).unwrap(), *c.data());
    for (chosen, written) in [
        (TextPosition::Top, "top"),
        (TextPosition::Middle, "middle"),
        (TextPosition::Bottom, "bottom"),
        (TextPosition::AboveImage, "above_image"),
        (TextPosition::BelowImage, "below_image"),
    ] {
        let button = c.data().buttons[0].binding.identity();
        let (next, registry) = c
            .prepare_edit(
                &r,
                Origin::Editor,
                Edit::SetAppearance {
                    button,
                    appearance: Appearance {
                        text: "Go".into(),
                        image: None,
                        text_position: Some(chosen),
                    },
                },
            )
            .unwrap()
            .into_parts();
        let bytes = encode(&next).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        let expected =
            format!(r#""appearance":{{"text":"Go","image":null,"text_position":"{written}"}}"#);
        assert!(text.contains(&expected), "{text}");
        let decoded = decode_data(&bytes).unwrap();
        assert_eq!(Configuration::from_data(decoded, &registry).unwrap(), next);
    }
    let unknown = automatic.replace(
        r#""appearance":{"text":"","image":null}"#,
        r#""appearance":{"text":"","image":null,"text_position":"left"}"#,
    );
    assert!(decode_data(unknown.as_bytes()).is_err());
}

#[test]
fn gaps_between_actions_are_not_written_and_earlier_documents_read_only_when_they_were_zero() {
    let (c, r) = initial();
    let (c, r) = c
        .prepare_edit(
            &r,
            Origin::Editor,
            Edit::CreateButton {
                folder: c.data().principal,
                position: 0,
                name: "key".into(),
            },
        )
        .unwrap()
        .into_parts();
    let text = String::from_utf8(encode(&c).unwrap()).unwrap();
    assert!(!text.contains("pauses_ms"), "{text}");
    // Documents written before waiting became an action carried a gap list per button.
    let earlier = |gaps: &str| {
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["version"] = serde_json::json!(1);
        v.as_object_mut().unwrap().remove("grid");
        v.as_object_mut().unwrap().remove("plugin_settings");
        v.as_object_mut().unwrap().remove("plugin_dependencies");
        v["settings"]["gui_language"] = serde_json::json!("en");
        serde_json::to_string(&v).unwrap().replace(
            r#""actions":[]"#,
            &format!(r#""actions":[],"pauses_ms":{gaps}"#),
        )
    };
    assert_ne!(earlier("[]"), text);
    for zero in ["[]", r#"["0"]"#, r#"["0","0"]"#] {
        let decoded = decode_data(earlier(zero).as_bytes()).unwrap();
        assert_eq!(Configuration::from_data(decoded, &r).unwrap(), c, "{zero}");
    }
    // A gap that was really set cannot be honoured any more, so it is not silently dropped.
    for set in [
        r#"["5"]"#,
        r#"["0","600000"]"#,
        "[0]",
        r#"["00"]"#,
        r#""0""#,
    ] {
        assert!(decode_data(earlier(set).as_bytes()).is_err(), "{set}");
    }
}
