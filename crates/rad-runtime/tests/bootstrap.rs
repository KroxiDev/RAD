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
    assert!(bootstrap::open(&directory.0, NonZeroU128::new(147).unwrap(), 800_000).is_err());
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"corrupted material must survive"
    );
}
