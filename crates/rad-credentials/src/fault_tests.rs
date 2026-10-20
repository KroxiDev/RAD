use super::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "rad-vault-fault-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn secret(s: &str) -> Secret {
    Secret::new(s.to_owned()).unwrap()
}
#[test]
fn failed_publication_never_reports_success_and_unknown_blocks_until_reconciled() {
    for fault in [
        files::Fault::Rng,
        files::Fault::Write,
        files::Fault::PartialWrite,
        files::Fault::FileSync,
        files::Fault::Rename,
        files::Fault::DirectorySync,
    ] {
        let f = Fixture::new();
        let mut v = Vault::open(&f.0).unwrap();
        v.replace("p", "s", secret("old")).unwrap();
        files::FAULT.with(|v| v.set(Some(fault)));
        assert!(v.replace("p", "s", secret("new")).is_err());
        let after_rename = matches!(fault, files::Fault::DirectorySync);
        if matches!(fault, files::Fault::Rename | files::Fault::DirectorySync) {
            assert_eq!(v.state(), State::Unknown);
            assert!(matches!(
                v.resolve_declared("p", &["s"]),
                Err(Error::Uncertain)
            ));
            assert_eq!(
                v.replace("p", "s", secret("must-not-write")),
                Err(Error::Uncertain)
            );
            v.reconcile().unwrap();
        }
        assert_eq!(
            v.resolve_declared("p", &["s"])
                .unwrap()
                .iter()
                .next()
                .unwrap()
                .1
                .expose(),
            if after_rename { "new" } else { "old" }
        );
    }
}
#[test]
fn removal_sync_failure_is_uncertain_and_restart_reconciles_absence() {
    let f = Fixture::new();
    let mut v = Vault::open(&f.0).unwrap();
    v.replace("p", "s", secret("old")).unwrap();
    files::FAULT.with(|v| v.set(Some(files::Fault::DirectorySync)));
    assert_eq!(v.remove("p", "s"), Err(Error::Uncertain));
    assert_eq!(v.state(), State::Unknown);
    drop(v);
    let v = Vault::open(&f.0).unwrap();
    assert_eq!(v.status("p", "s"), Ok(Status::Absent));
}
#[test]
fn key_publication_uncertainty_retains_same_key_and_ciphertexts() {
    let f = Fixture::new();
    let mut v = Vault::open(&f.0).unwrap();
    v.replace("p", "s", secret("old")).unwrap();
    let ciphertext = fs::read(f.0.join("credentials/p/s.radc")).unwrap();
    drop(v);
    fs::remove_file(f.0.join("credentials/master.key")).unwrap();
    let mut v = Vault::open(&f.0).unwrap();
    files::FAULT.with(|v| v.set(Some(files::Fault::DirectorySync)));
    assert_eq!(v.replace("p", "s", secret("new")), Err(Error::Uncertain));
    assert_eq!(v.state(), State::Unknown);
    let key = fs::read(f.0.join("credentials/master.key")).unwrap();
    assert_eq!(
        fs::read(f.0.join("credentials/p/s.radc")).unwrap(),
        ciphertext
    );
    files::FAULT.with(|v| v.set(Some(files::Fault::FileSync)));
    assert!(v.reconcile().is_err());
    assert_eq!(v.state(), State::Unknown);
    v.reconcile().unwrap();
    v.replace("p", "s", secret("new")).unwrap();
    assert_eq!(fs::read(f.0.join("credentials/master.key")).unwrap(), key);
}
#[test]
fn exclusive_temp_collision_preserves_file_not_owned_by_this_attempt() {
    let f = Fixture::new();
    let mut v = Vault::open(&f.0).unwrap();
    v.replace("p", "s", secret("old")).unwrap();
    let pending =
        f.0.join("credentials/p/.pending-00000000000000000000000000000000");
    fs::write(&pending, b"other-pending-ciphertext").unwrap();
    files::TEMP_RANDOM.with(|r| r.set(Some([0; 16])));
    assert!(v.replace("p", "s", secret("new")).is_err());
    assert_eq!(fs::read(&pending).unwrap(), b"other-pending-ciphertext");
    assert_eq!(v.status("p", "s"), Ok(Status::Present));
}
#[test]
fn initial_rng_failure_creates_no_key_and_retry_uses_durable_key() {
    let f = Fixture::new();
    files::FAULT.with(|v| v.set(Some(files::Fault::Rng)));
    assert!(Vault::open(&f.0).is_err());
    assert!(!f.0.join("credentials/master.key").exists());
    let _v = Vault::open(&f.0).unwrap();
    assert_eq!(
        fs::read(f.0.join("credentials/master.key")).unwrap().len(),
        40
    );
}
