//! Closed v4 importer: Brightness is never accepted under a v4 label.
use crate::{CodecError, document, v4_document};
pub(crate) fn import(bytes: &[u8]) -> Result<document::Document, CodecError> {
    let old: v4_document::Document =
        serde_json::from_slice(bytes).map_err(|_| CodecError::InvalidDocument)?;
    let mut value = serde_json::to_value(old).map_err(|_| CodecError::InvalidDocument)?;
    value["version"] = serde_json::json!(5);
    serde_json::from_value(value).map_err(|_| CodecError::InvalidDocument)
}
