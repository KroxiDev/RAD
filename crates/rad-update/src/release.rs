//! Stable release selection; all external responses are bounded and validated.
use crate::{Cancellation, Candidate, Error, ReleaseMetadata};

/// Injected source for controlled contract tests; production uses only the fixed official HTTPS origin.
pub trait Source {
    fn get(&mut self, url: &str, limit: usize, cancel: &Cancellation) -> Result<Vec<u8>, Error>;
}
/// Selects the highest eligible package, not the latest publication date.
pub fn check_with(
    source: &mut impl Source,
    installed: &str,
    cancel: &Cancellation,
) -> Result<Option<Candidate>, Error> {
    use std::cmp::Ordering;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    let mut best: Option<Candidate> = None;
    for page in 1..=5 {
        cancel.check()?;
        let url =
            format!("https://api.github.com/repos/KroxiDev/RAD/releases?per_page=100&page={page}");
        let bytes = source.get(&url, 1_048_576, cancel)?;
        if bytes.len() > 1_048_576 {
            return Err(Error("release page limit exceeded".into()));
        }
        let releases: Vec<Release> = serde_json::from_slice(&bytes)
            .map_err(|_| Error("malformed releases response".into()))?;
        if releases.len() > 100 {
            return Err(Error("release page limit exceeded".into()));
        }
        let complete = releases.len() < 100;
        for release in releases {
            cancel.check()?;
            if std::time::Instant::now() >= deadline {
                return Err(Error("release verification deadline exceeded".into()));
            }
            if release.assets.len() > 32 {
                return Err(Error("release asset limit exceeded".into()));
            }
            if release.draft || release.prerelease {
                continue;
            }
            let Some((version, rel)) = stable_tag(&release.tag_name) else {
                continue;
            };
            let package_version = format!("{version}-{rel}");
            if crate::installation::compare_versions(&package_version, installed)?
                != Ordering::Greater
            {
                continue;
            }
            let metadata_url = format!(
                "https://github.com/KroxiDev/RAD/releases/download/{}/rad-update.json",
                release.tag_name
            );
            let metadata_asset = exact_asset(&release.assets, "rad-update.json")?;
            if metadata_asset.browser_download_url != metadata_url
                || metadata_asset.size == 0
                || metadata_asset.size > crate::MAX_METADATA_BYTES as u64
            {
                return Err(Error("invalid metadata asset".into()));
            }
            let bytes = source.get(&metadata_url, crate::MAX_METADATA_BYTES, cancel)?;
            if bytes.len() as u64 != metadata_asset.size {
                return Err(Error(
                    "metadata asset length differs from release declaration".into(),
                ));
            }
            let metadata = parse_metadata(&bytes, &release.tag_name)?;
            let candidate = Candidate {
                tag: release.tag_name,
                metadata,
                installed_version: installed.into(),
            };
            let artifact = exact_asset(&release.assets, &candidate.metadata.file)?;
            if artifact.browser_download_url != package_url(&candidate)?
                || artifact.size != candidate.metadata.size
            {
                return Err(Error(
                    "package asset does not match release metadata".into(),
                ));
            }
            let replace = best
                .as_ref()
                .map(|old| {
                    crate::installation::compare_versions(
                        &candidate.metadata.package_version(),
                        &old.metadata.package_version(),
                    )
                })
                .transpose()?
                .is_none_or(|order| order == Ordering::Greater);
            if replace {
                best = Some(candidate);
            }
        }
        if complete {
            return Ok(best);
        }
    }
    Err(Error(
        "release pagination limit reached before a complete decision".into(),
    ))
}
pub fn package_url(candidate: &Candidate) -> Result<String, Error> {
    candidate.metadata.validate(&candidate.tag)?;
    Ok(format!(
        "https://github.com/KroxiDev/RAD/releases/download/{}/{}",
        candidate.tag, candidate.metadata.file
    ))
}
pub fn parse_metadata(bytes: &[u8], tag: &str) -> Result<ReleaseMetadata, Error> {
    if bytes.len() > crate::MAX_METADATA_BYTES {
        return Err(Error("metadata limit".into()));
    }
    let value: ReleaseMetadata =
        serde_json::from_slice(bytes).map_err(|_| Error("malformed release metadata".into()))?;
    value.validate(tag)?;
    Ok(value)
}

#[derive(serde::Deserialize)]
struct Release {
    draft: bool,
    prerelease: bool,
    tag_name: String,
    assets: Vec<Asset>,
}
#[derive(serde::Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}
fn exact_asset<'a>(assets: &'a [Asset], name: &str) -> Result<&'a Asset, Error> {
    let mut matching = assets.iter().filter(|asset| asset.name == name);
    let first = matching
        .next()
        .ok_or_else(|| Error(format!("missing release asset {name}")))?;
    if matching.next().is_some() {
        return Err(Error(format!("ambiguous release asset {name}")));
    }
    Ok(first)
}
fn stable_tag(tag: &str) -> Option<(String, u32)> {
    if tag.len() > 128 {
        return None;
    }
    let (version, rel) = tag.strip_prefix('v')?.rsplit_once('-')?;
    let parsed = semver::Version::parse(version).ok()?;
    let rel = rel.parse::<u32>().ok()?;
    if !parsed.pre.is_empty()
        || !parsed.build.is_empty()
        || parsed.to_string() != version
        || rel == 0
        || tag != format!("v{version}-{rel}")
    {
        return None;
    }
    Some((version.into(), rel))
}
