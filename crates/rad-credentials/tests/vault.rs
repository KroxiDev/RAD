use rad_credentials::Vault;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "rad-vault-{}-{}",
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
#[test]
fn installation_creates_exact_private_key_and_preserves_it_on_restart() {
    let f = Fixture::new();
    let v = Vault::open(&f.0).expect("initial vault");
    let key = fs::read(f.0.join("credentials/master.key")).unwrap();
    assert_eq!(key.len(), 40);
    assert_eq!(&key[..8], b"RADK\0\0\0\x01");
    assert_eq!(
        fs::metadata(f.0.join("credentials/master.key"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(v);
    let _v = Vault::open(&f.0).unwrap();
    assert_eq!(fs::read(f.0.join("credentials/master.key")).unwrap(), key);
}
#[test]
fn authenticated_roundtrip_empty_value_random_nonce_and_declared_only() {
    use rad_credentials::{Secret, Status};
    let f = Fixture::new();
    let mut v = Vault::open(&f.0).unwrap();
    v.replace(
        "plugin",
        "password",
        Secret::new("CANARY-vault".into()).unwrap(),
    )
    .unwrap();
    assert_eq!(v.status("plugin", "password"), Ok(Status::Present));
    let a = fs::read(f.0.join("credentials/plugin/password.radc")).unwrap();
    assert_eq!(&a[..8], b"RADC\0\0\0\x01");
    assert_eq!(a.len(), 60);
    assert!(!a.windows(12).any(|b| b == b"CANARY-vault"));
    v.replace(
        "plugin",
        "password",
        Secret::new("CANARY-vault".into()).unwrap(),
    )
    .unwrap();
    let b = fs::read(f.0.join("credentials/plugin/password.radc")).unwrap();
    assert_ne!(&a[8..32], &b[8..32]);
    v.replace("plugin", "empty", Secret::new("".into()).unwrap())
        .unwrap();
    assert_eq!(v.status("plugin", "empty"), Ok(Status::Present));
    let map = v.resolve_declared("plugin", &["password"]).unwrap();
    let values: Vec<_> = map.iter().collect();
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].1.expose(), "CANARY-vault");
    assert!(!format!("{map:?}").contains("CANARY"));
    drop(v);
    let v = Vault::open(&f.0).unwrap();
    assert_eq!(v.status("plugin", "password"), Ok(Status::Present));
}
#[test]
fn quota_counts_retained_slots_and_corrupt_sibling_blocks_until_explicit_repair() {
    use rad_credentials::{Error, Secret};
    let f = Fixture::new();
    let mut v = Vault::open(&f.0).unwrap();
    for s in ["a", "b"] {
        v.replace("p", s, Secret::new("x".repeat(4096)).unwrap())
            .unwrap();
    }
    assert_eq!(
        v.replace("p", "c", Secret::new("x".into()).unwrap()),
        Err(Error::Capacity)
    );
    v.replace("p", "a", Secret::new("".into()).unwrap())
        .unwrap();
    v.replace("p", "c", Secret::new("x".into()).unwrap())
        .unwrap();
    fs::write(f.0.join("credentials/p/b.radc"), b"bad").unwrap();
    assert_eq!(
        v.replace("p", "c", Secret::new("new".into()).unwrap()),
        Err(Error::Unavailable)
    );
    v.replace("p", "b", Secret::new("repaired".into()).unwrap())
        .unwrap();
    v.remove("p", "b").unwrap();
    assert!(!f.0.join("credentials/p/b.radc").exists());
    for i in 0..30 {
        v.replace("p", &format!("s{i}"), Secret::new("".into()).unwrap())
            .unwrap();
    }
    assert_eq!(
        v.replace("p", "extra", Secret::new("".into()).unwrap()),
        Err(Error::Capacity)
    );
}
#[test]
fn real_key_loss_preserves_ciphertexts_until_explicit_individual_reentry() {
    use rad_credentials::{Secret, State, Status};
    let f = Fixture::new();
    let mut v = Vault::open(&f.0).unwrap();
    for s in ["a", "b"] {
        v.replace("p", s, Secret::new("old".into()).unwrap())
            .unwrap();
    }
    let old = fs::read(f.0.join("credentials/p/b.radc")).unwrap();
    drop(v);
    fs::remove_file(f.0.join("credentials/master.key")).unwrap();
    let mut v = Vault::open(&f.0).unwrap();
    assert_eq!(v.state(), State::KeyMissing);
    assert_eq!(v.status("p", "a"), Ok(Status::Unavailable));
    assert!(!f.0.join("credentials/master.key").exists());
    assert_eq!(
        v.resolve_declared("p", &["a", "b"]).unwrap().iter().count(),
        0
    );
    v.replace("p", "a", Secret::new("new".into()).unwrap())
        .unwrap();
    assert_eq!(v.state(), State::Ready);
    assert_eq!(v.status("p", "a"), Ok(Status::Present));
    assert_eq!(v.status("p", "b"), Ok(Status::Unavailable));
    assert_eq!(fs::read(f.0.join("credentials/p/b.radc")).unwrap(), old);
}
#[test]
fn duplicate_missing_declarations_are_rejected() {
    use rad_credentials::Error;
    let f = Fixture::new();
    let v = Vault::open(&f.0).unwrap();
    assert!(matches!(
        v.resolve_declared("p", &["missing", "missing"]),
        Err(Error::InvalidInput)
    ));
}
#[test]
fn tampering_transplant_truncation_and_wrong_key_fail_closed() {
    use rad_credentials::{Secret, Status};
    let f = Fixture::new();
    let mut v = Vault::open(&f.0).unwrap();
    v.replace("p", "s", Secret::new("sensitive".into()).unwrap())
        .unwrap();
    let p = f.0.join("credentials/p/s.radc");
    let original = fs::read(&p).unwrap();
    for (plugin, slot) in [("p", "other"), ("other", "s")] {
        v.replace(plugin, slot, Secret::new("placeholder".into()).unwrap())
            .unwrap();
        fs::write(
            f.0.join(format!("credentials/{plugin}/{slot}.radc")),
            &original,
        )
        .unwrap();
        assert_eq!(v.status(plugin, slot), Ok(Status::Unavailable));
    }
    for bytes in [original[..20].to_vec(), {
        let mut b = original.clone();
        b[32] ^= 1;
        b
    }] {
        fs::write(&p, bytes).unwrap();
        assert_eq!(v.status("p", "s"), Ok(Status::Unavailable));
    }
    fs::write(&p, &original).unwrap();
    let k = f.0.join("credentials/master.key");
    let mut key = fs::read(&k).unwrap();
    key[8] ^= 1;
    fs::write(k, key).unwrap();
    assert_eq!(v.status("p", "s"), Ok(Status::Unavailable));
    assert_eq!(v.resolve_declared("p", &["s"]).unwrap().iter().count(), 0);
}
#[test]
fn unsafe_paths_links_permissions_and_second_owner_are_rejected() {
    use rad_credentials::{Error, Secret};
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let mut v = Vault::open(&f.0).unwrap();
    assert!(matches!(Vault::open(&f.0), Err(Error::Busy)));
    assert_eq!(
        v.replace("../escape", "s", Secret::new("x".into()).unwrap()),
        Err(Error::InvalidInput)
    );
    v.replace("p", "s", Secret::new("x".into()).unwrap())
        .unwrap();
    let slot = f.0.join("credentials/p/s.radc");
    fs::hard_link(&slot, f.0.join("copy")).unwrap();
    assert_eq!(v.status("p", "s"), Err(Error::UnsafePath));
    assert_eq!(v.remove("p", "s"), Err(Error::UnsafePath));
    fs::remove_file(f.0.join("copy")).unwrap();
    fs::set_permissions(&slot, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        v.replace("p", "s", Secret::new("x".into()).unwrap()),
        Err(Error::UnsafePath)
    );
    fs::remove_file(&slot).unwrap();
    symlink(f.0.join("credentials/master.key"), &slot).unwrap();
    assert_eq!(v.status("p", "s"), Err(Error::UnsafePath));
    assert_eq!(v.remove("p", "s"), Err(Error::UnsafePath));
    drop(v);
    fs::remove_file(f.0.join("credentials/master.key")).unwrap();
    symlink(&slot, f.0.join("credentials/master.key")).unwrap();
    assert!(matches!(Vault::open(&f.0), Err(Error::UnsafePath)));
}
#[test]
fn exact_aad_and_utf8_are_interoperable_with_library_and_invalid_key_is_preserved() {
    use chacha20poly1305::{
        KeyInit, XChaCha20Poly1305, XNonce,
        aead::{Aead, Payload},
    };
    use rad_credentials::{Error, Secret};
    let f = Fixture::new();
    let mut v = Vault::open(&f.0).unwrap();
    v.replace("p", "s", Secret::new("🔑\n\0".into()).unwrap())
        .unwrap();
    let key = fs::read(f.0.join("credentials/master.key")).unwrap();
    let slot = fs::read(f.0.join("credentials/p/s.radc")).unwrap();
    let cipher = XChaCha20Poly1305::new_from_slice(&key[8..]).unwrap();
    let plaintext = cipher
        .decrypt(
            XNonce::from_slice(&slot[8..32]),
            Payload {
                msg: &slot[32..],
                aad: b"RAD credentials v1\0\0\0\x01\0\x01p\0\x01s",
            },
        )
        .unwrap();
    assert_eq!(plaintext, "🔑\n\0".as_bytes());
    drop(v);
    let path = f.0.join("credentials/master.key");
    let mut invalid = key;
    invalid.push(0);
    fs::write(&path, &invalid).unwrap();
    assert!(matches!(Vault::open(&f.0), Err(Error::Unavailable)));
    assert_eq!(fs::read(&path).unwrap(), invalid);
    assert_eq!(fs::read(f.0.join("credentials/p/s.radc")).unwrap(), slot);
}

#[test]
fn reconcile_rejects_malformed_key_preserving_unknown_and_all_bytes() {
    use rad_credentials::{Error, Secret, State};
    let f = Fixture::new();
    let mut v = Vault::open(&f.0).unwrap();
    v.replace("p", "s", Secret::new("retained".into()).unwrap())
        .unwrap();
    let key_path = f.0.join("credentials/master.key");
    let valid_key = fs::read(&key_path).unwrap();
    let slot_path = f.0.join("credentials/p/s.radc");
    let slot = fs::read(&slot_path).unwrap();
    for malformed in [b"RADKbad".to_vec(), {
        let mut k = valid_key.clone();
        k[7] = 2;
        k
    }] {
        fs::write(&key_path, &malformed).unwrap();
        assert_eq!(v.reconcile(), Err(Error::Unavailable));
        assert_eq!(v.state(), State::Unknown);
        assert!(matches!(
            v.resolve_declared("p", &["s"]),
            Err(Error::Uncertain)
        ));
        assert_eq!(fs::read(&key_path).unwrap(), malformed);
        assert_eq!(fs::read(&slot_path).unwrap(), slot);
    }
    fs::write(&key_path, valid_key).unwrap();
    v.reconcile().unwrap();
    assert_eq!(v.state(), State::Ready);
    // No slot authentication can incidentally detect a bad key in an empty vault.
    let empty = Fixture::new();
    let mut vault = Vault::open(&empty.0).unwrap();
    let path = empty.0.join("credentials/master.key");
    let original = fs::read(&path).unwrap();
    fs::write(&path, b"RADKbad").unwrap();
    assert_eq!(vault.reconcile(), Err(Error::Unavailable));
    assert_eq!(vault.state(), State::Unknown);
    assert_eq!(fs::read(&path).unwrap(), b"RADKbad");
    fs::write(&path, original).unwrap();
    vault.reconcile().unwrap();
    assert_eq!(vault.state(), State::Ready);
}

#[test]
fn multiple_corrupt_slots_can_be_explicitly_removed_without_ignoring_quota() {
    use rad_credentials::{Error, Secret, Status};
    let f = Fixture::new();
    let mut v = Vault::open(&f.0).unwrap();
    for slot in ["a", "b", "retained"] {
        v.replace("p", slot, Secret::new("old".into()).unwrap())
            .unwrap();
    }
    let a = f.0.join("credentials/p/a.radc");
    let b = f.0.join("credentials/p/b.radc");
    fs::write(&a, b"bad-a").unwrap();
    fs::write(&b, b"bad-b").unwrap();
    assert_eq!(
        v.replace("p", "a", Secret::new("replacement".into()).unwrap()),
        Err(Error::Unavailable)
    );
    v.remove("p", "a").unwrap();
    assert!(!a.exists());
    assert_eq!(fs::read(&b).unwrap(), b"bad-b");
    assert_eq!(v.status("p", "b"), Ok(Status::Unavailable));
    assert_eq!(
        v.replace("p", "a", Secret::new("replacement".into()).unwrap()),
        Err(Error::Unavailable)
    );
    v.remove("p", "b").unwrap();
    assert!(!b.exists());
    v.replace("p", "a", Secret::new("x".repeat(4096)).unwrap())
        .unwrap();
    assert_eq!(
        v.replace("p", "b", Secret::new("x".repeat(4096)).unwrap()),
        Err(Error::Capacity)
    );
    assert_eq!(v.status("p", "retained"), Ok(Status::Present));
}
