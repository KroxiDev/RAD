use rad_update::installer::{self, PackageIdentity, System, TransactionReport};
use rad_update::{Candidate, Error, Prepared, ReleaseMetadata};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
struct Fake {
    version: String,
    expected: Candidate,
    transactions: usize,
    official_queries: usize,
    inspected: Vec<u8>,
    success: bool,
    details: String,
    configuration: String,
}
impl System for Fake {
    fn configuration(&mut self) -> Result<String, Error> {
        Ok(self.configuration.clone())
    }
    fn installed_version(&mut self) -> Result<String, Error> {
        Ok(self.version.clone())
    }
    fn official_candidate(&mut self, installed: &str) -> Result<Option<Candidate>, Error> {
        self.official_queries += 1;
        assert_eq!(installed, "0.1.0-1");
        Ok(Some(self.expected.clone()))
    }
    fn inspect(&mut self, path: &Path) -> Result<PackageIdentity, Error> {
        self.inspected = fs::read(path).unwrap();
        Ok(PackageIdentity {
            package: "rust-action-deck".into(),
            version: "0.2.0-1".into(),
            architecture: "x86_64".into(),
        })
    }
    fn transact(&mut self, path: &Path) -> Result<TransactionReport, Error> {
        self.transactions += 1;
        assert_eq!(fs::read(path).unwrap(), b"abc");
        if self.success {
            self.version = "0.2.0-1".into();
        }
        Ok(TransactionReport {
            success: self.success,
            details: self.details.clone(),
        })
    }
}
struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        for entry in fs::read_dir(&self.0).unwrap() {
            let _ = fs::remove_file(entry.unwrap().path());
        }
        let _ = fs::remove_dir(&self.0);
    }
}
fn scenario() -> (Staging, Prepared, Fake) {
    let random = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let dir = std::env::temp_dir().join(format!("rad-update-stage-{uid}-{random:032x}"));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let candidate = Candidate {
        tag: "v0.2.0-1".into(),
        installed_version: "0.1.0-1".into(),
        metadata: ReleaseMetadata {
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
        },
    };
    fs::write(dir.join(&candidate.metadata.file), b"abc").unwrap();
    fs::set_permissions(
        dir.join(&candidate.metadata.file),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let prepared = Prepared {
        directory: dir.clone(),
        candidate: candidate.clone(),
    };
    let fake = Fake {
        version: "0.1.0-1".into(),
        expected: candidate,
        transactions: 0,
        official_queries: 0,
        inspected: vec![],
        success: true,
        details: "controlled pacman fixture".into(),
        configuration: "[options]\nRootDir = /\nDBPath = /var/lib/pacman/\n".into(),
    };
    (Staging(dir), prepared, fake)
}
#[test]
fn official_requery_and_protected_bytes_precede_transaction_and_source_substitution_has_no_effect()
{
    let (_stage, prepared, mut system) = scenario();
    // SAFETY: geteuid has no preconditions.
    let approved =
        installer::preflight_with(&prepared, unsafe { libc::geteuid() }, &mut system).unwrap();
    assert_eq!(system.official_queries, 1);
    assert_eq!(system.transactions, 0);
    assert_eq!(system.inspected, b"abc");
    fs::write(
        prepared.directory.join(&prepared.candidate.metadata.file),
        b"substituted after verification",
    )
    .unwrap();
    let outcome = installer::commit_with(approved, &mut system).unwrap();
    assert_eq!(outcome.installed_version, "0.2.0-1");
    assert_eq!(system.transactions, 1);
}
#[test]
fn transaction_config_preserves_signature_policy_and_all_checks_but_has_no_sync_repositories() {
    let input = "[options]\nRootDir = /\nDBPath = /var/lib/pacman/\nCheckSpace\nLocalFileSigLevel = PackageRequired\nLocalFileSigLevel = PackageTrustedOnly\n[core]\nServer = https://example.invalid/repo\n";
    assert_eq!(
        installer::pacman_options_only(input).unwrap(),
        "[options]\nRootDir = /\nDBPath = /var/lib/pacman/\nCheckSpace\nLocalFileSigLevel = PackageRequired\nLocalFileSigLevel = PackageTrustedOnly\n"
    );
    assert!(installer::pacman_options_only("[options]\nRootDir = /foreign\n[core]\n").is_err());
}
#[test]
fn locked_pretransaction_guard_prevents_racing_external_upgrade_or_downgrade() {
    assert!(installer::transaction_guard("0.1.0-1", "0.2.0-1", "0.1.0-1").is_ok());
    for actual in ["0.2.0-1", "0.3.0-1", "0.0.9-1"] {
        assert!(installer::transaction_guard("0.1.0-1", "0.2.0-1", actual).is_err());
    }
    assert!(installer::transaction_guard("0.2.0-1", "0.1.0-1", "0.2.0-1").is_err());
}
#[test]
fn digest_origin_symlink_and_external_changes_never_start_a_transaction() {
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let (_stage, prepared, mut system) = scenario();
    fs::write(
        prepared.directory.join(&prepared.candidate.metadata.file),
        b"abd",
    )
    .unwrap();
    assert!(installer::preflight_with(&prepared, uid, &mut system).is_err());
    assert_eq!(system.transactions, 0);
    let (_stage, prepared, mut system) = scenario();
    system.expected.metadata.sha256 = "f".repeat(64);
    assert!(installer::preflight_with(&prepared, uid, &mut system).is_err());
    assert_eq!(system.transactions, 0);
    let (_stage, prepared, mut system) = scenario();
    let original = prepared.directory.join("original");
    let package = prepared.directory.join(&prepared.candidate.metadata.file);
    fs::rename(&package, &original).unwrap();
    std::os::unix::fs::symlink(&original, &package).unwrap();
    assert!(installer::preflight_with(&prepared, uid, &mut system).is_err());
    assert_eq!(system.transactions, 0);
    let (_stage, prepared, mut system) = scenario();
    let authorized = installer::preflight_with(&prepared, uid, &mut system).unwrap();
    system.version = "0.3.0-1".into();
    assert!(installer::commit_with(authorized, &mut system).is_err());
    assert_eq!(system.transactions, 0);
    let (_stage, prepared, mut system) = scenario();
    let authorized = installer::preflight_with(&prepared, uid, &mut system).unwrap();
    system.success = false;
    assert!(installer::commit_with(authorized, &mut system).is_err());
    assert_eq!(system.transactions, 1);
}

#[test]
fn outcome_frame_bounds_encoded_bytes_without_splitting_unicode_or_losing_diagnostic_tail() {
    use rad_update::InstallOutcome;
    let short = InstallOutcome {
        installed_version: "0.2.0-1".into(),
        details: "complete diagnostic 🦀".into(),
    };
    let frame = installer::outcome_frame(&short).unwrap();
    assert_eq!(
        serde_json::from_slice::<InstallOutcome>(&frame).unwrap(),
        short
    );
    let noisy = InstallOutcome {
        installed_version: "0.2.0-1".into(),
        details: format!("HEAD: {} :TAIL", "\0\n\t\"\\é🦀".repeat(10000)),
    };
    let frame = installer::outcome_frame(&noisy).unwrap();
    assert!(
        frame.len() <= installer::MAX_OUTCOME_FRAME_BYTES,
        "encoded frame was {} bytes",
        frame.len()
    );
    assert_eq!(frame.last(), Some(&b'\n'));
    let parsed: InstallOutcome = serde_json::from_slice(&frame).unwrap();
    assert_eq!(parsed.installed_version, "0.2.0-1");
    assert!(parsed.details.starts_with("HEAD:"));
    assert!(parsed.details.ends_with(":TAIL"));
    assert!(parsed.details.contains("diagnostic output truncated"));
}
#[test]
fn noisy_successful_transaction_keeps_a_parseable_verified_outcome_instead_of_protocol_failure() {
    let (_stage, prepared, mut system) = scenario();
    system.details = format!(
        "pacman exit 0\n{}\nfinal stderr diagnostic",
        "\0\n\t\"\\✓".repeat(10000)
    );
    // SAFETY: geteuid has no preconditions.
    let authorized =
        installer::preflight_with(&prepared, unsafe { libc::geteuid() }, &mut system).unwrap();
    let outcome = installer::commit_with(authorized, &mut system).unwrap();
    let frame = installer::outcome_frame(&outcome).unwrap();
    assert!(frame.len() <= installer::MAX_OUTCOME_FRAME_BYTES);
    let received: rad_update::InstallOutcome = serde_json::from_slice(&frame).unwrap();
    assert_eq!(received.installed_version, "0.2.0-1");
    assert!(received.details.ends_with("final stderr diagnostic"));
    assert_eq!(system.transactions, 1);
}
#[test]
fn partial_install_exclusions_require_manual_update_without_weakening_pacman_configuration() {
    let baseline =
        "[options]\nRootDir = /\nDBPath = /var/lib/pacman/\nLocalFileSigLevel = PackageRequired\n";
    for exclusion in [
        "NoUpgrade = usr/lib/rad/*\n",
        "NoExtract = usr/lib/rad/rad-editor\n",
        "NoUpgrade = unrelated/path\n",
        "NoExtract = !usr/lib/rad/*\n",
    ] {
        let configured = format!("{baseline}{exclusion}");
        let error = installer::pacman_options_only(&configured)
            .expect_err("any nonempty file exclusion makes a complete upgrade unverifiable");
        assert!(error.to_string().contains("manual"));
        assert_eq!(
            configured,
            format!("{baseline}{exclusion}"),
            "configuration must not be edited"
        );
    }
    let empty = format!("{baseline}NoUpgrade =   \nNoExtract = \n");
    assert_eq!(installer::pacman_options_only(&empty).unwrap(), empty);
}

#[test]
fn file_exclusions_abort_preflight_before_release_query_or_ready_for_shutdown() {
    let (_stage, prepared, mut system) = scenario();
    system.configuration.push_str("NoExtract = usr/lib/rad/*\n");
    // SAFETY: geteuid has no preconditions.
    let result = installer::preflight_with(&prepared, unsafe { libc::geteuid() }, &mut system);
    assert!(
        result.is_err(),
        "an excluded binary must abort before READY"
    );
    assert_eq!(system.official_queries, 0);
    assert!(system.inspected.is_empty());
    assert_eq!(system.transactions, 0);
}
