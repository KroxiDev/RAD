//! Closed UI request builders, matching rad-protocol API. Validation stays server-side.
use crate::Request;
use serde_json::{Value, json};
/// Everything Save sends for one key; the runtime applies it as a single edit.
pub struct KeySave<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub text: &'a str,
    pub image: Option<&'a str>,
    /// `top`, `middle` or `bottom` when the user chose one; otherwise it is left as it is.
    pub text_position: Option<&'a str>,
    pub mode: &'a str,
    pub gesture: &'a str,
    pub order: &'a [usize],
    /// Wait actions of the key, by identifier, with their durations in milliseconds.
    pub waits: &'a [(String, u64)],
    /// Launcher actions and the applications selected for them.
    pub launchers: &'a [(String, String)],
}
pub fn save_button(save: &KeySave) -> Request {
    use rad_api::{Appearance, Change, FieldEdit, Gesture, Mode, TextPosition};
    let text_position = save.text_position.map(|p| match p {
        "top" => Some(TextPosition::Top),
        "middle" => Some(TextPosition::Middle),
        "bottom" => Some(TextPosition::Bottom),
        "above_image" => Some(TextPosition::AboveImage),
        "below_image" => Some(TextPosition::BelowImage),
        _ => None,
    });
    let set = |id: &str, edit| Change::Set {
        id: id.into(),
        edit,
    };
    let mut changes = vec![
        set(save.id, FieldEdit::Name(save.name.into())),
        set(
            save.id,
            FieldEdit::Appearance(Appearance {
                text: save.text.into(),
                image: save.image.map(str::to_owned),
                text_position,
            }),
        ),
        set(
            save.id,
            FieldEdit::Mode(if save.mode == "parallel" {
                Mode::Parallel
            } else {
                Mode::Sequential
            }),
        ),
        set(
            save.id,
            FieldEdit::Gesture(match save.gesture {
                "hold" => Gesture::Hold,
                "long_press" => Gesture::LongPress,
                _ => Gesture::Press,
            }),
        ),
    ];
    changes.extend(
        save.launchers
            .iter()
            .map(|(action, entry)| set(action, FieldEdit::DesktopEntry(entry.clone()))),
    );
    changes.extend(
        save.waits
            .iter()
            .map(|(action, milliseconds)| set(action, FieldEdit::Milliseconds(*milliseconds))),
    );
    changes.push(Change::Order {
        id: save.id.into(),
        indices: save.order.to_vec(),
    });
    Request::Edit { changes }
}
/// Adds an action of a type the runtime knows to `button`, after the ones it has.
pub fn add_action(button: &str, kind: &str, parameters: Value) -> Request {
    Request::AddAction {
        button_id: button.into(),
        type_key: kind.into(),
        parameters: serde_json::from_value(parameters).expect("action draft matches its schema"),
    }
}
#[derive(Clone, Copy)]
pub enum PermissionField {
    Consult,
    Modify,
    Execute,
}
pub fn permission_change(value: &Value, field: PermissionField, enabled: bool) -> Value {
    let mut c = value["own"]["consult"].as_bool().unwrap_or(true);
    let mut m = value["own"]["modify"].as_bool().unwrap_or(true);
    let mut e = value["own"]["execute"].as_bool().unwrap_or(false);
    match field {
        PermissionField::Consult => {
            c = enabled;
            if !enabled {
                m = false;
                e = false;
            }
        }
        PermissionField::Modify => {
            m = enabled;
            if enabled {
                c = true;
            }
        }
        PermissionField::Execute => {
            e = enabled;
            if enabled {
                c = true;
            }
        }
    }
    json!({"own":{"consult":c,"modify":m,"execute":e}})
}
pub fn folder_destination(actions: &[Value]) -> Option<&str> {
    actions
        .iter()
        .find(|a| a["fields"]["type"] == "enter_folder")
        .and_then(|a| a["fields"]["folder_id"].as_str())
}
pub fn delete_selection(button: &str, actions: &[Value]) -> Request {
    Request::Delete {
        id: folder_destination(actions).unwrap_or(button).into(),
        confirm_contents: true,
    }
}

/// Build a folder move only from a selected folder entrance.
pub fn move_folder(actions: &[Value], parent: &str, position: usize) -> Option<Request> {
    let folder = folder_destination(actions)?;
    Some(Request::MoveFolder {
        id: folder.into(),
        parent_id: parent.into(),
        position,
    })
}
