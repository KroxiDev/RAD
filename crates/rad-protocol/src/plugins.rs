//! Metadata is the A schema, never a parallel parameter model.
use super::*;
use rad_plugin_contract::{ChoiceList, Definition, Field, FieldKind, OptionsScope, Parameters};
use std::hash::{Hash, Hasher};

pub use rad_api::PluginOperation;
#[derive(Clone, PartialEq, Eq)]
pub struct PluginAdmin {
    pub operation: PluginOperation,
    pub plugin_id: Option<String>,
    pub source: Option<String>,
}
impl std::fmt::Debug for PluginAdmin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PluginAdmin([redacted])")
    }
}
/// A dynamic-options query resolved against the Authority. The runtime sends it to the
/// plugin host and answers with `Protocol::options_response`.
#[derive(PartialEq)]
pub struct PluginOptions {
    pub plugin: String,
    /// Stable selected-package identity, not a process generation. Runtime resolves it at dispatch.
    pub generation: u64,
    pub scope: OptionsScope,
    pub definition: String,
    pub field: String,
    pub context: Parameters,
    pub subject: OptionsSubject,
}
impl std::fmt::Debug for PluginOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PluginOptions([redacted])")
    }
}
#[derive(Clone, PartialEq, Eq)]
pub enum OptionsSubject {
    /// A draft that has no entity yet.
    Type,
    /// A saved entity, with the value its configuration holds for the field, if the current
    /// schema can read one.
    Entity { saved: Option<String> },
}
impl std::fmt::Debug for OptionsSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Type => "OptionsSubject::Type",
            Self::Entity { .. } => "OptionsSubject::Entity([redacted])",
        })
    }
}
impl Protocol {
    /// Shape the outcome of a query: `None` means the plugin did not answer in time.
    /// A list that cannot fit one response is refused, never truncated.
    pub fn options_response(
        &self,
        c: IncomingContext,
        query: &PluginOptions,
        list: Option<&ChoiceList>,
    ) -> Response {
        let Some(list) = list else {
            return self.rejection(c, "options_timeout", false);
        };
        let (availability, options) = match list {
            ChoiceList::Available { options } => ("available", options.as_slice()),
            ChoiceList::Unavailable => ("unavailable", &[][..]),
        };
        let separator = match query.scope {
            OptionsScope::Action => ':',
            OptionsScope::Visual => '#',
        };
        let mut data = json!({
            "type": format!("{}{separator}{}", query.plugin, query.definition),
            "field": query.field,
            "availability": availability,
            "items": options.iter().map(|o| json!({"value":o.value,"label":o.label})).collect::<Vec<_>>(),
            "next_cursor": null,
        });
        if let OptionsSubject::Entity { saved } = &query.subject {
            data["saved"] = json!(saved);
            data["saved_listed"] = json!(
                saved
                    .as_ref()
                    .is_some_and(|s| options.iter().any(|o| &o.value == s))
            );
        }
        let response = self.envelope(
            c,
            Status::Ok,
            rad_api::ResponseData::Options(
                serde_json::from_value(data).expect("options projection"),
            ),
            None,
        );
        if self.encode_response(&response).is_err() {
            return self.rejection(c, "resource_exhausted", false);
        }
        response
    }
}
pub(super) fn admin(c: IncomingContext, request: &rad_api::Request) -> Checked<PluginAdmin> {
    if c.origin != Origin::Editor {
        return Err("permission_denied");
    }
    let rad_api::Request::PluginAdmin {
        operation,
        plugin_id,
        source,
        trusted,
    } = request
    else {
        return Err("invalid_request");
    };
    let operation = *operation;
    let downloads = matches!(
        operation,
        PluginOperation::Install | PluginOperation::Update | PluginOperation::Reinstall
    );
    let source = if downloads {
        if *trusted != Some(true) {
            return Err("confirmation_required");
        }
        Some(source.clone().ok_or("invalid_request")?)
    } else {
        if source.is_some() {
            return Err("invalid_request");
        }
        None
    };
    let plugin_id = if operation != PluginOperation::Install {
        Some(plugin_id.clone().ok_or("invalid_request")?)
    } else {
        None
    };
    Ok(PluginAdmin {
        operation,
        plugin_id,
        source,
    })
}
pub(super) fn catalog(a: &Authority, c: IncomingContext, cursor: Option<&str>) -> Checked<Value> {
    let mut items = Vec::new();
    for (m, selection, _) in a.plugin_manifests() {
        let ready = a.plugin_available(&m.id, selection);
        for (kind, separator, defs) in [("action", ':', &m.actions), ("visual", '#', &m.visuals)] {
            for d in defs {
                items.push(json!({"type":format!("{}{separator}{}",m.id,d.id),"kind":kind,"label":d.label,"version":m.version,"ready":ready,"gestures":d.gestures}));
            }
        }
    }
    page(a, c, "catalog", cursor, items)
}
pub(super) fn schema(
    a: &Authority,
    c: IncomingContext,
    name: &str,
    cursor: Option<&str>,
) -> Checked<Value> {
    let (plugin, key, visual) = split(name)?;
    let d = published(a, plugin, key, visual)?.definition;
    let mut result = page(
        a,
        c,
        name,
        cursor,
        d.schema.fields.iter().map(|f| json!(f)).collect(),
    )?;
    result["type"] = json!(name);
    result["schema_version"] = json!(d.schema.version);
    result["credential_slots"] = json!(d.credential_slots);
    Ok(result)
}
/// A definition of a published plugin, as the Authority knows it.
struct Published<'a> {
    plugin: &'a str,
    definition: &'a Definition,
    visual: bool,
    generation: u64,
    /// False once the plugin failed: it is still published, but cannot be asked.
    ready: bool,
}
fn published<'a>(
    a: &'a Authority,
    plugin: &'a str,
    name: &str,
    visual: bool,
) -> Checked<Published<'a>> {
    let (manifest, generation, _) = a
        .plugin_manifests()
        .find(|(m, _, _)| m.id == plugin)
        .ok_or("executor_unavailable")?;
    let definition = if visual {
        &manifest.visuals
    } else {
        &manifest.actions
    }
    .iter()
    .find(|d| d.id == name)
    .ok_or("executor_unavailable")?;
    Ok(Published {
        plugin,
        definition,
        visual,
        generation,
        ready: a.plugin_available(plugin, generation),
    })
}
fn split(name: &str) -> Checked<(&str, &str, bool)> {
    if let Some((p, k)) = name.split_once(':') {
        Ok((p, k, false))
    } else if let Some((p, k)) = name.split_once('#') {
        Ok((p, k, true))
    } else {
        Err("executor_unavailable")
    }
}
fn page(
    a: &Authority,
    c: IncomingContext,
    query: &str,
    cursor: Option<&str>,
    items: Vec<Value>,
) -> Checked<Value> {
    let mut hash = std::hash::DefaultHasher::new();
    query.hash(&mut hash);
    serde_json::to_vec(&items)
        .map_err(|_| "resource_exhausted")?
        .hash(&mut hash);
    let prefix = format!(
        "{}:{}:{}:",
        c.connection,
        a.view_revision(c.origin)
            .ok_or("resource_exhausted")?
            .value(),
        hash.finish()
    );
    let offset = match cursor {
        None => 0,
        Some(s) => s
            .strip_prefix(&prefix)
            .and_then(|n| n.parse::<usize>().ok())
            .ok_or("cursor_expired")?,
    };
    if offset > items.len() {
        return Err("cursor_expired");
    }
    let mut end = offset;
    let mut bytes = 2048;
    while end < items.len() && end - offset < 32 {
        let n = serde_json::to_vec(&items[end])
            .map_err(|_| "resource_exhausted")?
            .len()
            + 1;
        if bytes + n > 46 * 1024 {
            break;
        }
        bytes += n;
        end += 1;
    }
    if end == offset && offset < items.len() {
        return Err("resource_exhausted");
    }
    Ok(
        json!({"items":items[offset..end],"next_cursor":(end<items.len()).then(||format!("{prefix}{end}"))}),
    )
}

pub(super) fn configuration(
    a: &Authority,
    c: IncomingContext,
    id: &str,
    cursor: Option<&str>,
) -> Checked<Value> {
    let entity = parse_id(id)?;
    let (key, bytes, visual) = a
        .plugin_configuration(c.origin, entity)
        .ok_or("not_found")?;
    let items = bytes
        .chunks(2048)
        .map(|part| json!(part.iter().map(|b| format!("{b:02x}")).collect::<String>()))
        .collect();
    let mut result = page(a, c, &format!("configuration:{entity:?}"), cursor, items)?;
    result["type"] = json!(format!(
        "{}{}{}",
        key.plugin(),
        if visual { '#' } else { ':' },
        key.name()
    ));
    result["encoding"] = json!("hex");
    Ok(result)
}

/// What an `options` request for a saved entity turns into.
pub(super) enum Listing {
    /// Fixed choices of the schema, already paged.
    Fixed(Value),
    /// A dynamic field: the runtime must ask the plugin.
    Dynamic(Box<PluginOptions>),
}
pub(super) fn options(
    a: &Authority,
    c: IncomingContext,
    id: &str,
    field_name: &str,
    cursor: Option<&str>,
    context: Option<&std::collections::BTreeMap<String, String>>,
) -> Checked<Listing> {
    let entity = parse_id(id)?;
    let (key, stored, visual) = a
        .plugin_configuration(c.origin, entity)
        .ok_or("not_found")?;
    let published = published(a, key.plugin(), key.name(), visual)?;
    let field = field(published.definition, field_name)?;
    match &field.kind {
        FieldKind::Choice { options } => {
            if context.is_some() {
                return Err("invalid_request");
            }
            page(
                a,
                c,
                &format!("options:{entity:?}:{}", field.id),
                cursor,
                options.iter().map(|s| json!(s)).collect(),
            )
            .map(Listing::Fixed)
        }
        FieldKind::DynamicChoice { depends_on } => {
            let stored = readable(published.definition, stored);
            let saved = stored
                .as_ref()
                .and_then(|p| p.values.get(&field.id))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let subject = OptionsSubject::Entity { saved };
            dynamic(
                &published,
                field,
                depends_on,
                stored.as_ref(),
                cursor,
                context,
                subject,
            )
            .map(Box::new)
            .map(Listing::Dynamic)
        }
        _ => Err("invalid_request"),
    }
}
/// A draft that has no entity yet: only dynamic fields, only from the request's context.
/// Options are live state of the outside system (devices, players, scenes), not manifest
/// metadata like `schema`: automation may list them by type only while the global Consulta
/// is on. Without it, it can still ask entity by entity where the user allowed it.
pub(super) fn type_options(
    a: &Authority,
    origin: Origin,
    type_key: &str,
    field_name: &str,
    cursor: Option<&str>,
    context: Option<&std::collections::BTreeMap<String, String>>,
) -> Checked<PluginOptions> {
    if origin != Origin::Editor && !a.global_consult() {
        return Err("permission_denied");
    }
    let (plugin, name, visual) = split(type_key)?;
    let published = published(a, plugin, name, visual)?;
    let field = field(published.definition, field_name)?;
    let FieldKind::DynamicChoice { depends_on } = &field.kind else {
        return Err("invalid_request");
    };
    dynamic(
        &published,
        field,
        depends_on,
        None,
        cursor,
        context,
        OptionsSubject::Type,
    )
}
fn field<'a>(definition: &'a Definition, name: &str) -> Checked<&'a Field> {
    definition
        .schema
        .fields
        .iter()
        .find(|f| f.id == name)
        .ok_or("invalid_request")
}
/// A saved configuration the current schema can interpret; anything else contributes nothing.
fn readable(definition: &Definition, bytes: &[u8]) -> Option<Parameters> {
    let parameters: Parameters = serde_json::from_value(rad_wire::parse_json(bytes).ok()?).ok()?;
    (parameters.schema_version == definition.schema.version).then_some(parameters)
}
/// The query for a dynamic field. Its context is the saved values of the dependencies that
/// the current schema still accepts, overridden by the request's draft, which must be valid
/// as given. The list always fits one response, so there is no second page to ask for.
fn dynamic(
    published: &Published,
    field: &Field,
    depends_on: &[String],
    stored: Option<&Parameters>,
    cursor: Option<&str>,
    draft: Option<&std::collections::BTreeMap<String, String>>,
    subject: OptionsSubject,
) -> Checked<PluginOptions> {
    if cursor.is_some() {
        return Err("cursor_expired");
    }
    let definition = published.definition;
    let schema_version = definition.schema.version;
    let mut values = Map::new();
    for dependency in depends_on {
        let Some(value) = stored.and_then(|p| p.values.get(dependency)) else {
            continue;
        };
        let alone = Parameters {
            schema_version,
            values: Map::from_iter([(dependency.clone(), value.clone())]),
        };
        if definition
            .validate_options_context(&field.id, &alone)
            .is_ok()
        {
            values.insert(dependency.clone(), value.clone());
        }
    }
    if let Some(draft) = draft {
        values.extend(
            draft
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone()))),
        );
    }
    let context = Parameters {
        schema_version,
        values,
    };
    definition
        .validate_options_context(&field.id, &context)
        .map_err(|_| "invalid_request")?;
    if !published.ready {
        return Err("executor_unavailable");
    }
    Ok(PluginOptions {
        plugin: published.plugin.to_owned(),
        generation: published.generation,
        scope: if published.visual {
            OptionsScope::Visual
        } else {
            OptionsScope::Action
        },
        definition: definition.id.clone(),
        field: field.id.clone(),
        context,
        subject,
    })
}

pub(super) fn dependencies(
    a: &Authority,
    c: IncomingContext,
    cursor: Option<&str>,
) -> Checked<Value> {
    let references = a.plugin_dependencies(c.origin).ok_or("permission_denied")?;
    let items=references.into_iter().map(|(id,dependency)| {
        let slots=a.plugin_credential_status(&id);
        let installed=a.plugin_manifests().any(|(manifest,_,_)|manifest.id==id);
        let needs=installed.then(||slots.iter().any(|(_,present)|!present));
        json!({"id":id,"version":dependency.version.map(|v|json!({"major":v.major,"minor":v.minor,"patch":v.patch})),"origin":dependency.origin,"credential_slots":slots.iter().map(|(slot,_)|slot).collect::<Vec<_>>(),"needs_credentials":needs})
    }).collect();
    let mut result = page(a, c, "plugin_dependencies", cursor, items)?;
    result["view_revision"] = json!(
        a.view_revision(c.origin)
            .ok_or("resource_exhausted")?
            .value()
            .to_string()
    );
    Ok(result)
}
