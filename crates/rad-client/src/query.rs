//! Compound `get` stays bounded and discards all partial data on any error.
use crate::{
    connection::Failure,
    output,
    response::{Response, Status},
};
use serde_json::{Map, Value, json};
const MAX_OUTPUT: usize = 16 * 1024 * 1024;
pub(crate) fn assemble(
    mut response: Response,
    id: &str,
    mut request: impl FnMut(&str, Value) -> Result<Response, Failure>,
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
                response = request("get", json!({"id":id}))?;
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
    request: &mut impl FnMut(&str, Value) -> Result<Response, Failure>,
) -> Result<Option<Response>, Failure> {
    if !charge(response, remaining) {
        return Ok(Some(local(response, "resource_exhausted")));
    }
    let Some(sections) = response.data.get("sections") else {
        return Ok(None);
    };
    let sections = sections
        .as_array()
        .ok_or_else(|| invalid(response))?
        .clone();
    if sections.len() > 2 {
        return Err(invalid(response));
    }
    let revision = response.data["view_revision"]
        .as_str()
        .filter(|v| {
            !v.is_empty()
                && (*v == "0" || !v.starts_with('0'))
                && v.bytes().all(|b| b.is_ascii_digit())
                && v.parse::<u64>().is_ok()
        })
        .ok_or_else(|| invalid(response))?
        .to_owned();
    let mut gathered = Map::new();
    for section in sections {
        let section = section
            .as_str()
            .filter(|s| matches!(*s, "appearance" | "actions"))
            .ok_or_else(|| invalid(response))?;
        if gathered.contains_key(section) {
            return Err(invalid(response));
        }
        let mut items = Vec::new();
        let mut cursor = Value::Null;
        loop {
            let page = request(
                "get_section",
                json!({"id":id,"section":section,"cursor":cursor}),
            )?;
            if page.status == Status::Error {
                return Ok(Some(page));
            }
            if !charge(&page, remaining) {
                return Ok(Some(local(&page, "resource_exhausted")));
            }
            if page.data["view_revision"].as_str() != Some(revision.as_str()) {
                return Ok(Some(local(&page, "cursor_expired")));
            }
            if page.data["section"] != section {
                return Err(invalid(&page));
            }
            let entries = page.data["items"]
                .as_array()
                .filter(|a| a.len() <= 32)
                .ok_or_else(|| invalid(&page))?;
            items.extend(entries.iter().cloned());
            let next = page.data.get("next_cursor").ok_or_else(|| invalid(&page))?;
            if next.is_null() {
                break;
            }
            if !next
                .as_str()
                .is_some_and(|c| !c.is_empty() && c.len() <= 128)
                || next == &cursor
            {
                return Err(invalid(&page));
            }
            cursor = next.clone();
        }
        gathered.insert(section.into(), Value::Array(items));
    }
    response.data["section_data"] = Value::Object(gathered);
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
    fn response(data: Value) -> Response {
        Response {
            v: 1,
            id: Some("2".into()),
            session_id: Some("00000000000000000000000000000001".into()),
            status: Status::Ok,
            data,
            error: None,
        }
    }
    #[test]
    fn revision_change_between_get_and_first_section_discards_the_old_base() {
        let first = response(
            json!({"fields":{"name":"previously authorized"},"sections":["actions"],"view_revision":"1"}),
        );
        let mut calls = 0;
        let result=assemble(first,"b1",|op,_args|{calls+=1;Ok(match calls{
   1=>{assert_eq!(op,"get_section");response(json!({"section":"actions","items":[],"next_cursor":null,"view_revision":"2"}))},
   2=>{assert_eq!(op,"get");response(json!({"fields":{"name":"current"},"sections":["actions"],"view_revision":"2"}))},
   3=>response(json!({"section":"actions","items":[],"next_cursor":null,"view_revision":"2"})),
   _=>panic!("unexpected replay")})}).unwrap();
        assert_eq!(calls, 3);
        assert_eq!(result.data["fields"]["name"], "current");
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
        let result=assemble(base,"b1",|_,_|{
            count+=1;
            Ok(response(json!({"section":"actions","view_revision":"1","items":vec!["x".repeat(1000);32],"next_cursor":count.to_string()})))
        }).unwrap();
        assert_eq!(result.error.unwrap().code, "resource_exhausted");
        assert!(result.data.is_null());
        assert!((500..600).contains(&count));
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
        let result = assemble(base.clone(), "b1", |_, _| {
            calls += 1;
            Ok(response(
                json!({"section":"actions","view_revision":"1","items":[],"next_cursor":"same"}),
            ))
        });
        assert_eq!(result.unwrap_err().code, "invalid_request");
        assert_eq!(calls, 2);
        calls = 0;
        let error = assemble(base.clone(), "b1", |op, _| {
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
        assert!(error.data.is_null());
    }
}
