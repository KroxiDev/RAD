//! Only importer for prerelease v1, shared by Store and portable packages.
use crate::{CodecError, document, v1_document};
pub(crate) fn import(bytes: &[u8]) -> Result<document::Document, CodecError> {
    // Decode the original closed schema first: Value alone would erase duplicate fields.
    let old: v1_document::Document =
        serde_json::from_slice(bytes).map_err(|_| CodecError::InvalidDocument)?;
    if !matches!(old.settings.gui_language.as_str(), "en" | "es") {
        return Err(CodecError::InvalidValue);
    }
    for button in &old.buttons {
        for gap in button.pauses_ms.iter().flatten() {
            if crate::decimal::<u64>(gap)? != 0 {
                return Err(CodecError::InvalidValue);
            }
        }
    }
    let mut value = serde_json::to_value(old).map_err(|_| CodecError::InvalidDocument)?;
    value["settings"]
        .as_object_mut()
        .ok_or(CodecError::InvalidDocument)?
        .remove("gui_language");
    value["version"] = serde_json::json!(5);
    value["grid"] = serde_json::json!([8, 4]);
    value["plugin_dependencies"] = serde_json::json!({});
    value["plugin_settings"] = serde_json::json!({});
    serde_json::from_value(value).map_err(|_| CodecError::InvalidDocument)
}
