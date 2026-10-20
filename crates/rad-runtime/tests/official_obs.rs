//! Explicit cross-workspace conformance, synthetic OBS endpoint only.
use std::{fs, os::unix::fs::DirBuilderExt, path::PathBuf, process::Command};
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
#[ignore = "requires both workspaces' fixture binaries; scripts/conformance/obs/run.sh"]
fn official_obs_interoperates_with_runtime_and_cli() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = Scratch(std::env::temp_dir().join(format!("rad-d4-{}-{stamp}", std::process::id())));
    fs::DirBuilder::new().mode(0o700).create(&root.0).unwrap();
    for dir in ["data", "data/rad", "data/rad/plugins"] {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.0.join(dir))
            .unwrap();
    }
    let manifest = fs::read(repo.join("plugins/obs/manifest.json")).unwrap();
    let executable = fs::read(repo.join("target/plugins/release/examples/obs-fixture")).unwrap();
    let candidate = rad_plugin_package::Candidate::from_parts(
        &manifest,
        &executable,
        rad_plugin_contract::HOST_API,
        rad_plugin_host::TARGET,
    )
    .unwrap();
    assert_eq!(candidate.manifest().id, "org.rad.obs");
    let bytes = rad_plugin_package::encode(&candidate);
    let mut manager = rad_plugin_manager::Manager::create(
        &root.0.join("data/rad/plugins"),
        rad_plugin_contract::HOST_API,
        rad_plugin_host::TARGET,
    )
    .unwrap();
    let mut change = Some(
        manager
            .prepare(rad_plugin_manager::Operation::Install, Some(&bytes))
            .unwrap(),
    );
    assert_eq!(
        manager.commit(&mut change).unwrap(),
        rad_plugin_manager::CommitOutcome::Applied
    );
    drop(manager);
    assert!(
        Command::new("python3")
            .arg(repo.join("scripts/conformance/obs/check.py"))
            .arg(&root.0)
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );
}
