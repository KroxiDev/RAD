//! Cross-workspace metrics conformance; deterministic reader, never proc/NVML or XL.
use std::{fs, os::unix::fs::DirBuilderExt, path::PathBuf, process::Command};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "requires separately built metrics fixtures; scripts/conformance/metrics/run.sh"]
fn official_metrics_interoperates_without_hardware() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = Scratch(repo.join(format!("target/f5-{}-{stamp}", std::process::id())));
    fs::DirBuilder::new().mode(0o700).create(&root.0).unwrap();
    for dir in ["data", "data/rad", "data/rad/plugins"] {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.0.join(dir))
            .unwrap();
    }
    let output = root.0.join("metrics.radp");
    assert!(
        Command::new(repo.join("target/plugins/release/radp-pack"))
            .arg(repo.join("plugins/metrics/manifest.json"))
            .arg(repo.join("target/plugins/release/examples/metrics-fixture"))
            .arg(&output)
            .status()
            .unwrap()
            .success()
    );
    let bytes = fs::read(&output).unwrap();
    let candidate =
        rad_plugin_package::Candidate::parse(&bytes, 3, rad_plugin_host::TARGET).unwrap();
    assert_eq!(candidate.manifest().id, "org.rad.metrics");
    assert!(candidate.manifest().actions.is_empty());
    assert_eq!(
        candidate
            .manifest()
            .visuals
            .iter()
            .map(|d| d.id.as_str())
            .collect::<Vec<_>>(),
        ["cpu", "ram", "gpu", "vram"]
    );
    assert_eq!(rad_plugin_package::encode(&candidate), bytes);
    assert!(
        rad_plugin_package::Candidate::parse(&bytes[..bytes.len() - 1], 3, rad_plugin_host::TARGET)
            .is_err()
    );
    let store = root.0.join("data/rad/plugins");
    let mut manager =
        rad_plugin_manager::Manager::create(&store, 3, rad_plugin_host::TARGET).unwrap();
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
    println!("PASS SDK RADP official manifest, independent parser and durable install");
    let check = Command::new("python3")
        .args(["-I", "-S", "-B"])
        .arg(repo.join("scripts/conformance/metrics/check.py"))
        .arg(&root.0)
        .arg(&repo)
        .arg("installed")
        .status()
        .unwrap();
    assert!(check.success(), "I1 runtime/IPC contract must be verified");
    let check = Command::new("python3")
        .args(["-I", "-S", "-B"])
        .arg(repo.join("scripts/conformance/metrics/check.py"))
        .arg(&root.0)
        .arg(&repo)
        .arg("uninstall")
        .status()
        .unwrap();
    assert!(
        check.success(),
        "I2 IPC uninstall and retained placeholder must be verified"
    );
    let mut manager =
        rad_plugin_manager::Manager::open(&store, 3, rad_plugin_host::TARGET).unwrap();
    assert!(manager.installed().unwrap().is_empty());
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
    let check = Command::new("python3")
        .args(["-I", "-S", "-B"])
        .arg(repo.join("scripts/conformance/metrics/check.py"))
        .arg(&root.0)
        .arg(&repo)
        .arg("restored")
        .status()
        .unwrap();
    assert!(
        check.success(),
        "I2 compatible reinstall must recover saved visuals"
    );
    let fault_executable = fs::read(
        repo.join("target/conformance/metrics-fixture/release/metrics-conformance-fixture"),
    )
    .unwrap();
    let fault_manifest = fs::read(repo.join("plugins/metrics/manifest.json")).unwrap();
    let fault_candidate = rad_plugin_package::Candidate::from_parts(
        &fault_manifest,
        &fault_executable,
        3,
        rad_plugin_host::TARGET,
    )
    .unwrap();
    let fault_bytes = rad_plugin_package::encode(&fault_candidate);
    let mut manager =
        rad_plugin_manager::Manager::open(&store, 3, rad_plugin_host::TARGET).unwrap();
    let mut change = Some(
        manager
            .prepare(
                rad_plugin_manager::Operation::Reinstall("org.rad.metrics".into()),
                Some(&fault_bytes),
            )
            .unwrap(),
    );
    assert_eq!(
        manager.commit(&mut change).unwrap(),
        rad_plugin_manager::CommitOutcome::Applied
    );
    drop(manager);
    let check = Command::new("python3")
        .args(["-I", "-S", "-B"])
        .arg(repo.join("scripts/conformance/metrics/check.py"))
        .arg(&root.0)
        .arg(&repo)
        .arg("fault")
        .status()
        .unwrap();
    assert!(
        check.success(),
        "I3 missing/stale reader must not block runtime"
    );
}
