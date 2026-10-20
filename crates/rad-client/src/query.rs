//! Compound `get` stays bounded and discards all partial data on any error.
use crate::{
    connection::Failure,
    output,
    response::{Response, Status},
};
#[cfg(test)]
use serde_json::{Value, json};
const MAX_OUTPUT: usize = 16 * 1024 * 1024;
pub(crate) fn assemble(
    mut response: Response,
    id: &str,
    mut request: impl FnMut(rad_api::Request) -> Result<Response, Failure>,
) -> Result<Response, Failure> {
    let mut remaining = MAX_OUTPUT;
    for attempt in 0..2 {
        match pages(&mut response, id, &mut remaining, &mut request)? {
            None => return Ok(response),
            Some(error)
                if attempt == 0
                    && error
                        .error
                        .as_ref()
                        .is_some_and(|e| e.code == "cursor_expired") =>
            {
                response = request(rad_api::Request::Get { id: id.to_owned() })?;
                if response.status == Status::Error {
                    return Ok(response);
                }
            }
            Some(error) => return Ok(error),
        }
    }
    unreachable!()
}
fn pages(
    response: &mut Response,
    id: &str,
    remaining: &mut usize,
    request: &mut impl FnMut(rad_api::Request) -> Result<Response, Failure>,
) -> Result<Option<Response>, Failure> {
    if !charge(response, remaining) {
        return Ok(Some(local(response, "resource_exhausted")));
    }
    let rad_api::ResponseData::Entity(entity) = &response.data else {
        return Ok(None);
    };
    let entity = entity.clone();
    let revision = &entity.view_revision;
    if revision.is_empty()
        || (revision != "0" && revision.starts_with('0'))
        || revision.parse::<u64>().is_err()
    {
        return Err(invalid(response));
    }
    if entity.sections.len() > 2
        || entity
            .sections
            .iter()
            .enumerate()
            .any(|(i, s)| entity.sections[..i].contains(s))
    {
        return Err(invalid(response));
    }
    let mut gathered = rad_api::SectionData {
        appearance: None,
        actions: None,
    };
    for section in &entity.sections {
        let mut cursor: Option<String> = None;
        loop {
            let page = request(rad_api::Request::GetSection {
                id: id.to_owned(),
                section: *section,
                cursor: cursor.clone(),
            })?;
            if page.status == Status::Error {
                return Ok(Some(page));
            }
            if !charge(&page, remaining) {
                return Ok(Some(local(&page, "resource_exhausted")));
            }
            let (page_revision, next, len) = match (&page.data, section) {
                (rad_api::ResponseData::Appearance(p), rad_api::Section::Appearance) => {
                    (&p.view_revision, &p.next_cursor, p.items.len())
                }
                (rad_api::ResponseData::Actions(p), rad_api::Section::Actions) => {
                    (&p.view_revision, &p.next_cursor, p.items.len())
                }
                _ => return Err(invalid(&page)),
            };
            if page_revision != revision {
                return Ok(Some(local(&page, "cursor_expired")));
            }
            if len > 32 {
                return Err(invalid(&page));
            }
            match &page.data {
                rad_api::ResponseData::Appearance(p) => gathered
                    .appearance
                    .get_or_insert_with(Vec::new)
                    .extend(p.items.clone()),
                rad_api::ResponseData::Actions(p) => gathered
                    .actions
                    .get_or_insert_with(Vec::new)
                    .extend(p.items.clone()),
                _ => unreachable!(),
            }
            let Some(next) = next else {
                break;
            };
            if next.is_empty() || next.len() > 128 || cursor.as_ref() == Some(next) {
                return Err(invalid(&page));
            }
            cursor = Some(next.clone());
        }
    }
    response.data = rad_api::ResponseData::CompoundEntity(rad_api::CompoundEntity {
        entity,
        section_data: gathered,
    });
    if serde_json::to_vec(response)
        .map_err(|_| invalid(response))?
        .len()
        > MAX_OUTPUT
    {
        return Ok(Some(local(response, "resource_exhausted")));
    }
    Ok(None)
}
fn charge(response: &Response, remaining: &mut usize) -> bool {
    let Ok(bytes) = serde_json::to_vec(response) else {
        return false;
    };
    if bytes.len() > *remaining {
        return false;
    }
    *remaining -= bytes.len();
    true
}
fn invalid(response: &Response) -> Failure {
    Failure {
        code: "invalid_request",
        id: response.id.as_deref().and_then(|s| s.parse().ok()),
        session_id: response.session_id.clone(),
        outcome: None,
    }
}
fn local(response: &Response, code: &'static str) -> Response {
    let mut f = invalid(response);
    f.code = code;
    output::failure(f)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn response(mut data: Value) -> Response {
        if data.get("sections").is_some() {
            data["visibility"] = json!("visible");
            data["id"] = json!("b1");
            data["kind"] = json!("button");
            let name = data["fields"]["name"].as_str().unwrap_or("").to_owned();
            data["fields"] = json!({"name":name,"gesture":"press","mode":"sequential","permissions":{"inherit":true},"effective_permissions":{"consult":true,"modify":true,"execute":true},"folder_id":"f1","position":0});
        }
        if data["section"] == "actions"
            && let Some(items) = data["items"].as_array_mut()
        {
            for item in items {
                if let Some(value) = item.as_str() {
                    *item = json!({"visibility":"visible","id":"a1","kind":"action","fields":{"type":"launcher","permissions":{"inherit":true},"effective_permissions":{"consult":true,"modify":true,"execute":true},"parent_id":"b1","parameters":{"desktop_entry":value},"folder_id":null},"view_revision":"1","sections":[]});
                }
            }
        }
        Response {
            v: 1,
            id: Some("2".into()),
            session_id: Some("00000000000000000000000000000001".into()),
            status: Status::Ok,
            data: rad_api::ResponseData::decode(
                if data.as_object().is_some_and(|v| v.is_empty()) {
                    rad_api::Operation::Subscribe
                } else if data.get("section").is_some() {
                    rad_api::Operation::GetSection
                } else {
                    rad_api::Operation::Get
                },
                Status::Ok,
                data,
            )
            .unwrap(),
            error: None,
        }
    }
    #[test]
    fn revision_change_between_get_and_first_section_discards_the_old_base() {
        let first = response(
            json!({"fields":{"name":"previously authorized"},"sections":["actions"],"view_revision":"1"}),
        );
        let mut calls = 0;
        let result=assemble(first,"b1",|request|{let op=request.operation().as_str();calls+=1;Ok(match calls{
   1=>{assert_eq!(op,"get_section");response(json!({"section":"actions","items":[],"next_cursor":null,"view_revision":"2"}))},
   2=>{assert_eq!(op,"get");response(json!({"fields":{"name":"current"},"sections":["actions"],"view_revision":"2"}))},
   3=>response(json!({"section":"actions","items":[],"next_cursor":null,"view_revision":"2"})),
   _=>panic!("unexpected replay")})}).unwrap();
        assert_eq!(calls, 3);
        assert_eq!(
            serde_json::to_value(&result.data).unwrap()["fields"]["name"],
            "current"
        );
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("previously authorized")
        );
    }
    #[test]
    fn output_budget_rejects_without_publishing_any_partial_sections() {
        let base = response(json!({"sections":["actions"],"view_revision":"1"}));
        let mut count = 0;
        let result=assemble(base,"b1",|_|{
            count+=1;
            Ok(response(json!({"section":"actions","view_revision":"1","items":vec!["x".repeat(1000);32],"next_cursor":count.to_string()})))
        }).unwrap();
        assert_eq!(result.error.unwrap().code, "resource_exhausted");
        assert!(matches!(result.data, rad_api::ResponseData::None));
        assert!((300..600).contains(&count));
    }
    #[test]
    fn budget_exact_size_accepts_and_one_byte_less_rejects() {
        let value = response(json!({}));
        let size = serde_json::to_vec(&value).unwrap().len();
        let mut exact = size;
        assert!(charge(&value, &mut exact));
        assert_eq!(exact, 0);
        let mut short = size - 1;
        assert!(!charge(&value, &mut short));
        assert_eq!(short, size - 1);
    }
    #[test]
    fn repeated_cursor_is_protocol_error_and_repeated_expiry_is_not_an_infinite_retry() {
        let base = response(json!({"sections":["actions"],"view_revision":"1"}));
        let mut calls = 0;
        let result = assemble(base.clone(), "b1", |_| {
            calls += 1;
            Ok(response(
                json!({"section":"actions","view_revision":"1","items":[],"next_cursor":"same"}),
            ))
        });
        assert_eq!(result.unwrap_err().code, "invalid_request");
        assert_eq!(calls, 2);
        calls = 0;
        let error = assemble(base.clone(), "b1", |request| {
            let op = request.operation().as_str();
            calls += 1;
            Ok(if op == "get" {
                base.clone()
            } else {
                local(&base, "cursor_expired")
            })
        })
        .unwrap();
        assert_eq!(calls, 3);
        assert_eq!(error.error.unwrap().code, "cursor_expired");
        assert!(matches!(error.data, rad_api::ResponseData::None));
    }
}
