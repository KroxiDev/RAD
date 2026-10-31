use rad_editor::commands::{KeySave, save_button};
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
