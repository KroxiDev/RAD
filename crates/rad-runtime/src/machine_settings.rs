//! Machine-local options. Phase 1 stores them without changing device/service policy.
use crate::private_file::{invalid, read_bounded, write_atomic};
use std::{io, path::Path};
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MachineSettings {
    pub use_stream_deck: bool,
    pub start_with_session: bool,
}
impl MachineSettings {
    /// The directory is the runtime's private local data directory, not the Store.
    pub fn load(directory: &Path) -> io::Result<Self> {
        let Some(bytes) = read_bounded(&directory.join("machine.json"), 4096)? else {
            return Ok(Self::default());
        };
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if value["version"] != 1 || value.as_object().is_none_or(|o| o.len() != 3) {
            return Err(invalid());
        }
        Ok(Self {
            use_stream_deck: value["use_stream_deck"].as_bool().ok_or_else(invalid)?,
            start_with_session: value["start_with_session"].as_bool().ok_or_else(invalid)?,
        })
    }
    pub fn save(self, directory: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec(
            &serde_json::json!({"version":1,"use_stream_deck":self.use_stream_deck,"start_with_session":self.start_with_session}),
        )?;
        write_atomic(directory, "machine.json", &bytes)
    }
}
