//! Serde accepts positional arrays for ordinary structs. The wire contract does
//! not: validate structural shape against the typed serialization, without a
//! second list of fields. Missing optional/defaulted members remain permitted.
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

pub(crate) fn from_value<T: DeserializeOwned + Serialize>(
    value: Value,
) -> Result<T, serde_json::Error> {
    let decoded: T = serde_json::from_value(value.clone())?;
    let normalized = serde_json::to_value(&decoded)?;
    if !same_shape(&value, &normalized) {
        return Err(<serde_json::Error as serde::de::Error>::custom(
            "invalid API object shape",
        ));
    }
    Ok(decoded)
}

fn same_shape(input: &Value, normalized: &Value) -> bool {
    match (input, normalized) {
        (Value::Object(input), Value::Object(normalized)) => input.iter().all(|(key, value)| {
            normalized
                .get(key)
                // DTOs (including flattened views) reject unknown fields. An
                // accepted optional null may be omitted by skip_serializing_if.
                .map_or(value.is_null(), |expected| same_shape(value, expected))
        }),
        (Value::Array(input), Value::Array(normalized)) => {
            input.len() == normalized.len()
                && input
                    .iter()
                    .zip(normalized)
                    .all(|(value, expected)| same_shape(value, expected))
        }
        (Value::Null, Value::Null)
        | (Value::Bool(_), Value::Bool(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_)) => true,
        _ => false,
    }
}
