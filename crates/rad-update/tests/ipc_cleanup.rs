//! Critical updater-only IPC collection; every path belongs to a synthetic fixture.
use rad_update::{
    cleanup::{Liveness, ProcessState},
    coordinator,
};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
};
fn namespace() -> String {
    let metadata = fs::File::open("/proc/self/ns/pid")
        .unwrap()
        .metadata()
        .unwrap();
    format!("{:016x}{:016x}", metadata.dev(), metadata.ino())
}
struct Processes;
impl Liveness for Processes {
    fn state(&self, pid: u32) -> ProcessState {
        match pid {
            101 => ProcessState::Dead,
            102 => ProcessState::Alive,
            _ => ProcessState::Unknown,
        }
    }
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "rad-update-ipc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, b"not JSON, and never parsed").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn only_private_exact_shape_files_of_dead_owners_are_collected_and_diagnostics_survive() {
    let fixture = Fixture::new();
    let ns = namespace();
    let nonce = "0123456789abcdef0123456789abcdef";
    let removed = [
        format!(".write-{ns}-101-{nonce}"),
        format!(".grant-{ns}-101-{nonce}"),
        format!("request-{ns}-101-{nonce}.json"),
    ];
    for name in &removed {
        fixture.file(name);
    }
    let retained = [
        format!(".write-{ns}-102-{nonce}"),
        format!(".write-101-{nonce}"),
        format!(".grant-101-{nonce}"),
        format!("request-101-{nonce}.json"),
        format!(".write-{}-101-{nonce}", "0".repeat(32)),
        format!(".write-{ns}-103-{nonce}"),
        format!("request-{nonce}.json"),
        format!("request-{ns}-0101-{nonce}.json"),
        format!("request-{ns}-101-{nonce}.json.extra"),
        format!(".write-{ns}-101-{nonce}A"),
        "operation.json".into(),
        "handoff.json".into(),
        "grant.json".into(),
        "opened.json".into(),
        "diagnostic.log".into(),
    ];
    for name in &retained {
        fixture.file(name);
    }
    let linked = fixture.file("kept-source");
    symlink(
        &linked,
        fixture
            .0
            .join(format!(".write-{ns}-101-{}", "b".repeat(32))),
    )
    .unwrap();
    fs::hard_link(
        &linked,
        fixture
            .0
            .join(format!(".write-{ns}-101-{}", "c".repeat(32))),
    )
    .unwrap();
    let public = fixture.file(&format!(".write-{ns}-101-{}", "d".repeat(32)));
    fs::set_permissions(&public, fs::Permissions::from_mode(0o644)).unwrap();
    let nested = fixture
        .0
        .join(format!(".write-{ns}-101-{}", "e".repeat(32)));
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("keep"), b"no recursion").unwrap();
    // SAFETY: geteuid has no preconditions.
    let removed_count =
        coordinator::collect_ipc_in(&fixture.0, unsafe { libc::geteuid() }, &Processes).unwrap();
    assert_eq!(removed_count, removed.len());
    for name in removed {
        assert!(!fixture.0.join(name).exists());
    }
    for name in retained {
        assert!(fixture.0.join(&name).exists(), "retained {name}");
    }
    assert!(public.exists());
    assert!(nested.join("keep").exists());
    assert_eq!(fs::read(linked).unwrap(), b"not JSON, and never parsed");
}

#[test]
fn collector_does_not_follow_directory_links_or_scan_beyond_its_fixed_bound() {
    let fixture = Fixture::new();
    let ns = namespace();
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let aliased = fixture.0.with_extension("link");
    symlink(&fixture.0, &aliased).unwrap();
    assert!(coordinator::collect_ipc_in(&aliased, uid, &Processes).is_err());
    fs::remove_file(aliased).unwrap();
    for index in 0..300 {
        fixture.file(&format!(".write-{ns}-101-{index:032x}"));
    }
    let removed = coordinator::collect_ipc_in(&fixture.0, uid, &Processes).unwrap();
    assert_eq!(removed, 256);
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 44);
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(coordinator::collect_ipc_in(&fixture.0, uid, &Processes).is_err());
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 44);
}
#[test]
fn private_final_directory_does_not_authorize_a_writable_ancestor() {
    let fixture = Fixture::new();
    let ns = namespace();
    let shared = fixture.0.join("unsafe-parent");
    let directory = shared.join("ipc");
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let file = directory.join(format!(".write-{ns}-101-{}", "a".repeat(32)));
    fs::write(&file, b"preserve").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    // SAFETY: geteuid has no preconditions.
    let result = coordinator::collect_ipc_in(&directory, unsafe { libc::geteuid() }, &Processes);
    assert!(
        result.is_err(),
        "the directory descriptor walk must reject writable ancestors"
    );
    assert!(file.exists());
}

#[test]
fn legacy_and_foreign_namespace_files_never_trigger_a_pid_probe() {
    struct NeverProbe;
    impl Liveness for NeverProbe {
        fn state(&self, _pid: u32) -> ProcessState {
            panic!("a foreign or unknown PID namespace must not be probed");
        }
    }
    let fixture = Fixture::new();
    let nonce = "a".repeat(32);
    for name in [
        format!(".write-101-{nonce}"),
        format!(".grant-101-{nonce}"),
        format!("request-101-{nonce}.json"),
        format!(".write-{}-101-{nonce}", "0".repeat(32)),
        format!("request-unknown-101-{nonce}.json"),
    ] {
        fixture.file(&name);
    }
    // SAFETY: geteuid has no preconditions.
    assert_eq!(
        coordinator::collect_ipc_in(&fixture.0, unsafe { libc::geteuid() }, &NeverProbe).unwrap(),
        0
    );
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 5);
}
