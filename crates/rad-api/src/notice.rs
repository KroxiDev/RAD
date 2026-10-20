use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    Board,
    Inspector,
    Overview,
    Catalog,
    Dependencies,
    Execution,
    Plugins,
}
impl NoticeKind {
    pub const ALL: [Self; 7] = [
        Self::Board,
        Self::Inspector,
        Self::Overview,
        Self::Catalog,
        Self::Dependencies,
        Self::Execution,
        Self::Plugins,
    ];
    pub const fn key(self) -> u8 {
        match self {
            Self::Board => 0,
            Self::Inspector => 1,
            Self::Overview => 2,
            Self::Catalog => 3,
            Self::Dependencies => 4,
            Self::Execution => 5,
            Self::Plugins => 6,
        }
    }
}
/// An invalidation, not a delta: re-read only the affected view. Intermediate
/// notices may coalesce. Session changes invalidate every cached view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notice {
    pub v: u8,
    pub session_id: String,
    pub kind: NoticeKind,
    pub revision: String,
}
impl Notice {
    /// Decode a validated wire JSON value without accepting serde's positional
    /// struct representation. Session/version checks belong to the connection.
    pub fn decode(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        crate::strict::from_value(value)
    }
}
