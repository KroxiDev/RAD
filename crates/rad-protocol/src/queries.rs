use super::*;
use rad_authority::{ActionType, ActionView, Cursor, EntityView, QueryError, Visible};
use rad_core::{configuration::Mode, permissions::LocalPermissions};
pub(super) fn read(
    a: &Authority,
    c: IncomingContext,
    request: &rad_api::Request,
) -> Checked<Value> {
    let rev = a
        .view_revision(c.origin)
        .ok_or("resource_exhausted")?
        .value()
        .to_string();
    use rad_api::Request as R;
    match request {
        R::Get { id } => {
            let id = parse_id(id)?;
            if id.kind() == EntityKind::Folder {
                let f = a.folder(c.origin, id).ok_or("not_found")?;
                return Ok(
                    json!({"visibility":"visible","id":short(id),"kind":"folder","fields":{"name":f.name,"entry_button_id":f.entry_button.map(|b|short(b.short())),"parent_id":f.parent_binding.map(|b|short(b.short())),"slots":f.slots.into_iter().map(|slot|match slot {None=>Value::Null,Some(Visible::Restricted)=>json!({"visibility":"restricted"}),Some(Visible::Visible(b))=>json!({"visibility":"visible","id":short(b.short())})}).collect::<Vec<_>>()},"view_revision":rev,"sections":[]}),
                );
            }
            Ok(entity(&a.get(c.origin, id).ok_or("not_found")?, &rev))
        }
        R::List {
            cursor: cursor_value,
            limit,
        } => {
            if *limit != 32 {
                return Err("invalid_request");
            };
            let cursor = cursor(cursor_value.as_deref())?;
            // Reserve envelope/cursor overhead once, not a second unbounded page buffer.
            let mut first = true;
            let page = a
                .list(c.connection, c.origin, cursor.as_ref(), |e| {
                    let n = serde_json::to_vec(&entity(e, &rev)).unwrap().len() + 1;
                    let overhead = if first {
                        first = false;
                        1024
                    } else {
                        0
                    };
                    n + overhead
                })
                .map_err(query_error)?;
            Ok(
                json!({"items":page.entries.iter().map(|e|entity(e,&rev)).collect::<Vec<_>>(),"next_cursor":page.next.as_ref().map(encode_cursor),"view_revision":rev}),
            )
        }
        R::GetSection {
            id,
            section,
            cursor: cursor_value,
        } => {
            let id = parse_id(id)?;
            let cursor = cursor(cursor_value.as_deref())?;
            match section {
                rad_api::Section::Appearance => {
                    if cursor.is_some() {
                        return Err("cursor_expired");
                    };
                    let v = super::views::appearance(a, c, id)?;
                    let mut items = vec![
                        json!({"name":"text","value":v.text}),
                        json!({"name":"image","value":v.image}),
                        json!({"name":"text_position","value":v.text_position}),
                    ];
                    if let Some(icon) = v.native_icon {
                        items.push(json!({"name":"native_icon","value":icon}));
                    }
                    Ok(
                        json!({"visual_type":v.visual_type,"section":"appearance","items":items,"next_cursor":null,"view_revision":rev}),
                    )
                }
                rad_api::Section::Actions => {
                    let mut first = true;
                    let p = a
                        .actions(c.connection, c.origin, id, cursor.as_ref(), |e| {
                            let n = serde_json::to_vec(&visible_action(e, &rev)).unwrap().len() + 1;
                            let overhead = if first {
                                first = false;
                                1024
                            } else {
                                0
                            };
                            n + overhead
                        })
                        .map_err(query_error)?;
                    Ok(
                        json!({"section":"actions","items":p.entries.iter().map(|e|visible_action(e,&rev)).collect::<Vec<_>>(),"next_cursor":p.next.as_ref().map(encode_cursor),"view_revision":rev}),
                    )
                }
            }
        }
        _ => Err("invalid_request"),
    }
}
fn query_error(e: QueryError) -> &'static str {
    match e {
        QueryError::NotFound => "not_found",
        QueryError::CursorExpired => "cursor_expired",
        QueryError::Capacity => "resource_exhausted",
    }
}
fn local(p: LocalPermissions) -> Value {
    match p.own_value() {
        None => json!({"inherit":true}),
        Some(p) => json!({"own":permissions(p)}),
    }
}
fn entity(e: &EntityView, rev: &str) -> Value {
    match e {
        EntityView::Button(b) => {
            json!({"visibility":"visible","id":short(b.binding.short()),"kind":"button","fields":{"name":b.name,"gesture":gesture(b.gesture),"mode":match b.mode {Mode::Sequential=>"sequential",Mode::Parallel=>"parallel"},"permissions":local(b.local_permissions),"effective_permissions":permissions(b.permissions),"folder_id":b.folder.map(|b|short(b.short())),"position":b.position},"view_revision":rev,"sections":["appearance","actions"]})
        }
        EntityView::Action(v) => action(v, rev),
    }
}
fn action(v: &ActionView, rev: &str) -> Value {
    let kind = match &v.kind {
        ActionType::Launcher => "launcher".to_owned(),
        ActionType::EnterFolder => "enter_folder".to_owned(),
        ActionType::Return => "return".to_owned(),
        ActionType::Sleep => "sleep".to_owned(),
        ActionType::Wait => "wait".to_owned(),
        ActionType::External(t) => format!("{}:{}", t.plugin(), t.name()),
    };
    json!({"visibility":"visible","id":short(v.binding.short()),"kind":"action","fields":{"type":kind,"permissions":local(v.local_permissions),"effective_permissions":permissions(v.permissions),"parent_id":v.parent_binding.map(|b|short(b.short())),"parameters":parameters(v),"folder_id":v.destination.map(|b|short(b.short()))},"view_revision":rev,"sections":[]})
}
/// The action's own configuration, for the kinds whose parameters are not opaque.
fn parameters(v: &ActionView) -> Value {
    match (&v.desktop_entry, v.wait_ms) {
        (Some(entry), _) => json!({"desktop_entry":entry.id()}),
        (None, Some(milliseconds)) => json!({"milliseconds":milliseconds}),
        (None, None) => Value::Null,
    }
}
fn visible_action(v: &Visible<ActionView>, rev: &str) -> Value {
    match v {
        Visible::Visible(v) => action(v, rev),
        Visible::Restricted => json!({"visibility":"restricted"}),
    }
}
fn encode_cursor(c: &Cursor) -> String {
    c.to_bytes().iter().map(|b| format!("{b:02x}")).collect()
}
fn cursor(value: Option<&str>) -> Checked<Option<Cursor>> {
    match value {
        None => Ok(None),
        Some(s)
            if s.len() == 102
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
        {
            let bytes = (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect::<Vec<_>>();
            Cursor::from_bytes(&bytes).map(Some).map_err(query_error)
        }
        _ => Err("invalid_request"),
    }
}

fn gesture(value: rad_core::configuration::Gesture) -> rad_api::Gesture {
    match value {
        rad_core::configuration::Gesture::Press => rad_api::Gesture::Press,
        rad_core::configuration::Gesture::Hold => rad_api::Gesture::Hold,
        rad_core::configuration::Gesture::LongPress => rad_api::Gesture::LongPress,
    }
}
