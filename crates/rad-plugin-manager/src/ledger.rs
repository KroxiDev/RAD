use crate::Error;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Versions {
    pub current: u64,
    pub previous: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Index {
    pub format: u32,
    pub revision: u64,
    pub plugins: BTreeMap<String, Versions>,
}
impl Index {
    pub fn empty() -> Self {
        Self {
            format: 1,
            revision: 0,
            plugins: BTreeMap::new(),
        }
    }
    pub fn bytes(&self) -> Result<Vec<u8>, Error> {
        serde_json::to_vec(self).map_err(|_| Error::CorruptState)
    }
    pub fn parse(b: &[u8]) -> Result<Self, Error> {
        let i: Self = serde_json::from_slice(b).map_err(|_| Error::CorruptState)?;
        let mut slots = std::collections::BTreeSet::new();
        let invalid = i.plugins.iter().any(|(id, v)| {
            v.source.as_ref().is_some_and(|s| {
                crate::GithubSource::parse(s).map_or(true, |p| p.canonical_url() != *s)
            }) || id.is_empty()
                || id.len() > 96
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
                || std::iter::once(v.current)
                    .chain(v.previous)
                    .any(|n| n == 0 || n > i.revision || !slots.insert(n))
        });
        if invalid
            || i.format != 1
            || i.plugins.len() > rad_plugin_contract::MAX_PLUGINS
            || i.bytes()? != b
        {
            return Err(Error::CorruptState);
        }
        Ok(i)
    }
}
pub(crate) fn slot(revision: u64) -> String {
    format!("v{revision:016x}")
}
