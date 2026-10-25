//! Critical cleanup contract; all collection is scoped to this test's private synthetic parent.
use rad_update::cleanup::{self, Liveness, ProcessState};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
struct Parent(PathBuf);
impl Parent {
    fn new() -> Self {
        let random = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("rad-gc-critical-{}-{random}", std::process::id()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn directory(&self, kind: &str, nonce: u128, pid: u32) -> PathBuf {
        let uid = uid();
        let nonce = format!("{nonce:032x}");
        let path = self.0.join(format!("rad-update-{kind}-{uid}-{nonce}"));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        file(
            &path.join(".owner.json"),
            serde_json::to_vec(
                &serde_json::json!({"schema":1,"uid":uid,"kind":kind,"pid":pid,"namespace":namespace(),"nonce":nonce}),
            )
            .unwrap()
            .as_slice(),
        );
        file(&path.join(".lease"), b"");
        path
    }
}
impl Drop for Parent {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn namespace() -> String {
    let meta = fs::metadata("/proc/self/ns/pid").unwrap();
    format!("{:016x}{:016x}", meta.dev(), meta.ino())
}
fn uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}
fn file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
struct State(ProcessState);
impl Liveness for State {
    fn state(&self, _: u32) -> ProcessState {
        self.0
    }
}
#[test]
fn collects_only_proven_dead_owned_directory_with_known_contents_and_free_lease() {
    let parent = Parent::new();
    let path = parent.directory("fetch", 1, 12345);
    file(&path.join("response"), b"synthetic abandoned download");
    let report = cleanup::collect_in(&parent.0, uid(), &State(ProcessState::Dead)).unwrap();
    assert_eq!(report.removed, 1);
    assert!(!path.exists());
    assert_eq!(report.inspected, 1);
}
#[test]
fn live_reused_or_unknown_owner_and_shared_consumers_are_retained_without_signals() {
    use std::os::fd::AsRawFd;
    let parent = Parent::new();
    let path = parent.directory("fetch", 2, std::process::id());
    file(&path.join("response"), b"keep this transfer");
    for state in [ProcessState::Alive, ProcessState::Unknown] {
        assert_eq!(
            cleanup::collect_in(&parent.0, uid(), &State(state))
                .unwrap()
                .removed,
            0
        );
        assert!(path.join("response").exists());
    }
    let lease = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path.join(".lease"))
        .unwrap();
    // SAFETY: lease is live and LOCK_SH acquires a nonblocking shared lock only.
    assert_eq!(
        unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) },
        0
    );
    assert_eq!(
        cleanup::collect_in(&parent.0, uid(), &State(ProcessState::Dead))
            .unwrap()
            .removed,
        0
    );
    assert!(path.exists());
    drop(lease);
    assert_eq!(
        cleanup::collect_in(&parent.0, uid(), &State(ProcessState::Dead))
            .unwrap()
            .removed,
        1
    );
}
#[test]
fn only_known_files_in_all_four_owned_kinds_can_be_collected() {
    let parent = Parent::new();
    for (n, kind) in ["fetch", "stage", "root", "config"].into_iter().enumerate() {
        let path = parent.directory(kind, n as u128, 45678);
        let files: Vec<&str> = match kind {
            "fetch" => vec!["response"],
            "stage" => vec![
                "candidate.json",
                "rust-action-deck-0.2.0-1-x86_64.pkg.tar.zst",
            ],
            "root" => vec!["rust-action-deck-0.2.0-1-x86_64.pkg.tar.zst"],
            _ => vec!["pacman.conf", "00-rad-update-guard.hook"],
        };
        for name in files {
            file(&path.join(name), b"controlled partial temporary bytes");
        }
    }
    assert_eq!(
        cleanup::collect_in(&parent.0, uid(), &State(ProcessState::Dead))
            .unwrap()
            .removed,
        4
    );
}
#[test]
fn legacy_symlinks_hardlinks_unsafe_permissions_and_unknown_contents_are_conserved() {
    let parent = Parent::new();
    let dead = State(ProcessState::Dead);
    let legacy = parent.directory("fetch", 10, 45678);
    fs::remove_file(legacy.join(".owner.json")).unwrap();
    file(&legacy.join("response"), b"legacy");
    let unknown = parent.directory("fetch", 11, 45678);
    file(&unknown.join("response"), b"known but must remain");
    file(&unknown.join("unrecognized"), b"unknown");
    let linked = parent.directory("fetch", 12, 45678);
    let outside = parent.0.join("not-updater-data");
    file(&outside, b"must not be touched");
    std::os::unix::fs::symlink(&outside, linked.join("response")).unwrap();
    let hardlinked = parent.directory("fetch", 13, 45678);
    fs::hard_link(&outside, hardlinked.join("response")).unwrap();
    let insecure = parent.directory("fetch", 14, 45678);
    file(&insecure.join("response"), b"insecure");
    fs::set_permissions(insecure.join("response"), fs::Permissions::from_mode(0o644)).unwrap();
    let unsafe_marker = parent.directory("fetch", 15, 45678);
    file(&unsafe_marker.join(".owner.json"), &vec![b'x'; 1025]);
    let wrong_marker = parent.directory("fetch", 16, 45678);
    file(
        &wrong_marker.join(".owner.json"),
        br#"{"schema":1,"uid":0,"kind":"fetch","pid":45678,"nonce":"wrong"}"#,
    );
    let target = parent.0.join("unrelated-directory");
    fs::create_dir(&target).unwrap();
    file(&target.join("response"), b"outside");
    let symlink = parent
        .0
        .join(format!("rad-update-fetch-{}-{:032x}", uid(), 17));
    std::os::unix::fs::symlink(&target, &symlink).unwrap();
    assert_eq!(
        cleanup::collect_in(&parent.0, uid(), &dead)
            .unwrap()
            .removed,
        0
    );
    for path in [
        legacy,
        unknown,
        linked,
        hardlinked,
        insecure,
        unsafe_marker,
        wrong_marker,
    ] {
        assert!(path.exists());
    }
    assert_eq!(fs::read(outside).unwrap(), b"must not be touched");
    assert_eq!(fs::read(target.join("response")).unwrap(), b"outside");
}
#[test]
fn scanning_is_bounded_and_never_crosses_into_another_uid_or_unknown_namespace() {
    struct PanicLiveness;
    impl Liveness for PanicLiveness {
        fn state(&self, _: u32) -> ProcessState {
            panic!("unrelated namespace must be filtered before inspecting ownership");
        }
    }
    let unrelated = Parent::new();
    for name in [
        format!("rad-update-fetch-{}-{:032x}", uid() + 1, 1),
        format!("rad-update-fetch-{}-legacy", uid()),
        "operation.json".into(),
        "Store".into(),
    ] {
        file(&unrelated.0.join(name), b"unrelated");
    }
    assert_eq!(
        cleanup::collect_in(&unrelated.0, uid(), &PanicLiveness)
            .unwrap()
            .inspected,
        0
    );
    assert!(cleanup::collect_in(&unrelated.0, uid() + 1, &PanicLiveness).is_err());
    let parent = Parent::new();
    for n in 0..80 {
        parent.directory("fetch", 100 + n, 45678);
    }
    let report = cleanup::collect_in(&parent.0, uid(), &State(ProcessState::Dead)).unwrap();
    assert!(report.limit_reached);
    assert_eq!(report.inspected, 64);
    assert_eq!(report.removed, 64);
    let many = Parent::new();
    let path = many.directory("fetch", 999, 45678);
    file(&path.join("response"), b"keep");
    for n in 0..10 {
        file(&path.join(format!("unknown-{n}")), b"keep");
    }
    assert_eq!(
        cleanup::collect_in(&many.0, uid(), &State(ProcessState::Dead))
            .unwrap()
            .removed,
        0
    );
    assert_eq!(fs::read(path.join("response")).unwrap(), b"keep");
}
#[test]
fn pid_namespace_must_be_known_and_identical_before_any_pid_liveness_probe() {
    struct NoProbe;
    impl Liveness for NoProbe {
        fn state(&self, _: u32) -> ProcessState {
            panic!("namespace is not proven; PID has no meaning here");
        }
    }
    let parent = Parent::new();
    let path = parent.directory("fetch", 300, 12345);
    let mut marker: serde_json::Value =
        serde_json::from_slice(&fs::read(path.join(".owner.json")).unwrap()).unwrap();
    marker.as_object_mut().unwrap().remove("namespace");
    file(
        &path.join(".owner.json"),
        &serde_json::to_vec(&marker).unwrap(),
    );
    assert_eq!(
        cleanup::collect_in(&parent.0, uid(), &NoProbe)
            .unwrap()
            .removed,
        0
    );
    marker["namespace"] = serde_json::json!("00000000000000000000000000000000");
    file(
        &path.join(".owner.json"),
        &serde_json::to_vec(&marker).unwrap(),
    );
    assert_eq!(
        cleanup::collect_in(&parent.0, uid(), &NoProbe)
            .unwrap()
            .removed,
        0
    );
    assert!(path.exists());
}
