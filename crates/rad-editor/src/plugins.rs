//! Declarative drafts over contract A. Unchanged and incompatible bytes are never published.
use crate::options::fixed;
use rad_plugin_contract::{Definition, Document, FieldKind, Parameters};
use serde_json::{Value, json};
#[derive(Clone)]
pub struct FormDraft {
    pub definition: Definition,
    pub original: Vec<u8>,
    pub fields: Vec<Option<String>>,
    pub dirty: bool,
    fresh: bool,
    pub(crate) options: crate::options::OptionLists,
}
impl FormDraft {
    pub fn load(definition: Definition, original: Vec<u8>) -> Self {
        let params = serde_json::from_slice::<Parameters>(&original).ok();
        let fields: Vec<Option<String>> = definition
            .schema
            .fields
            .iter()
            .map(|f| {
                params.as_ref().and_then(|p| p.values.get(&f.id)).map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                })
            })
            .collect();
        let options = crate::options::OptionLists::new(&definition, fields.clone());
        Self {
            definition,
            original,
            fields,
            dirty: false,
            fresh: false,
            options,
        }
    }
    pub fn new(definition: Definition) -> Self {
        let original =
            serde_json::to_vec(&json!({"schema_version":definition.schema.version,"values":{}}))
                .expect("scalar JSON");
        let mut form = Self::load(definition, original);
        for (field, value) in form.definition.schema.fields.iter().zip(&mut form.fields) {
            if field.required {
                *value = Some(if matches!(field.kind, FieldKind::Boolean) {
                    "false".into()
                } else {
                    String::new()
                });
            }
        }
        form.fresh = true;
        form.dirty = true;
        form
    }
    pub fn compatible(&self) -> bool {
        self.fresh
            || Document::new(&self.original)
                .and_then(|d| d.parameters(&self.definition))
                .is_ok()
    }
    pub fn set(&mut self, index: usize, value: Option<String>) {
        if let Some(field) = self.fields.get_mut(index) {
            if self.definition.schema.fields[index].required && value.is_none() {
                return;
            }
            let changed = fixed(field) != fixed(&value);
            *field = value;
            self.dirty = true;
            if changed {
                self.options.invalidate_dependents(&self.definition, index);
            }
        }
    }
    pub fn publish(&self) -> Result<Option<Value>, &'static str> {
        if !self.dirty {
            return Ok(None);
        }
        if !self.compatible() {
            return Err("incompatible_plugin");
        }
        let mut params: Parameters =
            serde_json::from_slice(&self.original).map_err(|_| "incompatible_plugin")?;
        for (field, input) in self.definition.schema.fields.iter().zip(&self.fields) {
            match input {
                None => {
                    params.values.remove(&field.id);
                }
                Some(text) => {
                    let value = match &field.kind {
                        FieldKind::Boolean => {
                            Value::Bool(text.parse::<bool>().map_err(|_| "invalid_parameters")?)
                        }
                        FieldKind::Integer { .. } => {
                            json!(text.parse::<i64>().map_err(|_| "invalid_parameters")?)
                        }
                        FieldKind::Text { .. }
                        | FieldKind::Choice { .. }
                        | FieldKind::DynamicChoice { .. } => Value::String(text.clone()),
                    };
                    params.values.insert(field.id.clone(), value);
                }
            }
        }
        params
            .validate(&self.definition)
            .map_err(|_| "invalid_parameters")?;
        serde_json::to_value(params)
            .map(Some)
            .map_err(|_| "invalid_parameters")
    }
}

use crate::{
    Request,
    session::{Error, Port, Session},
};
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub type_key: String,
    pub label: String,
    pub visual: bool,
    pub ready: bool,
}
pub struct LoadedForm {
    pub type_key: String,
    pub id: Option<String>,
    pub form: Option<FormDraft>,
    pub original: Vec<u8>,
}
pub struct Visual {
    pub id: String,
    pub revision: String,
    pub availability: String,
    pub text: Option<String>,
    pub text_position: String,
    pub color: Option<[u8; 4]>,
    pub pixels: Option<crate::preview::Pixels>,
    pub has_image: bool,
    pub fault: bool,
}
fn bad() -> Error {
    Error::protocol()
}
fn decode_hex(s: &str, cap: usize) -> Result<Vec<u8>, Error> {
    if s.len() > cap * 2 || !s.len().is_multiple_of(2) {
        return Err(bad());
    }
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| {
            let a = (p[0] as char).to_digit(16).ok_or_else(bad)?;
            let b = (p[1] as char).to_digit(16).ok_or_else(bad)?;
            Ok((a * 16 + b) as u8)
        })
        .collect()
}
impl<P: Port> Session<P> {
    pub fn plugin_catalog(&mut self) -> Result<Vec<CatalogEntry>, Error> {
        let mut entries = vec![];
        let mut cursor = Value::Null;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let data = self
                .request(Request::new("plugin_catalog", json!({"cursor":cursor})))?
                .data;
            for item in data["items"]
                .as_array()
                .filter(|a| a.len() <= 32)
                .ok_or_else(bad)?
            {
                let key = item["type"]
                    .as_str()
                    .filter(|s| s.len() <= 193)
                    .ok_or_else(bad)?;
                let visual = match item["kind"].as_str() {
                    Some("action") => false,
                    Some("visual") => true,
                    _ => return Err(bad()),
                };
                entries.push(CatalogEntry {
                    type_key: key.into(),
                    label: item["label"]
                        .as_str()
                        .filter(|s| s.len() <= 256)
                        .ok_or_else(bad)?
                        .into(),
                    visual,
                    ready: item["ready"].as_bool().ok_or_else(bad)?,
                });
            }
            if entries.len() > 1024 {
                return Err(bad());
            }
            cursor = data["next_cursor"].clone();
            if cursor.is_null() {
                break;
            }
            if !seen.insert(cursor.as_str().ok_or_else(bad)?.to_owned()) || seen.len() > 64 {
                return Err(bad());
            }
        }
        Ok(entries)
    }
    pub fn plugin_form(&mut self, key: &str, id: Option<&str>) -> Result<LoadedForm, Error> {
        let mut key = key.to_owned();
        let mut original = vec![];
        if let Some(id) = id {
            let mut cursor = Value::Null;
            let mut seen = std::collections::BTreeSet::new();
            loop {
                let d = self
                    .request(Request::new(
                        "plugin_configuration",
                        json!({"id":id,"cursor":cursor}),
                    ))?
                    .data;
                if key.is_empty() {
                    key = d["type"].as_str().ok_or_else(bad)?.to_owned();
                }
                if d["type"] != key || d["encoding"] != "hex" {
                    return Err(bad());
                }
                for chunk in d["items"]
                    .as_array()
                    .filter(|v| v.len() <= 32)
                    .ok_or_else(bad)?
                {
                    original.extend(decode_hex(chunk.as_str().ok_or_else(bad)?, 2048)?);
                }
                if original.len() > 16384 {
                    return Err(bad());
                }
                cursor = d["next_cursor"].clone();
                if cursor.is_null() {
                    break;
                }
                if !seen.insert(cursor.as_str().ok_or_else(bad)?.to_owned()) || seen.len() > 16 {
                    return Err(bad());
                }
            }
        }
        let mut cursor = Value::Null;
        let mut seen = std::collections::BTreeSet::new();
        let mut fields = vec![];
        let mut version = None;
        let mut slots = None;
        loop {
            let d = match self.request(Request::new("schema", json!({"type":key,"cursor":cursor})))
            {
                Ok(r) => r.data,
                Err(e)
                    if id.is_some()
                        && matches!(
                            e.code.as_str(),
                            "not_found" | "executor_unavailable" | "incompatible_plugin"
                        ) =>
                {
                    return Ok(LoadedForm {
                        type_key: key,
                        id: id.map(str::to_owned),
                        form: None,
                        original,
                    });
                }
                Err(e) => return Err(e),
            };
            if d["type"] != key {
                return Err(bad());
            }
            let v = d["schema_version"]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(bad)?;
            let s: Vec<String> =
                serde_json::from_value(d["credential_slots"].clone()).map_err(|_| bad())?;
            if version.is_some_and(|old| old != v) || slots.as_ref().is_some_and(|old| old != &s) {
                return Err(bad());
            }
            version = Some(v);
            slots = Some(s);
            for f in d["items"]
                .as_array()
                .filter(|v| v.len() <= 32)
                .ok_or_else(bad)?
            {
                fields.push(serde_json::from_value(f.clone()).map_err(|_| bad())?);
            }
            if fields.len() > 32 {
                return Err(bad());
            }
            cursor = d["next_cursor"].clone();
            if cursor.is_null() {
                break;
            }
            if !seen.insert(cursor.as_str().ok_or_else(bad)?.to_owned()) || seen.len() > 32 {
                return Err(bad());
            }
        }
        let name = key.rsplit([':', '#']).next().ok_or_else(bad)?;
        let definition = Definition {
            response_timeout: None,
            id: name.into(),
            label: name.into(),
            schema: rad_plugin_contract::Schema {
                version: version.ok_or_else(bad)?,
                fields,
            },
            credential_slots: slots.unwrap_or_default(),
        };
        definition.validate().map_err(|_| bad())?;
        let form = if id.is_some() {
            FormDraft::load(definition, original.clone())
        } else {
            FormDraft::new(definition)
        };
        Ok(LoadedForm {
            type_key: key,
            id: id.map(str::to_owned),
            form: Some(form),
            original,
        })
    }
    pub fn plugin_visuals(&mut self, ids: &[String]) -> Result<Vec<Visual>, Error> {
        self.plugin_visuals_since(ids, &std::collections::BTreeMap::new())
    }
    pub fn plugin_visuals_since(
        &mut self,
        ids: &[String],
        revisions: &std::collections::BTreeMap<String, String>,
    ) -> Result<Vec<Visual>, Error> {
        if ids.len() > 32 {
            return Err(bad());
        }
        let mut out = vec![];
        for id in ids {
            let d = self
                .request(Request::new("plugin_visual", json!({"id":id})))?
                .data;
            let availability = d["availability"]
                .as_str()
                .filter(|s| matches!(*s, "current" | "stale" | "unavailable"))
                .ok_or_else(bad)?
                .to_owned();
            let revision = d["revision"]
                .as_str()
                .filter(|s| s.len() <= 20 && s.parse::<u64>().is_ok())
                .ok_or_else(bad)?
                .to_owned();
            let text = if d["text"].is_null() {
                None
            } else {
                Some(
                    d["text"]
                        .as_str()
                        .filter(|s| s.len() <= 4096)
                        .ok_or_else(bad)?
                        .to_owned(),
                )
            };
            let color = serde_json::from_value(d["color"].clone()).map_err(|_| bad())?;
            let mut pixels = None;
            let has_image = d["image"].as_bool().ok_or_else(bad)?;
            if has_image && availability != "unavailable" && revisions.get(id) != Some(&revision) {
                let mut bytes = vec![];
                loop {
                    let chunk = self
                        .request(Request::new(
                            "plugin_visual_read",
                            json!({"id":id,"revision":revision,"offset":bytes.len().to_string()}),
                        ))?
                        .data;
                    let part = decode_hex(chunk["hex"].as_str().ok_or_else(bad)?, 4096)?;
                    if part.is_empty() || bytes.len() + part.len() > 256 * 1024 {
                        return Err(bad());
                    }
                    bytes.extend(part);
                    if chunk["next_offset"].is_null() {
                        break;
                    }
                    if chunk["next_offset"]
                        .as_str()
                        .and_then(|s| s.parse::<usize>().ok())
                        != Some(bytes.len())
                        || bytes.is_empty()
                    {
                        return Err(bad());
                    }
                }
                pixels = Some(crate::preview::decode(&bytes)?);
            }
            out.push(Visual {
                id: id.clone(),
                revision,
                availability,
                text_position: match d["text_position"].as_str().unwrap_or("middle") {
                    v @ ("top" | "middle" | "bottom" | "above_image" | "below_image") => v.into(),
                    _ => return Err(bad()),
                },
                text,
                color,
                pixels,
                has_image,
                fault: false,
            });
        }
        Ok(out)
    }
}
#[derive(Debug, Clone)]
pub enum FormTarget {
    AddAction(String),
    Action(String),
    Visual(String),
}
impl FormTarget {
    pub fn request(&self, key: &str, value: Value) -> Request {
        match self {
            Self::AddAction(button) => crate::commands::add_action(button, key, value),
            Self::Action(id) => Request::new(
                "edit",
                json!({"changes":[{"kind":"set","id":id,"field":"parameters","value":value}]}),
            ),
            Self::Visual(id) => Request::new(
                "edit",
                json!({"changes":[{"kind":"set","id":id,"field":"visual","value":{"type":key,"parameters":value}}]}),
            ),
        }
    }
}
impl<P: Port> Session<P> {
    pub fn plugin_visuals_isolated(
        &mut self,
        ids: &[String],
        revisions: &std::collections::BTreeMap<String, String>,
    ) -> Result<Vec<Visual>, Error> {
        if ids.len() > 32 {
            return Err(bad());
        }
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            match self.plugin_visuals_since(std::slice::from_ref(id), revisions) {
                Ok(v) => out.extend(v),
                Err(e) if !self.is_open() => return Err(e),
                Err(_) => out.push(Visual {
                    id: id.clone(),
                    revision: String::new(),
                    availability: "unavailable".into(),
                    text: None,
                    text_position: "middle".into(),
                    color: None,
                    pixels: None,
                    has_image: false,
                    fault: true,
                }),
            }
        }
        Ok(out)
    }
}
/// UI affordance guard; Authority still validates the actual add transaction.
pub fn accepts_action(actions: &[Value]) -> bool {
    actions.len() < 20
        && actions.iter().all(|a| {
            !matches!(
                a["fields"]["type"].as_str(),
                Some("return" | "enter_folder")
            )
        })
}
