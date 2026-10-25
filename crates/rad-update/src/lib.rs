//! Official installation updates, independent of runtime and deck data.
pub mod cleanup;
mod contract;
pub mod download;
pub mod installation;
pub mod installer;
pub mod release;
pub use contract::*;
pub use download::discard_prepared;

/// A manual query; constructing the UI does not call this function.
pub fn check(cancel: &Cancellation) -> Result<CheckResult, Error> {
    use std::time::Duration;
    cancel.check()?;
    cleanup::opportunistic();
    let managed = installation::managed()?;
    let identity = current_release()?;
    let installed = if managed {
        installation::installed_version()?
    } else {
        format!("{}-{}", identity.version, identity.pkgrel)
    };
    let candidate = release::check_with(
        &mut download::Curl::new(Duration::from_secs(45)),
        &installed,
        cancel,
    )?;
    Ok(match candidate {
        Some(candidate) => CheckResult::Available {
            candidate: Box::new(candidate),
            managed,
        },
        None => CheckResult::NoUpdate,
    })
}
/// Prepares and verifies every byte while the current runtime remains operational.
pub fn prepare(
    candidate: &Candidate,
    cancel: &Cancellation,
    progress: &Progress,
) -> Result<Prepared, Error> {
    use std::time::Duration;
    cancel.check()?;
    cleanup::opportunistic();
    if !installation::managed()? {
        return Err(Error(
            "this editor is not managed by the official package".into(),
        ));
    }
    candidate.metadata.validate(&candidate.tag)?;
    let installed = installation::installed_version()?;
    if installed != candidate.installed_version {
        return Err(Error(
            "the installed package changed; verify updates again".into(),
        ));
    }
    let fresh = release::check_with(
        &mut download::Curl::new(Duration::from_secs(45)),
        &installed,
        cancel,
    )?;
    if fresh.as_ref() != Some(candidate) {
        return Err(Error(
            "the official candidate changed; verify updates again".into(),
        ));
    }
    let mut stage = download::Temporary::new("stage")?;
    drop(stage.create(&candidate.metadata.file)?);
    download::Curl::new(Duration::from_secs(600)).fetch_file(
        &release::package_url(candidate)?,
        &stage.path.join(&candidate.metadata.file),
        candidate.metadata.size,
        cancel,
        progress,
    )?;
    download::verify_file(
        &stage.path.join(&candidate.metadata.file),
        &candidate.metadata,
        cancel,
    )?;
    cancel.check()?;
    download::save_candidate(&mut stage, candidate)?;
    std::fs::File::open(&stage.path)?.sync_all()?;
    let prepared = Prepared {
        directory: stage.path.clone(),
        candidate: candidate.clone(),
    };
    stage.keep = true;
    Ok(prepared)
}

pub mod coordinator;
pub mod gate;
