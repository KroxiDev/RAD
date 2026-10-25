use rad_editor::commands::{KeySave, add_action, save_button};
use serde_json::json;
#[test]
fn save_is_one_atomic_edit_with_current_order_waits_and_brightness_direction() {
    let request = save_button(&KeySave {
        id: "b7",
        name: "Browser",
        text: "Go",
        image: None,
        text_position: None,
        mode: "parallel",
        gesture: "long_press",
        order: &[2, 1, 0],
        waits: &[("a9".into(), 1500)],
        launchers: &[("a8".into(), "browser.desktop".into())],
        brightness: &[("a10".into(), rad_api::BrightnessDirection::Decrease)],
    });
    assert_eq!(request.operation().as_str(), "edit");
    assert_eq!(
        request.arguments(),
        json!({"changes":[
            {"kind":"set","id":"b7","field":"name","value":"Browser"},
            {"kind":"set","id":"b7","field":"appearance","value":{"text":"Go","image":null}},
            {"kind":"set","id":"b7","field":"mode","value":"parallel"},
            {"kind":"set","id":"b7","field":"gesture","value":"long_press"},
            {"kind":"set","id":"a8","field":"desktop_entry","value":"browser.desktop"},
            {"kind":"set","id":"a9","field":"milliseconds","value":1500},
            {"kind":"set","id":"a10","field":"parameters","value":{"direction":"decrease"}},
            {"kind":"order","id":"b7","indices":[2,1,0]}
        ]})
    );
}
#[test]
fn save_names_the_text_position_only_when_one_was_chosen() {
    let save = |text_position| {
        save_button(&KeySave {
            id: "b7",
            name: "Browser",
            text: "Go",
            image: Some("owned-media"),
            text_position,
            mode: "sequential",
            gesture: "press",
            order: &[],
            waits: &[],
            launchers: &[],
            brightness: &[],
        })
        .arguments()["changes"][1]["value"]
            .clone()
    };
    assert_eq!(save(None), json!({"text":"Go","image":"owned-media"}));
    assert_eq!(
        save(Some("top")),
        json!({"text":"Go","image":"owned-media","text_position":"top"})
    );
}
#[test]
fn adding_an_action_names_its_type_and_parameters_for_the_runtime_to_validate() {
    for (kind, parameters) in [
        ("wait", json!({"milliseconds":1000})),
        ("brightness", json!({"direction":"increase"})),
        ("brightness", json!({"direction":"decrease"})),
    ] {
        let request = add_action("b7", kind, parameters.clone());
        assert_eq!(request.operation().as_str(), "add_action");
        assert_eq!(
            request.arguments(),
            json!({"button_id":"b7","type":kind,"parameters":parameters})
        );
    }
}
#[test]
fn permission_controls_emit_consistent_explicit_own_values() {
    use rad_editor::commands::{PermissionField, permission_change};
    let own = json!({"own":{"consult":true,"modify":true,"execute":true}});
    let off = permission_change(&own, PermissionField::Consult, false);
    assert_eq!(
        off,
        json!({"own":{"consult":false,"modify":false,"execute":false}})
    );
    assert_eq!(
        permission_change(&off, PermissionField::Execute, true),
        json!({"own":{"consult":true,"modify":false,"execute":true}})
    );
}
#[test]
fn deleting_folder_entry_targets_folder_not_protected_navigation_button() {
    let actions = vec![json!({"fields":{"type":"enter_folder","folder_id":"f2"}})];
    assert_eq!(
        rad_editor::commands::delete_selection("b3", &actions).arguments(),
        json!({"id":"f2","confirm_contents":true})
    );
}

#[test]
fn moving_folder_uses_entrance_never_return_destination() {
    use rad_editor::commands::move_folder;
    let returning = vec![json!({"fields":{"type":"return","folder_id":"f2"}})];
    assert_eq!(
        move_folder(&returning, "f1", 4),
        None,
        "Return must not move its parent folder"
    );
    let entering = vec![json!({"fields":{"type":"enter_folder","folder_id":"f3"}})];
    let request = move_folder(&entering, "f1", 4).unwrap();
    assert_eq!(request.operation().as_str(), "move_folder");
    assert_eq!(
        request.arguments(),
        json!({"id":"f3","parent_id":"f1","position":4})
    );
}
