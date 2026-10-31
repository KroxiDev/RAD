//! Closed v5 importer: per-state faces cannot be smuggled under an older version.
use crate::{CodecError, document, v5_document};
pub(crate) fn import(bytes: &[u8]) -> Result<document::Document, CodecError> {
    let old: v5_document::Document =
        serde_json::from_slice(bytes).map_err(|_| CodecError::InvalidDocument)?;
    let mut value = serde_json::to_value(old).map_err(|_| CodecError::InvalidDocument)?;
    value["version"] = serde_json::json!(6);
    serde_json::from_value(value).map_err(|_| CodecError::InvalidDocument)
}
