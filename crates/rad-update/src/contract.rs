use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self(value.to_string())
    }
}
#[derive(Clone, Default, Debug)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub fn check(&self) -> Result<(), Error> {
        if self.is_cancelled() {
            Err(Error("cancelled".into()))
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Default)]
pub struct Progress(Arc<ProgressState>);
#[derive(Default)]
struct ProgressState {
    known: AtomicBool,
    percent: AtomicU8,
    notify: Option<Box<dyn Fn() + Send + Sync>>,
}
impl Progress {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(ProgressState {
            notify: Some(Box::new(notify)),
            ..Default::default()
        }))
    }
    pub fn percent(&self) -> Option<u8> {
        self.0
            .known
            .load(Ordering::Acquire)
            .then(|| self.0.percent.load(Ordering::Acquire))
    }
    pub fn report(&self, received: u64, total: u64) {
        if total == 0 {
            return;
        }
        let value = ((received.min(total) as u128) * 100 / total as u128) as u8;
        let known = self.0.known.swap(true, Ordering::AcqRel);
        if (self.0.percent.swap(value, Ordering::AcqRel) != value || !known)
            && let Some(notify) = &self.0.notify
        {
            notify();
        }
    }
}
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMetadata {
    pub schema: u32,
    pub product: String,
    pub package: String,
    pub version: String,
    pub pkgrel: u32,
    pub architecture: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
    pub base: String,
    pub commit: String,
}
impl ReleaseMetadata {
    pub fn validate(&self, tag: &str) -> Result<(), Error> {
        let identity = current_release()?;
        let version = semver::Version::parse(&self.version)
            .map_err(|_| Error("invalid product version".into()))?;
        if self.schema != 1
            || self.product != identity.product
            || self.package != identity.package
            || self.architecture != identity.architecture
            || self.base != identity.base
            || !version.pre.is_empty()
            || !version.build.is_empty()
            || version.to_string() != self.version
            || self.pkgrel == 0
            || tag != format!("v{}-{}", self.version, self.pkgrel)
            || self.file
                != format!(
                    "{}-{}-{}-{}.pkg.tar.zst",
                    self.package, self.version, self.pkgrel, self.architecture
                )
            || self.size == 0
            || self.size > MAX_PACKAGE_BYTES
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || self.commit.len() != 40
            || !self
                .commit
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(Error(
                "release metadata does not identify an official supported artifact".into(),
            ));
        }
        Ok(())
    }
    pub fn package_version(&self) -> String {
        format!("{}-{}", self.version, self.pkgrel)
    }
}
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub tag: String,
    pub metadata: ReleaseMetadata,
    pub installed_version: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckResult {
    NoUpdate,
    Available {
        candidate: Box<Candidate>,
        managed: bool,
    },
}
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Prepared {
    pub directory: PathBuf,
    pub candidate: Candidate,
}
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstallOutcome {
    pub installed_version: String,
    pub details: String,
}

/// Client protocol and release representation limits, independent from data/API versions.
pub const MAX_METADATA_BYTES: usize = 65_536;
pub const MAX_PACKAGE_BYTES: u64 = 536_870_912;
pub const MAX_REQUEST_BYTES: u64 = 16_384;
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseIdentity {
    pub schema: u32,
    pub product: String,
    pub package: String,
    pub version: String,
    pub pkgrel: u32,
    pub architecture: String,
    pub base: String,
}
/// The checked-in release identity is the sole product/package version source.
pub fn current_release() -> Result<ReleaseIdentity, Error> {
    serde_json::from_str(include_str!("../../../packaging/release.json"))
        .map_err(|_| Error("invalid compiled release identity".into()))
}
