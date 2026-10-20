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
    pub order: &'a [usize],
    /// Wait actions of the key, by identifier, with their durations in milliseconds.
    pub waits: &'a [(String, u64)],
}
pub fn save_button(save: &KeySave) -> Request {
    let mut appearance = json!({"text":save.text,"image":save.image});
    if let Some(text_position) = save.text_position {
        appearance["text_position"] = json!(text_position);
    }
    let mut changes = vec![
        json!({"kind":"set","id":save.id,"field":"name","value":save.name}),
        json!({"kind":"set","id":save.id,"field":"appearance","value":appearance}),
        json!({"kind":"set","id":save.id,"field":"mode","value":save.mode}),
    ];
    changes.extend(save.waits.iter().map(|(action, milliseconds)| {
        json!({"kind":"set","id":action,"field":"milliseconds","value":milliseconds})
    }));
    changes.push(json!({"kind":"order","id":save.id,"indices":save.order}));
    Request::new("edit", json!({"changes":changes}))
}
/// Adds an action of a type the runtime knows to `button`, after the ones it has.
pub fn add_action(button: &str, kind: &str, parameters: Value) -> Request {
    Request::new(
        "add_action",
        json!({"button_id":button,"type":kind,"parameters":parameters}),
    )
}
pub fn call(op: &str, args: Value) -> Request {
    Request::new(op, args)
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
    Request::new(
        "delete",
        json!({"id":folder_destination(actions).unwrap_or(button),"confirm_contents":true}),
    )
}

/// Build a folder move only from a selected folder entrance.
pub fn move_folder(actions: &[Value], parent: &str, position: usize) -> Option<Request> {
    let folder = folder_destination(actions)?;
    Some(Request::new(
        "move_folder",
        json!({"id":folder,"parent_id":parent,"position":position}),
    ))
}
