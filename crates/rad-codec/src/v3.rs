//! Closed v3 importer: action variants added by v4 are never accepted under v3.
use crate::{CodecError, document, v3_document};
pub(crate) fn import(bytes: &[u8]) -> Result<document::Document, CodecError> {
    let old: v3_document::Document =
        serde_json::from_slice(bytes).map_err(|_| CodecError::InvalidDocument)?;
    let mut value = serde_json::to_value(old).map_err(|_| CodecError::InvalidDocument)?;
    value["version"] = serde_json::json!(4);
    serde_json::from_value(value).map_err(|_| CodecError::InvalidDocument)
}
