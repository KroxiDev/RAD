//! Offline seeding outside measured runtime memory; real metrics/OBS require explicit opt-in.
use std::{
    ffi::OsStr,
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::PathBuf,
};

fn real_selection(name: &str, option: Option<&OsStr>) -> Option<(&'static str, &'static str)> {
    let option = option?;
    Some(match (name, option) {
        ("metrics", option) if option == "--real-metrics" => {
            ("target/plugins/release/rad-plugin-metrics", "real_metrics")
        }
        ("obs", option) if option == "--real-obs" => {
            ("target/plugins/release/rad-plugin-obs", "real_obs")
        }
        _ => panic!("only matching metrics --real-metrics or obs --real-obs; no executable paths"),
    })
}

fn relative_executable(name: &str, option: Option<&OsStr>) -> &'static str {
    if let Some((executable, _)) = real_selection(name, option) {
        return executable;
    }
    match name {
        "audio" => "target/plugins/release/rad-plugin-audio",
        "media" => "target/plugins/release/rad-plugin-media",
        "obs" => "target/plugins/release/examples/obs-fixture",
        "metrics" => "target/plugins/release/examples/metrics-fixture",
        _ => panic!("unknown fixture; no arbitrary executable paths"),
    }
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    assert!(
        (2..=3).contains(&args.len()),
        "official_profile_seed ROOT audio|media|obs|metrics [--real-metrics|--real-obs]"
    );
    let name = args[1].to_str().expect("known fixture name");
    let relative_executable = relative_executable(name, args.get(2).map(|s| s.as_os_str()));
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
    let mut event = serde_json::json!({"event":"seeded", "plugin":name,
            "executable":relative_executable,"executable_bytes":executable.len(),
            "package_bytes":bytes.len(),"separate_process":true});
    if let Some((_, mode)) = real_selection(name, args.get(2).map(|s| s.as_os_str())) {
        event[mode] = true.into();
    }
    println!("{event}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_selection_remains_fixture_without_explicit_opt_in() {
        assert_eq!(
            relative_executable("metrics", None),
            "target/plugins/release/examples/metrics-fixture"
        );
        assert_eq!(
            relative_executable("obs", None),
            "target/plugins/release/examples/obs-fixture"
        );
        assert_eq!(
            relative_executable("metrics", Some(OsStr::new("--real-metrics"))),
            "target/plugins/release/rad-plugin-metrics"
        );
    }

    #[test]
    fn real_obs_requires_explicit_fixed_opt_in() {
        assert_eq!(
            relative_executable("obs", None),
            "target/plugins/release/examples/obs-fixture"
        );
        assert_eq!(
            relative_executable("obs", Some(OsStr::new("--real-obs"))),
            "target/plugins/release/rad-plugin-obs"
        );
    }

    #[test]
    fn opt_in_event_keys_are_scoped_to_the_selected_reader() {
        assert_eq!(
            real_selection("obs", Some(OsStr::new("--real-obs"))),
            Some(("target/plugins/release/rad-plugin-obs", "real_obs"))
        );
        assert_eq!(
            real_selection("metrics", Some(OsStr::new("--real-metrics"))),
            Some(("target/plugins/release/rad-plugin-metrics", "real_metrics"))
        );
        for name in ["audio", "media", "obs", "metrics"] {
            assert_eq!(real_selection(name, None), None);
        }
    }

    #[test]
    fn real_mode_cannot_select_another_plugin_or_arbitrary_executable() {
        for (name, option) in [
            ("audio", "--real-metrics"),
            ("metrics", "/arbitrary/executable"),
            ("metrics", "--real-metrics=other"),
            ("metrics", "--real-obs"),
            ("obs", "--real-metrics"),
            ("audio", "--real-obs"),
            ("obs", "/arbitrary/executable"),
            ("obs", "--real-obs=other"),
        ] {
            assert!(
                std::panic::catch_unwind(|| relative_executable(name, Some(OsStr::new(option))))
                    .is_err()
            );
        }
    }
}
