//! Cross-workspace conformance: run explicitly after building the official plugins.
//! No network, deck, desktop bus or audio server. The Python file is test orchestration only.
use std::{os::unix::fs::DirBuilderExt, path::PathBuf, process::Command};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "requires separately built official plugins; scripts/conformance/audio-media/run.sh"]
fn official_packages_interoperate_with_runtime_and_cli() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let binaries = repo.join("target/plugins/release");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = Scratch(std::env::temp_dir().join(format!("rad-d7-{}-{stamp}", std::process::id())));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root.0)
        .unwrap();
    for dir in ["data", "data/rad", "data/rad/plugins"] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(root.0.join(dir))
            .unwrap();
    }
    let mut manager = rad_plugin_manager::Manager::create(
        &root.0.join("data/rad/plugins"),
        2,
        rad_plugin_host::TARGET,
    )
    .unwrap();
    for name in ["audio", "media"] {
        let output = root.0.join(format!("{name}.radp"));
        assert!(
            Command::new(binaries.join("radp-pack"))
                .arg(repo.join(format!("plugins/{name}/manifest.json")))
                .arg(binaries.join(format!("rad-plugin-{name}")))
                .arg(&output)
                .status()
                .unwrap()
                .success()
        );
        let bytes = std::fs::read(&output).unwrap();
        let candidate =
            rad_plugin_package::Candidate::parse(&bytes, 2, rad_plugin_host::TARGET).unwrap();
        assert_eq!(
            rad_plugin_package::encode(&candidate),
            bytes,
            "independent RAD parser must round-trip SDK package"
        );
        let mut corrupt = bytes.clone();
        corrupt[0] ^= 1;
        assert!(
            rad_plugin_package::Candidate::parse(&corrupt, 2, rad_plugin_host::TARGET).is_err(),
            "negative: damaged magic"
        );
        assert!(
            rad_plugin_package::Candidate::parse(&bytes, 1, rad_plugin_host::TARGET).is_err(),
            "negative: old host API"
        );
        assert!(
            rad_plugin_package::Candidate::parse(
                &bytes[..bytes.len() - 1],
                2,
                rad_plugin_host::TARGET
            )
            .is_err(),
            "negative: truncation"
        );
        let mut prepared = Some(
            manager
                .prepare(rad_plugin_manager::Operation::Install, Some(&bytes))
                .unwrap(),
        );
        assert!(matches!(
            manager.commit(&mut prepared).unwrap(),
            rad_plugin_manager::CommitOutcome::Applied
        ));
        println!("PASS SDK RADP {name}: RAD parser, negative controls, durable Manager install");
    }
    drop(manager);
    assert!(
        Command::new("python3")
            .arg(repo.join("scripts/conformance/audio-media/check.py"))
            .arg(&root.0)
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );
}
