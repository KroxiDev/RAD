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
    let original: Value = serde_json::from_slice(bytes).map_err(|_| crate::Error::CorruptState)?;
    let typed = serde_json::to_value(&old).map_err(|_| crate::Error::CorruptState)?;
    if !same_shape(&original, &typed) {
        return Err(crate::Error::CorruptState);
    }
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
    rad_plugin_package::Candidate::from_parts(&normalized, binary, old.host_max, target)
        .map_err(|_| crate::Error::CorruptState)?;
    Ok(metadata)
}

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
