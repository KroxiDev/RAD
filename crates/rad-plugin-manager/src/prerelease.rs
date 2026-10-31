//! Read-only inventory bridge for packages installed before published contract v1.
//! This does not negotiate their old protocol or authorize execution.
use rad_plugin_contract::{Definition, Manifest, PluginLifecycle, Version};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Prerelease {
    #[serde(default)]
    lifecycle: PluginLifecycle,
    contract: u32,
    id: String,
    version: Version,
    host_min: u32,
    host_max: u32,
    targets: Vec<String>,
    actions: Vec<Definition>,
    visuals: Vec<Definition>,
}

pub(super) fn inventory(
    bytes: &[u8],
    binary: &[u8],
    target: &str,
) -> Result<Manifest, crate::Error> {
    let old: Prerelease = serde_json::from_slice(bytes).map_err(|_| crate::Error::CorruptState)?;
    // Deserialize the original bytes first: Value alone would erase duplicate fields.
    // Derive the expected object/list shape from the same typed metadata, not a
    // second field table. Serde's positional struct arrays are not JSON objects.
    let original: Value = serde_json::from_slice(bytes).map_err(|_| crate::Error::CorruptState)?;
    let typed = serde_json::to_value(&old).map_err(|_| crate::Error::CorruptState)?;
    if !same_shape(&original, &typed) {
        return Err(crate::Error::CorruptState);
    }
    // Only the known prerelease range, never a future published major or missing v1 caps.
    if !(2..=3).contains(&old.host_min) || !(old.host_min..=3).contains(&old.host_max) {
        return Err(crate::Error::CorruptState);
    }
    let mut capabilities = vec![];
    if !old.visuals.is_empty() {
        capabilities.push(rad_plugin_contract::VISUAL_FRESHNESS.into());
    }
    let definitions = || old.actions.iter().chain(&old.visuals);
    if definitions().any(|d| {
        d.schema
            .fields
            .iter()
            .any(|f| matches!(f.kind, rad_plugin_contract::FieldKind::DynamicChoice { .. }))
    }) {
        capabilities.push(rad_plugin_contract::DYNAMIC_OPTIONS.into());
    }
    if definitions().any(|d| !d.credential_slots.is_empty()) {
        capabilities.push(rad_plugin_contract::CREDENTIALS.into());
    }
    if definitions().any(|d| d.response_timeout.is_some()) {
        capabilities.push(rad_plugin_contract::RESPONSE_TIMEOUT.into());
    }
    let metadata = Manifest {
        lifecycle: old.lifecycle,
        contract: old.contract,
        capabilities,
        resources: vec![],
        settings: None,
        id: old.id,
        version: old.version,
        host_min: old.host_min,
        host_max: old.host_max,
        targets: old.targets,
        actions: old.actions,
        visuals: old.visuals,
        translations: Default::default(),
    };
    let normalized = serde_json::to_vec(&metadata).map_err(|_| crate::Error::CorruptState)?;
    // Reuse strict manifest/schema and ELF validation. The original bytes remain untouched.
    rad_plugin_package::Candidate::from_parts(&normalized, binary, old.host_max, target)
        .map_err(|_| crate::Error::CorruptState)?;
    Ok(metadata)
}

// Omitted defaults are allowed; an explicitly null Option may disappear during
// serialization. Unknown/duplicate fields were already rejected by typed parsing.
fn same_shape(original: &Value, typed: &Value) -> bool {
    match (original, typed) {
        (Value::Object(original), Value::Object(typed)) => original.iter().all(|(key, value)| {
            typed
                .get(key)
                .map_or_else(|| value.is_null(), |expected| same_shape(value, expected))
        }),
        (Value::Array(original), Value::Array(typed)) => {
            original.len() == typed.len()
                && original
                    .iter()
                    .zip(typed)
                    .all(|(value, expected)| same_shape(value, expected))
        }
        (_, Value::Object(_) | Value::Array(_)) | (Value::Object(_) | Value::Array(_), _) => false,
        _ => true,
    }
}
