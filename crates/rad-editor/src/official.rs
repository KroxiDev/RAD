//! The official plugins (E5): one Install button each, from their current release in
//! KroxiDev/rad-plugins. Publishing a new official release updates its line here, so this
//! list is also where the editor learns that an installed official plugin has an update.
use serde_json::Value;

pub struct Official {
    pub id: &'static str,
    /// Catalog key of the name the user reads.
    pub name: &'static str,
    pub version: [u64; 3],
    pub source: &'static str,
}

pub const OFFICIAL: [Official; 4] = [
    Official {
        id: "org.rad.audio",
        name: "Audio",
        version: [0, 2, 0],
        source: "https://github.com/KroxiDev/rad-plugins/releases/tag/audio-v0.2.0",
    },
    Official {
        id: "org.rad.media",
        name: "Media",
        version: [0, 2, 0],
        source: "https://github.com/KroxiDev/rad-plugins/releases/tag/media-v0.2.0",
    },
    Official {
        id: "org.rad.obs",
        name: "OBS",
        version: [0, 2, 0],
        source: "https://github.com/KroxiDev/rad-plugins/releases/tag/obs-v0.2.0",
    },
    Official {
        id: "org.rad.metrics",
        name: "Metrics",
        version: [0, 2, 0],
        source: "https://github.com/KroxiDev/rad-plugins/releases/tag/metrics-v0.2.0",
    },
];

pub fn find(id: &str) -> Option<&'static Official> {
    OFFICIAL.iter().find(|official| official.id == id)
}

impl Official {
    /// The current release is newer than `installed`, a version as the plugin status gives it.
    pub fn newer_than(&self, installed: &Value) -> bool {
        let part = |key: &str| installed[key].as_u64();
        match (part("major"), part("minor"), part("patch")) {
            (Some(major), Some(minor), Some(patch)) => self.version > [major, minor, patch],
            _ => false,
        }
    }
}
