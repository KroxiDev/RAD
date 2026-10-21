//! Revision-bound, bounded projection of used plugins, never installation authority.
use crate::{Request, session::Error};
use serde_json::Value;
use std::collections::BTreeSet;

// The canonical configuration is bounded to 1 MiB. Every distinct reference
// consumes at least six bytes (`"a":""` in plugin_settings, excluding commas);
// action, visual and dependency-hint references cost more. Count the union once.
const DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_REFERENCES: usize = DOCUMENT_BYTES / 6;
// IDs/origins already fit in the document. Allow 160 additional bytes per item
// for field names, delimiters, nulls and even a full three-u32 version object.
// Only installed plugins add credential slots: <=32 plugins, <=32 ASCII slots
// per manifest, <=96 bytes per slot plus quotes/comma. No up-front allocation.
const MAX_ITEMS_BYTES: usize =
    DOCUMENT_BYTES + MAX_REFERENCES * 160 + rad_plugin_contract::MAX_PLUGINS * 32 * 99;

/// One page per worker turn. Partial lists are never offered for installation.
#[derive(Default)]
pub struct Dependencies {
    items: Vec<Value>,
    revision: Option<String>,
    cursor: Option<String>,
    cursors: BTreeSet<String>,
    ids: BTreeSet<String>,
    bytes: usize,
    complete: bool,
    failed: bool,
}
impl Dependencies {
    pub fn request(&self) -> Option<Request> {
        (!self.complete && !self.failed).then(|| Request::PluginDependencies {
            cursor: self.cursor.clone(),
        })
    }
    pub fn accept(&mut self, data: Value) -> Result<(), Error> {
        let result = self.accept_page(data);
        if let Err(error) = &result {
            self.fail(error);
        }
        result
    }
    fn accept_page(&mut self, data: Value) -> Result<(), Error> {
        if self.complete || self.failed {
            return Err(Error::protocol());
        }
        let revision = data["view_revision"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .ok_or_else(Error::protocol)?;
        if self.revision.as_ref().is_some_and(|old| old != revision) {
            return Err(Error {
                code: "cursor_expired".into(),
                cause: None,
                source_change: None,
                unknown: false,
            });
        }
        let bytes = serde_json::to_vec(&data)
            .map_err(|_| Error::protocol())?
            .len();
        let items = data["items"]
            .as_array()
            .filter(|items| items.len() <= 32)
            .ok_or_else(Error::protocol)?;
        // Budget retained items, not page envelopes: a valid peer may return
        // fewer than 32 items per page without inflating the retained payload.
        let item_bytes = serde_json::to_vec(items)
            .map_err(|_| Error::protocol())?
            .len();
        if bytes > 49152
            || self.bytes + item_bytes > MAX_ITEMS_BYTES
            || self.items.len() + items.len() > MAX_REFERENCES
        {
            return Err(Error::protocol());
        }
        let next = match data.get("next_cursor") {
            Some(Value::Null) => None,
            Some(Value::String(cursor))
                if !cursor.is_empty() && cursor.len() <= 1024 && !items.is_empty() =>
            {
                if !self.cursors.insert(cursor.clone()) {
                    return Err(Error::protocol());
                }
                Some(cursor.clone())
            }
            _ => return Err(Error::protocol()),
        };
        for item in items {
            let id = item["id"]
                .as_str()
                .filter(|id| portable_string(id))
                .ok_or_else(Error::protocol)?;
            if !self.ids.insert(id.into()) {
                return Err(Error::protocol());
            }
            if !item["version"].is_null()
                && !["major", "minor", "patch"].iter().all(|key| {
                    item["version"][key]
                        .as_u64()
                        .is_some_and(|n| n <= u32::MAX as u64)
                })
            {
                return Err(Error::protocol());
            }
            if !item["origin"].is_null()
                && item["origin"].as_str().is_none_or(|s| !portable_string(s))
            {
                return Err(Error::protocol());
            }
            let slots = item["credential_slots"]
                .as_array()
                .filter(|slots| slots.len() <= 32)
                .ok_or_else(Error::protocol)?;
            if slots
                .iter()
                .any(|slot| slot.as_str().is_none_or(|s| s.is_empty() || s.len() > 96))
                || !(item["needs_credentials"].is_null() || item["needs_credentials"].is_boolean())
            {
                return Err(Error::protocol());
            }
        }
        self.revision = Some(revision.into());
        self.bytes += item_bytes;
        self.items.extend(items.iter().cloned());
        self.cursor = next;
        self.complete = self.cursor.is_none();
        Ok(())
    }
    pub fn fail(&mut self, error: &Error) {
        self.reset();
        self.failed = error.code != "cursor_expired";
    }
    pub fn failed(&self) -> bool {
        self.failed
    }
    pub fn items(&self) -> Option<&[Value]> {
        self.complete.then_some(&self.items)
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

fn portable_string(value: &str) -> bool {
    !value.is_empty()
        && !value.chars().any(char::is_control)
        && serde_json::to_vec(value).is_ok_and(|json| json.len() <= 8192)
}
