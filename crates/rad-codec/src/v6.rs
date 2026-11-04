//! Closed v6 importer: the base position used to govern every state face.
use crate::{CodecError, document, v6_document};
pub(crate) fn import(bytes: &[u8]) -> Result<document::Document, CodecError> {
    let old: v6_document::Document =
        serde_json::from_slice(bytes).map_err(|_| CodecError::InvalidDocument)?;
    let mut value = serde_json::to_value(old).map_err(|_| CodecError::InvalidDocument)?;
    value["version"] = serde_json::json!(7);
    for button in value["buttons"]
        .as_array_mut()
        .ok_or(CodecError::InvalidDocument)?
    {
        let appearance = &mut button["appearance"];
        let position = appearance.get("text_position").cloned();
        if let Some(states) = appearance
            .get_mut("states")
            .and_then(serde_json::Value::as_object_mut)
        {
            for face in states.values_mut() {
                if let Some(position) = &position {
                    face["text_position"] = position.clone();
                }
            }
        }
    }
    serde_json::from_value(value).map_err(|_| CodecError::InvalidDocument)
}
