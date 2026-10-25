use rad_update::ReleaseMetadata;
fn metadata() -> ReleaseMetadata {
    ReleaseMetadata {
        schema: 1,
        product: "RAD".into(),
        package: "rust-action-deck".into(),
        version: "0.10.0".into(),
        pkgrel: 2,
        architecture: "x86_64".into(),
        file: "rust-action-deck-0.10.0-2-x86_64.pkg.tar.zst".into(),
        size: 1024,
        sha256: "a".repeat(64),
        base: "omarchy-4.0.6".into(),
        commit: "b".repeat(40),
    }
}
#[test]
fn metadata_binds_stable_version_package_revision_and_artifact_identity() {
    let m = metadata();
    assert!(m.validate("v0.10.0-2").is_ok());
    assert_eq!(m.package_version(), "0.10.0-2");
    for m in [
        ReleaseMetadata {
            version: "0.10.0-beta.1".into(),
            ..metadata()
        },
        ReleaseMetadata {
            package: "other-package".into(),
            ..metadata()
        },
        ReleaseMetadata {
            file: "../arbitrary.pkg.tar.zst".into(),
            ..metadata()
        },
        ReleaseMetadata {
            architecture: "aarch64".into(),
            ..metadata()
        },
        ReleaseMetadata {
            pkgrel: 0,
            ..metadata()
        },
        ReleaseMetadata {
            sha256: "x".repeat(64),
            ..metadata()
        },
    ] {
        assert!(m.validate("v0.10.0-2").is_err(), "{m:?}");
    }
    assert!(metadata().validate("v0.10.0-1").is_err());
}
