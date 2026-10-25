use rad_update::download::{validate_destination, verify_file};
use rad_update::{Cancellation, ReleaseMetadata};
use std::{fs, os::unix::fs::PermissionsExt};
#[test]
fn every_destination_requires_official_https_host_without_credentials_or_alternate_port() {
    for value in [
        "https://api.github.com/repos/KroxiDev/RAD/releases?per_page=100&page=1",
        "https://github.com/KroxiDev/RAD/releases/download/v0.2.0-1/rad-update.json",
        "https://release-assets.githubusercontent.com/github-production-release-asset/one?signature=synthetic",
    ] {
        assert!(validate_destination(value).is_ok(), "{value}");
    }
    for value in [
        "http://github.com/KroxiDev/RAD",
        "https://github.com.evil/KroxiDev/RAD",
        "https://user@github.com/KroxiDev/RAD",
        "https://github.com:444/KroxiDev/RAD",
        "https://127.0.0.1/a",
        "https://github.com/KroxiDev/RAD#fragment",
        "https://github.com/Other/Project",
        "https://api.github.com/repos/Other/Project/releases",
        "https://release-assets.githubusercontent.com/\\evil",
    ] {
        assert!(validate_destination(value).is_err(), "{value}");
    }
}
#[test]
fn digest_and_size_bind_the_complete_package_and_symlinks_are_rejected() {
    let directory = std::env::temp_dir().join(format!(
        "rad-update-critical-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let file = directory.join("package");
    fs::write(&file, b"abc").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let metadata = ReleaseMetadata {
        schema: 1,
        product: "RAD".into(),
        package: "rust-action-deck".into(),
        version: "0.2.0".into(),
        pkgrel: 1,
        architecture: "x86_64".into(),
        file: "rust-action-deck-0.2.0-1-x86_64.pkg.tar.zst".into(),
        size: 3,
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
        base: "omarchy-4.0.6".into(),
        commit: "a".repeat(40),
    };
    assert!(verify_file(&file, &metadata, &Cancellation::default()).is_ok());
    fs::write(&file, b"abd").unwrap();
    assert!(verify_file(&file, &metadata, &Cancellation::default()).is_err());
    fs::write(&file, b"abcx").unwrap();
    assert!(verify_file(&file, &metadata, &Cancellation::default()).is_err());
    let link = directory.join("link");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(verify_file(&link, &metadata, &Cancellation::default()).is_err());
    fs::remove_file(link).unwrap();
    fs::remove_file(file).unwrap();
    fs::remove_dir(directory).unwrap();
}
