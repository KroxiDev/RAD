use rad_core::configuration::{Edit, Origin, SettingsData};
use rad_runtime::bootstrap;
use std::{num::NonZeroU128, os::unix::fs::PermissionsExt};
struct Directory(std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn first_start_creates_private_state_restart_preserves_it_and_corruption_never_becomes_empty() {
    let directory =
        Directory(std::env::temp_dir().join(format!("rad-bootstrap-{}", std::process::id())));
    let session = NonZeroU128::new(145).unwrap();
    let mut boot = bootstrap::open(&directory.0, session, 600_000)
        .expect("first startup must initialize a genuinely missing Store");
    assert!(!boot.recovered_previous);
    assert_eq!(
        std::fs::metadata(&directory.0)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(boot.authority.next_deadline(), Some(900_000));
    let loaded = boot.store.load().unwrap();
    let candidate = loaded
        .configuration
        .prepare_edit(
            &loaded.registry,
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                brightness_percent: 39,
                ..SettingsData::default()
            }),
        )
        .unwrap();
    assert!(matches!(
        boot.store.commit(&candidate),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let installation = candidate.registry().installation();
    drop(boot);
    let boot = bootstrap::open(&directory.0, NonZeroU128::new(146).unwrap(), 700_000).unwrap();
    assert_eq!(
        boot.authority
            .editor_view(Origin::Editor)
            .unwrap()
            .brightness,
        39
    );
    assert_eq!(
        boot.store.load().unwrap().registry.installation(),
        installation
    );
    drop(boot);
    let file = directory.0.join("state.sqlite3");
    std::fs::write(&file, b"corrupted material must survive").unwrap();
    let error = bootstrap::open(&directory.0, NonZeroU128::new(147).unwrap(), 800_000)
        .err()
        .expect("a damaged Store is not opened");
    assert_eq!(
        error,
        bootstrap::StartupError::Unreadable(bootstrap::Unreadable::Corrupt)
    );
    assert_eq!(
        error.diagnosis(),
        Some(bootstrap::Diagnosis {
            cause: rad_api::RecoveryCause::Unreadable,
            code: rad_api::RecoveryDetailCode::Corrupt
        })
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"corrupted material must survive"
    );
}

#[test]
fn only_unreadable_newer_and_unsafe_failures_are_recovery_causes() {
    use bootstrap::{StartupError as E, Unreadable};
    use rad_api::{RecoveryCause as C, RecoveryDetailCode as D};
    let cause = |error: E| error.diagnosis().map(|d| (d.cause, d.code));
    assert_eq!(
        cause(E::Unreadable(Unreadable::Invalid)),
        Some((C::Unreadable, D::Invalid))
    );
    assert_eq!(
        cause(E::NewerVersion),
        Some((C::NewerVersion, D::UnsupportedVersion))
    );
    assert_eq!(
        cause(E::UnsafeDirectory),
        Some((C::UnsafeFolder, D::UnsafeDirectory))
    );
    assert_eq!(
        cause(E::UnsafeDatabaseFile),
        Some((C::UnsafeFolder, D::UnsafeDatabaseFile))
    );
    for unexpected in [E::Path, E::Storage, E::Entropy] {
        assert_eq!(cause(unexpected), None, "{unexpected:?}");
    }
}

#[test]
fn start_again_keeps_the_damaged_file_and_the_identity_of_what_was_readable() {
    use rad_core::identity::{EntityKind, IdentityCounters};
    let directory =
        Directory(std::env::temp_dir().join(format!("rad-bootstrap-again-{}", std::process::id())));
    let session = NonZeroU128::new(150).unwrap();
    let file = directory.0.join("state.sqlite3");
    drop(bootstrap::open(&directory.0, session, 1).unwrap());
    std::fs::write(&file, b"not a database").unwrap();
    let peeked = rad_store::PeekedCounters {
        installation: Some(0xABCD),
        counters: IdentityCounters {
            folders: 7,
            buttons: 12,
            actions: 30,
            serial: 50,
        },
    };
    let boot =
        bootstrap::start_again(&directory.0, Some(peeked), session, 5, 1_700_000_000_000).unwrap();
    assert!(!boot.recovered_previous);
    let loaded = boot.store.load().unwrap();
    assert_eq!(loaded.registry.installation(), 0xABCD);
    let mut registry = loaded.registry.clone();
    assert_eq!(
        registry
            .allocate(EntityKind::Button)
            .unwrap()
            .short()
            .to_string(),
        "b13"
    );
    assert_eq!(
        std::fs::read(directory.0.join("state.sqlite3.damaged-1700000000000")).unwrap(),
        b"not a database"
    );
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // A second replacement with the same stamp is refused and leaves the base alone.
    drop(boot);
    let again = bootstrap::start_again(&directory.0, None, session, 6, 1_700_000_000_000);
    assert_eq!(again.err(), Some(bootstrap::ReplaceFailed::NotApplied));
    assert!(bootstrap::open(&directory.0, session, 7).is_ok());
}
