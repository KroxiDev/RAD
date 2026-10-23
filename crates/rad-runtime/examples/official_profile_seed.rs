//! Offline fixture seeding in a separate process, never part of measured runtime memory.
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::PathBuf,
};

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    assert_eq!(
        args.len(),
        2,
        "official_profile_seed ROOT audio|media|obs|metrics"
    );
    let root = PathBuf::from(&args[0]);
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let permitted = repo.join("target/performance").canonicalize().unwrap();
    assert!(root.is_absolute() && root.canonicalize().unwrap() == root);
    assert!(root.starts_with(&permitted) && root != permitted);
    let meta = fs::symlink_metadata(&root).unwrap();
    // SAFETY: geteuid takes no pointers and has no preconditions.
    assert!(meta.is_dir() && meta.mode() & 0o077 == 0 && meta.uid() == unsafe { libc::geteuid() });
    let name = args[1].to_str().expect("known fixture name");
    let relative_executable = match name {
        "audio" => "target/plugins/release/rad-plugin-audio",
        "media" => "target/plugins/release/rad-plugin-media",
        "obs" => "target/plugins/release/examples/obs-fixture",
        "metrics" => "target/plugins/release/examples/metrics-fixture",
        _ => panic!("unknown fixture; no arbitrary executable paths"),
    };
    for part in ["data", "data/rad", "data/rad/plugins"] {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join(part))
            .unwrap();
    }
    let manifest = fs::read(repo.join(format!("plugins/{name}/manifest.json"))).unwrap();
    let executable = fs::read(repo.join(relative_executable)).unwrap();
    let candidate = rad_plugin_package::Candidate::from_parts(
        &manifest,
        &executable,
        rad_plugin_contract::HOST_API,
        rad_plugin_host::TARGET,
    )
    .unwrap();
    assert_eq!(candidate.manifest().id, format!("org.rad.{name}"));
    let bytes = rad_plugin_package::encode(&candidate);
    let mut manager = rad_plugin_manager::Manager::create(
        &root.join("data/rad/plugins"),
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
    println!(
        "{}",
        serde_json::json!({"event":"seeded", "plugin":name,
            "executable":relative_executable,"executable_bytes":executable.len(),
            "package_bytes":bytes.len(),"separate_process":true})
    );
}
