//! Critical export-file contract; all paths and contents are synthetic.
use rad_editor::backups::ExportFile;
use std::{
    io::Write,
    path::PathBuf,
    time::{Duration, UNIX_EPOCH},
};
fn home(name: &str) -> PathBuf {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("one-click-backup")
        .join(format!(
            "{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    std::fs::create_dir_all(&path).unwrap();
    path
}
#[test]
fn automatic_exports_use_a_separate_private_folder_and_never_overwrite_a_copy() {
    use std::os::unix::fs::PermissionsExt;
    let home = home("unique");
    let at = UNIX_EPOCH + Duration::from_secs(1234567890);
    let mut first = ExportFile::automatic(&home, at).unwrap();
    assert_eq!(first.path().parent().unwrap(), home.join("RAD Backups"));
    first.write_all(b"first synthetic copy").unwrap();
    let first = first.finish().unwrap();
    let mut second = ExportFile::automatic(&home, at).unwrap();
    second.write_all(b"second synthetic copy").unwrap();
    let second = second.finish().unwrap();
    assert_ne!(first, second);
    assert_eq!(std::fs::read(&first).unwrap(), b"first synthetic copy");
    assert_eq!(std::fs::read(&second).unwrap(), b"second synthetic copy");
    assert_eq!(
        std::fs::metadata(first).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(home.join("RAD Backups"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn unfinished_exports_are_removed_and_unsafe_destinations_are_untouched() {
    use std::os::unix::{fs::PermissionsExt, fs::symlink};
    let home = home("cleanup");
    let at = UNIX_EPOCH + Duration::from_secs(2);
    let path = {
        let mut file = ExportFile::automatic(&home, at).unwrap();
        file.write_all(b"incomplete").unwrap();
        file.path().to_path_buf()
    };
    assert!(!path.exists());
    let elsewhere = home.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let linked_home = home.join("linked-home");
    std::fs::create_dir(&linked_home).unwrap();
    symlink(&elsewhere, linked_home.join("RAD Backups")).unwrap();
    assert!(ExportFile::automatic(&linked_home, at).is_err());
    assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
    let public_home = home.join("public-home");
    std::fs::create_dir(&public_home).unwrap();
    let public = public_home.join("RAD Backups");
    std::fs::create_dir(&public).unwrap();
    std::fs::set_permissions(&public, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(ExportFile::automatic(&public_home, at).is_err());
    assert_eq!(
        std::fs::metadata(public).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(ExportFile::automatic(std::path::Path::new("relative"), at).is_err());
}
