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
    pub settings: bool,
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
            settings: false,
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
    pub gestures: Vec<rad_api::Gesture>,
}
/// Physical gestures supported by every action in a button's current plan.
pub fn plan_gestures(actions: &[Value], catalog: &[CatalogEntry]) -> Vec<rad_api::Gesture> {
    use rad_api::Gesture;
    [Gesture::Press, Gesture::Hold, Gesture::LongPress]
        .into_iter()
        .filter(|gesture| {
            if actions.is_empty() {
                return *gesture == Gesture::Press;
            }
            actions.iter().all(|action| {
                let kind = action["fields"]["type"].as_str().unwrap_or("");
                if kind.contains(':') {
                    catalog
                        .iter()
                        .find(|entry| entry.type_key == kind && !entry.visual)
                        .is_some_and(|entry| entry.gestures.contains(gesture))
                } else {
                    matches!(kind, "launcher" | "wait" | "return" | "enter_folder")
                        && *gesture != Gesture::Hold
                }
            })
        })
        .collect()
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
        let mut cursor = None;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let rad_api::ResponseData::Catalog(data) =
                self.request(crate::Request::PluginCatalog { cursor })?.data
            else {
                return Err(bad());
            };
            if data.items.len() > 32 {
                return Err(bad());
            }
            for item in data.items {
                if item.type_key.len() > 193 || item.label.len() > 256 {
                    return Err(bad());
                }
                entries.push(CatalogEntry {
                    type_key: item.type_key,
                    label: item.label,
                    visual: item.kind == rad_api::CatalogKind::Visual,
                    ready: item.ready,
                    gestures: item.gestures,
                });
            }
            if entries.len() > 1024 {
                return Err(bad());
            }
            cursor = data.next_cursor;
            let Some(next) = &cursor else {
                break;
            };
            if !seen.insert(next.clone()) || seen.len() > 64 {
                return Err(bad());
            }
        }
        Ok(entries)
    }
    fn plugin_settings_form(&mut self, plugin_id: &str) -> Result<LoadedForm, Error> {
        let rad_api::ResponseData::PluginSettings(settings) = self
            .request(Request::PluginSettings {
                plugin_id: plugin_id.into(),
            })?
            .data
        else {
            return Err(bad());
        };
        if settings.plugin_id != plugin_id {
            return Err(bad());
        }
        let original = settings
            .hex
            .as_deref()
            .map(|hex| decode_hex(hex, 16384))
            .transpose()?
            .unwrap_or_default();
        let manifest = match self.request(Request::PluginManifest {
            plugin_id: plugin_id.into(),
        }) {
            Ok(response) => {
                let rad_api::ResponseData::PluginManifest(view) = response.data else {
                    return Err(bad());
                };
                if view.plugin_id != plugin_id {
                    return Err(bad());
                }
                serde_json::from_value::<rad_plugin_contract::Manifest>(
                    serde_json::to_value(view.document).map_err(|_| bad())?,
                )
                .ok()
                .filter(|manifest| manifest.id == plugin_id)
            }
            Err(error)
                if error.code == "not_found"
                    || error.code == "plugin_missing"
                    || error.code == "incompatible_plugin" =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        let form = manifest
            .and_then(|manifest| manifest.settings)
            .map(|schema| {
                let definition = Definition {
                    id: "settings".into(),
                    label: "Settings".into(),
                    schema,
                    gestures: rad_plugin_contract::default_gestures(),
                    credential_slots: vec![],
                    response_timeout: None,
                };
                let mut form = if settings.hex.is_some() {
                    FormDraft::load(definition, original.clone())
                } else {
                    FormDraft::new(definition)
                };
                form.settings = true;
                form
            });
        Ok(LoadedForm {
            type_key: plugin_id.into(),
            id: None,
            form,
            original,
        })
    }
    pub fn plugin_form(&mut self, key: &str, id: Option<&str>) -> Result<LoadedForm, Error> {
        if id.is_none() && !key.contains([':', '#']) {
            return self.plugin_settings_form(key);
        }
        let mut key = key.to_owned();
        let mut original = vec![];
        if let Some(id) = id {
            let mut cursor = None;
            let mut seen = std::collections::BTreeSet::new();
            loop {
                let rad_api::ResponseData::Configuration(d) = self
                    .request(crate::Request::PluginConfiguration {
                        id: id.into(),
                        cursor,
                    })?
                    .data
                else {
                    return Err(bad());
                };
                if key.is_empty() {
                    key = d.type_key.clone();
                }
                if d.type_key != key || d.items.len() > 32 {
                    return Err(bad());
                }
                for chunk in &d.items {
                    original.extend(decode_hex(chunk, 2048)?);
                }
                if original.len() > 16384 {
                    return Err(bad());
                }
                cursor = d.next_cursor;
                if cursor.is_none() {
                    break;
                }
                if !seen.insert(cursor.as_ref().ok_or_else(bad)?.clone()) || seen.len() > 16 {
                    return Err(bad());
                }
            }
        }
        let mut cursor = None;
        let mut seen = std::collections::BTreeSet::new();
        let mut fields = vec![];
        let mut version = None;
        let mut slots = None;
        loop {
            let d = match self.request(crate::Request::Schema {
                type_key: key.clone(),
                cursor,
            }) {
                Ok(r) => match r.data {
                    rad_api::ResponseData::Schema(data) => data,
                    _ => return Err(bad()),
                },
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
            if d.type_key != key || d.items.len() > 32 {
                return Err(bad());
            }
            let v = d.schema_version.ok_or_else(bad)?;
            let s = d.credential_slots.ok_or_else(bad)?;
            if version.is_some_and(|old| old != v) || slots.as_ref().is_some_and(|old| old != &s) {
                return Err(bad());
            }
            version = Some(v);
            slots = Some(s);
            for f in d.items {
                fields.push(
                    serde_json::from_value(serde_json::to_value(f).map_err(|_| bad())?)
                        .map_err(|_| bad())?,
                );
            }
            if fields.len() > 32 {
                return Err(bad());
            }
            cursor = d.next_cursor;
            if cursor.is_none() {
                break;
            }
            if !seen.insert(cursor.as_ref().ok_or_else(bad)?.clone()) || seen.len() > 32 {
                return Err(bad());
            }
        }
        let name = key.rsplit([':', '#']).next().ok_or_else(bad)?;
        let definition = Definition {
            gestures: rad_plugin_contract::default_gestures(),
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
            let rad_api::ResponseData::Visual(d) = self
                .request(crate::Request::PluginVisual { id: id.clone() })?
                .data
            else {
                return Err(bad());
            };
            let availability = match d.availability {
                rad_api::Availability::Current => "current",
                rad_api::Availability::Stale => "stale",
                rad_api::Availability::Unavailable => "unavailable",
            }
            .to_owned();
            let revision = d.revision;
            if revision.len() > 20
                || revision.parse::<u64>().is_err()
                || d.text.as_ref().is_some_and(|s| s.len() > 4096)
            {
                return Err(bad());
            }
            let text = d.text;
            let color = d.color;
            let mut pixels = None;
            let has_image = d.image;
            if has_image && availability != "unavailable" && revisions.get(id) != Some(&revision) {
                let mut bytes = vec![];
                loop {
                    let rad_api::ResponseData::VisualChunk(chunk) = self
                        .request(crate::Request::PluginVisualRead {
                            id: id.clone(),
                            revision: revision.clone(),
                            offset: bytes.len().to_string(),
                        })?
                        .data
                    else {
                        return Err(bad());
                    };
                    let part = decode_hex(&chunk.hex, 4096)?;
                    if part.is_empty() || bytes.len() + part.len() > 256 * 1024 {
                        return Err(bad());
                    }
                    bytes.extend(part);
                    if chunk.next_offset.is_none() {
                        break;
                    }
                    if chunk
                        .next_offset
                        .as_deref()
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
                text_position: match d.text_position {
                    rad_api::TextPosition::Top => "top",
                    rad_api::TextPosition::Middle => "middle",
                    rad_api::TextPosition::Bottom => "bottom",
                    rad_api::TextPosition::AboveImage => "above_image",
                    rad_api::TextPosition::BelowImage => "below_image",
                }
                .into(),
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
    Settings(String),
    AddAction(String),
    Action(String),
    Visual(String),
}
impl FormTarget {
    pub fn request(&self, key: &str, value: Value) -> Request {
        match self {
            Self::Settings(plugin_id) => Request::SetPluginSettings {
                plugin_id: plugin_id.clone(),
                parameters: Some(serde_json::from_value(value).expect("validated settings draft")),
            },
            Self::AddAction(button) => crate::commands::add_action(button, key, value),
            Self::Action(id) => Request::Edit {
                changes: vec![rad_api::Change::Set {
                    id: id.clone(),
                    edit: rad_api::FieldEdit::Parameters(rad_api::ActionParameters::Plugin(
                        serde_json::from_value(value).expect("plugin draft"),
                    )),
                }],
            },
            Self::Visual(id) => Request::Edit {
                changes: vec![rad_api::Change::Set {
                    id: id.clone(),
                    edit: rad_api::FieldEdit::Visual(rad_api::VisualSource {
                        type_key: key.into(),
                        parameters: Some(serde_json::from_value(value).expect("plugin draft")),
                    }),
                }],
            },
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
