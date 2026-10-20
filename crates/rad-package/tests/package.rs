use rad_core::{configuration::Configuration, identity::IdentityRegistry};
use rad_package::*;
use rad_store::{MediaEntry, MediaReadError, MediaSource};
use sha2::{Digest, Sha256};
struct Empty;
impl MediaSource for Empty {
    fn catalog(&self) -> &[MediaEntry] {
        &[]
    }
    fn read(&mut self, _: &[u8; 32]) -> Result<rad_media::NormalizedImage, MediaReadError> {
        panic!("no media")
    }
}
#[test]
fn export_empty_media_is_exact_versioned_full_settings_document() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let mut bytes = Vec::new();
    let receipt = export(
        ExportSnapshot {
            configuration: c.configuration(),
            media: &mut Empty,
        },
        &mut bytes,
    )
    .unwrap();
    let doc = rad_codec::encode(c.configuration()).unwrap();
    assert_eq!(&bytes[..8], b"RADPKG\r\n");
    assert_eq!(&bytes[8..12], &[0, 1, 0, 0]);
    assert_eq!(receipt.total_len, 92 + doc.len() as u64);
    assert_eq!(bytes.len() as u64, receipt.total_len);
    assert_eq!(&bytes[60..60 + doc.len()], doc);
    assert_eq!(
        receipt.digest,
        Sha256::digest(&bytes[..bytes.len() - 32]).as_slice()
    );
    assert_eq!(&bytes[bytes.len() - 32..], receipt.digest);
    assert!(std::str::from_utf8(&doc).unwrap().contains("settings"));
}
fn area() -> (std::path::PathBuf, StagingArea) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "rad-package-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let area = StagingArea::create_new(&root).unwrap();
    (root, area)
}
#[test]
fn prepare_stages_then_confirmation_returns_candidate_without_publication() {
    use rad_core::configuration::Origin;
    let c = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let mut bytes = Vec::new();
    export(
        ExportSnapshot {
            configuration: c.configuration(),
            media: &mut Empty,
        },
        &mut bytes,
    )
    .unwrap();
    let (root, area) = area();
    let prepared = prepare(&mut bytes.as_slice(), &area).unwrap();
    assert_eq!(area.usage().unwrap(), (1, bytes.len() as u64));
    let (candidate, media) = prepared
        .confirm(c.configuration(), c.registry(), Origin::Editor, true)
        .unwrap();
    assert_eq!(candidate.configuration(), c.configuration());
    assert_eq!(candidate.registry(), c.registry());
    assert!(media.catalog().is_empty());
    drop(media);
    assert_eq!(area.usage().unwrap(), (0, 0));
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn staging_remains_anchored_when_its_directory_name_is_replaced() {
    let c = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let mut bytes = Vec::new();
    export(
        ExportSnapshot {
            configuration: c.configuration(),
            media: &mut Empty,
        },
        &mut bytes,
    )
    .unwrap();
    let (root, area) = area();
    let moved = root.with_extension("moved");
    std::fs::rename(&root, &moved).unwrap();
    std::fs::create_dir(&root).unwrap();
    let prepared = prepare(&mut bytes.as_slice(), &area).unwrap();
    let foreign_count = std::fs::read_dir(&root).unwrap().count();
    drop(prepared);
    std::fs::remove_dir(root).unwrap();
    std::fs::remove_dir(moved).unwrap();
    assert_eq!(
        foreign_count, 0,
        "New object at original name must not receive staging data"
    );
}
fn empty_bytes() -> Vec<u8> {
    let c = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let mut b = Vec::new();
    export(
        ExportSnapshot {
            configuration: c.configuration(),
            media: &mut Empty,
        },
        &mut b,
    )
    .unwrap();
    b
}
fn with_document(mut bytes: Vec<u8>, document: Vec<u8>) -> Vec<u8> {
    bytes.truncate(60);
    bytes[12..20].copy_from_slice(&(92 + document.len() as u64).to_be_bytes());
    bytes[20..24].copy_from_slice(&(document.len() as u32).to_be_bytes());
    bytes[28..60].copy_from_slice(&Sha256::digest(&document));
    bytes.extend(document);
    let hash = Sha256::digest(&bytes);
    bytes.extend_from_slice(&hash);
    bytes
}
#[test]
fn document_inclusive_limit_streams_reads_at_most_64k() {
    let bytes = empty_bytes();
    let mut doc = bytes[60..bytes.len() - 32].to_vec();
    doc.resize(1024 * 1024, b' ');
    let bytes = with_document(bytes, doc);
    struct Bounded<'a>(&'a [u8]);
    impl std::io::Read for Bounded<'_> {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            assert!(b.len() <= 65536, "oversized read {}", b.len());
            std::io::Read::read(&mut self.0, b)
        }
    }
    let (root, area) = area();
    let result = prepare(&mut Bounded(&bytes), &area);
    assert!(result.is_ok());
    drop(result);
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn malformed_headers_truncations_checksums_and_trailing_bytes_release_reservations() {
    let bytes = empty_bytes();
    let (root, area) = area();
    for end in 0..bytes.len() {
        assert!(
            prepare(&mut &bytes[..end], &area).is_err(),
            "truncation {end}"
        );
        assert_eq!(area.usage().unwrap(), (0, 0));
    }
    for offset in [
        0,
        8,
        9,
        10,
        11,
        12,
        19,
        20,
        23,
        24,
        25,
        26,
        27,
        28,
        59,
        60,
        bytes.len() - 1,
    ] {
        let mut b = bytes.clone();
        b[offset] ^= 1;
        assert!(
            prepare(&mut b.as_slice(), &area).is_err(),
            "offset {offset}"
        );
        assert_eq!(area.usage().unwrap(), (0, 0));
    }
    let mut b = bytes.clone();
    b.push(0);
    assert!(matches!(
        prepare(&mut b.as_slice(), &area),
        Err(PackageError::InvalidPackage)
    ));
    for total in [256 * 1024 * 1024, 256 * 1024 * 1024 + 1, u64::MAX] {
        let mut b = bytes.clone();
        b[12..20].copy_from_slice(&total.to_be_bytes());
        assert!(prepare(&mut b.as_slice(), &area).is_err());
    }
    for (range, value) in [
        (20..24, (1024 * 1024 + 1u32).to_be_bytes().to_vec()),
        (24..26, 513u16.to_be_bytes().to_vec()),
    ] {
        let mut b = bytes.clone();
        b[range].copy_from_slice(&value);
        assert!(matches!(
            prepare(&mut b.as_slice(), &area),
            Err(PackageError::LimitExceeded)
        ));
    }
    assert_eq!(area.usage().unwrap(), (0, 0));
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn one_byte_io_and_interrupted_calls_preserve_complete_stream() {
    struct ShortRead<'a> {
        bytes: &'a [u8],
        interrupt: bool,
    }
    impl std::io::Read for ShortRead<'_> {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let n = b.len().min(1);
            std::io::Read::read(&mut self.bytes, &mut b[..n])
        }
    }
    struct ShortWrite {
        bytes: Vec<u8>,
        interrupt: bool,
    }
    impl std::io::Write for ShortWrite {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let n = b.len().min(1);
            self.bytes.extend_from_slice(&b[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let c = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let mut out = ShortWrite {
        bytes: vec![],
        interrupt: false,
    };
    export(
        ExportSnapshot {
            configuration: c.configuration(),
            media: &mut Empty,
        },
        &mut out,
    )
    .unwrap();
    assert_eq!(out.bytes, empty_bytes());
    let (root, area) = area();
    drop(
        prepare(
            &mut ShortRead {
                bytes: &out.bytes,
                interrupt: false,
            },
            &area,
        )
        .unwrap(),
    );
    assert_eq!(area.usage().unwrap(), (0, 0));
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn staging_capacity_failed_cleanup_and_foreign_symlink_are_accounted() {
    let bytes = empty_bytes();
    let (root, area) = area();
    let one = prepare(&mut bytes.as_slice(), &area).unwrap();
    let two = prepare(&mut bytes.as_slice(), &area).unwrap();
    assert!(matches!(
        prepare(&mut bytes.as_slice(), &area),
        Err(PackageError::Capacity)
    ));
    let p = root.join("transfer-0000000000000000");
    std::fs::remove_file(&p).unwrap();
    let foreign = root.join("foreign");
    std::fs::write(&foreign, b"untouched").unwrap();
    std::os::unix::fs::symlink(&foreign, &p).unwrap();
    assert_eq!(one.discard(), Err(PackageError::UnsafeStaging));
    assert_eq!(area.usage().unwrap().0, 2);
    assert_eq!(area.reap(), Err(PackageError::UnsafeStaging));
    assert_eq!(std::fs::read(&foreign).unwrap(), b"untouched");
    drop(two);
    assert_eq!(area.usage().unwrap().0, 1);
    std::fs::remove_file(p).unwrap();
    area.reap().unwrap();
    assert_eq!(area.usage().unwrap(), (0, 0));
    std::fs::remove_file(foreign).unwrap();
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn confirm_rejects_automation_unconfirmed_and_altered_staging() {
    use rad_core::configuration::Origin;
    use std::io::{Seek, SeekFrom, Write};
    let c = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let bytes = empty_bytes();
    let (root, area) = area();
    assert!(matches!(
        prepare(&mut bytes.as_slice(), &area).unwrap().confirm(
            c.configuration(),
            c.registry(),
            Origin::Automation,
            true
        ),
        Err(PackageError::NotAllowed)
    ));
    assert!(matches!(
        prepare(&mut bytes.as_slice(), &area).unwrap().confirm(
            c.configuration(),
            c.registry(),
            Origin::Editor,
            false
        ),
        Err(PackageError::ConfirmationRequired)
    ));
    let p = prepare(&mut bytes.as_slice(), &area).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(root.join("transfer-0000000000000002"))
        .unwrap();
    f.seek(SeekFrom::Start(60)).unwrap();
    f.write_all(b"X").unwrap();
    drop(f);
    assert!(matches!(
        p.confirm(c.configuration(), c.registry(), Origin::Editor, true),
        Err(PackageError::InvalidPackage)
    ));
    assert_eq!(area.usage().unwrap(), (0, 0));
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn confirmation_uses_current_ledger_including_retired_and_old_backup_never_reuses() {
    use rad_core::{
        configuration::{Edit, Origin},
        identity::{Binding, EntityIdentity, EntityKind, ShortId},
    };
    let base = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let bytes = empty_bytes();
    let (root, area) = area();
    let prepared = prepare(&mut bytes.as_slice(), &area).unwrap();
    let added = base
        .configuration()
        .prepare_edit(
            base.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: base.configuration().data().principal,
                position: 0,
                name: "later".into(),
            },
        )
        .unwrap();
    let later = added.configuration().data().buttons[0].binding;
    let (restored, media) = prepared
        .confirm(
            added.configuration(),
            added.registry(),
            Origin::Editor,
            true,
        )
        .unwrap();
    drop(media);
    assert!(restored.configuration().data().buttons.is_empty());
    let next = restored
        .configuration()
        .prepare_edit(
            restored.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: restored.configuration().data().principal,
                position: 0,
                name: "new".into(),
            },
        )
        .unwrap();
    assert!(
        next.configuration().data().buttons[0]
            .binding
            .short()
            .number()
            > later.short().number()
    );
    // A package may be valid in isolation but conflict with a retired current short ID.
    let mut data = added.configuration().data().clone();
    data.buttons[0].binding = Binding::new(
        ShortId::new(EntityKind::Button, later.short().number()).unwrap(),
        EntityIdentity::new(88, 1).unwrap(),
    );
    data.folders[0].slots[0] = Some(data.buttons[0].binding.identity());
    let reg = IdentityRegistry::new(77)
        .unwrap()
        .restore(&[data.folders[0].binding, data.buttons[0].binding])
        .unwrap();
    let foreign = Configuration::from_data(data, &reg).unwrap();
    let mut b = Vec::new();
    export(
        ExportSnapshot {
            configuration: &foreign,
            media: &mut Empty,
        },
        &mut b,
    )
    .unwrap();
    let p = prepare(&mut b.as_slice(), &area).unwrap();
    assert!(matches!(
        p.confirm(
            restored.configuration(),
            restored.registry(),
            Origin::Editor,
            true
        ),
        Err(PackageError::IdentityCollision)
    ));
    assert_eq!(area.usage().unwrap(), (0, 0));
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn identity_maxima_restore_into_same_identity_without_narrowing() {
    use rad_core::{
        configuration::{ConfigurationData, FolderData, Origin, SettingsData},
        identity::{Binding, EntityIdentity, EntityKind, IdentityRecord, ShortId},
        permissions::Permissions,
    };
    let id = EntityIdentity::new(u128::MAX, u64::MAX).unwrap();
    let binding = Binding::new(ShortId::new(EntityKind::Folder, u64::MAX).unwrap(), id);
    let registry =
        IdentityRegistry::from_records(u128::MAX, [IdentityRecord::new(binding, false)]).unwrap();
    let config = Configuration::from_data(
        ConfigurationData {
            settings: SettingsData::default(),
            principal: id,
            global_permissions: Permissions::default(),
            folders: vec![FolderData {
                binding,
                parent: None,
                slots: vec![None; 32],
            }],
            buttons: vec![],
        },
        &registry,
    )
    .unwrap();
    let mut bytes = Vec::new();
    export(
        ExportSnapshot {
            configuration: &config,
            media: &mut Empty,
        },
        &mut bytes,
    )
    .unwrap();
    let (root, area) = area();
    let (candidate, media) = prepare(&mut bytes.as_slice(), &area)
        .unwrap()
        .confirm(&config, &registry, Origin::Editor, true)
        .unwrap();
    assert_eq!(candidate.configuration(), &config);
    assert_eq!(candidate.registry(), &registry);
    drop(media);
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn reader_failure_at_every_byte_releases_staging_without_publishing() {
    struct Fails<'a>(&'a [u8]);
    impl std::io::Read for Fails<'_> {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            if self.0.is_empty() {
                return Err(std::io::ErrorKind::Other.into());
            }
            std::io::Read::read(&mut self.0, b)
        }
    }
    let bytes = empty_bytes();
    let (root, area) = area();
    for n in 0..=bytes.len() {
        assert!(matches!(
            prepare(&mut Fails(&bytes[..n]), &area),
            Err(PackageError::Io)
        ));
        assert_eq!(area.usage().unwrap(), (0, 0));
    }
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn staging_existing_path_symlink_and_file_permissions_are_safe() {
    use std::os::unix::fs::PermissionsExt;
    let (root, area) = area();
    assert!(matches!(
        StagingArea::create_new(&root),
        Err(PackageError::UnsafeStaging)
    ));
    let alias = root.with_extension("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    assert!(matches!(
        StagingArea::create_new(&alias),
        Err(PackageError::UnsafeStaging)
    ));
    let prepared = prepare(&mut empty_bytes().as_slice(), &area).unwrap();
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(root.join("transfer-0000000000000000"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(prepared);
    std::fs::remove_file(alias).unwrap();
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn absent_plugins_opaque_parameters_and_permission_ownership_roundtrip_without_execution() {
    use rad_core::{
        configuration::{ActionKind, Edit, ExternalType, Origin},
        permissions::{LocalPermissions, Permissions},
    };
    let base = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let mut c = base
        .configuration()
        .prepare_edit(
            base.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: base.configuration().data().principal,
                position: 0,
                name: "missing plugin".into(),
            },
        )
        .unwrap();
    let button = c.configuration().data().buttons[0].binding.identity();
    c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::AddAction {
                button,
                kind: ActionKind::External {
                    type_key: ExternalType::new("not.installed".into(), "action".into()).unwrap(),
                    parameters: vec![0, 255, 128],
                },
            },
        )
        .unwrap();
    c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetButtonPermissions {
                button,
                permissions: LocalPermissions::own(Permissions::default()),
            },
        )
        .unwrap();
    let mut bytes = Vec::new();
    export(
        ExportSnapshot {
            configuration: c.configuration(),
            media: &mut Empty,
        },
        &mut bytes,
    )
    .unwrap();
    let (root, area) = area();
    let (restored, media) = prepare(&mut bytes.as_slice(), &area)
        .unwrap()
        .confirm(base.configuration(), base.registry(), Origin::Editor, true)
        .unwrap();
    assert_eq!(restored.configuration(), c.configuration());
    drop(media);
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn crash_child_leaves_prepared_staging() {
    if let Some(root) = std::env::var_os("RAD_PACKAGE_TEST_CRASH_ROOT") {
        let area = StagingArea::create_new(std::path::Path::new(&root)).unwrap();
        let _prepared = prepare(&mut empty_bytes().as_slice(), &area).unwrap();
        assert_eq!(area.usage().unwrap().0, 1);
        std::process::exit(0);
    }
}
#[test]
fn exclusive_restart_cleanup_recognizes_abandoned_files_without_replay() {
    let (root, area) = area();
    drop(area);
    std::fs::remove_dir(&root).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_child_leaves_prepared_staging"])
        .env("RAD_PACKAGE_TEST_CRASH_ROOT", &root)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    let recovered = StagingArea::recover_abandoned(&root);
    assert!(recovered.is_ok());
    assert_eq!(recovered.unwrap().usage().unwrap(), (0, 0));
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn restart_cleanup_preserves_unknown_material_and_rejects_symlink() {
    use std::os::unix::fs::PermissionsExt;
    let (root, area) = area();
    drop(area);
    let owned = root.join("transfer-0000000000000000");
    std::fs::write(&owned, b"partial").unwrap();
    std::fs::set_permissions(&owned, std::fs::Permissions::from_mode(0o600)).unwrap();
    let unknown = root.join("not-a-transfer");
    std::fs::write(&unknown, b"keep").unwrap();
    assert!(matches!(
        StagingArea::recover_abandoned(&root),
        Err(PackageError::UnsafeStaging)
    ));
    assert_eq!(std::fs::read(&owned).unwrap(), b"partial");
    std::fs::remove_file(&unknown).unwrap();
    let link = root.join("transfer-0000000000000001");
    std::os::unix::fs::symlink(&owned, &link).unwrap();
    assert!(matches!(
        StagingArea::recover_abandoned(&root),
        Err(PackageError::UnsafeStaging)
    ));
    assert_eq!(std::fs::read(&owned).unwrap(), b"partial");
    std::fs::remove_file(link).unwrap();
    let recovered = StagingArea::recover_abandoned(&root).unwrap();
    assert_eq!(recovered.usage().unwrap(), (0, 0));
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn confirmation_rechecks_reverse_identity_collision_against_current_ledger() {
    use rad_core::{
        configuration::{Edit, Origin},
        identity::{Binding, EntityKind, ShortId},
    };
    let base = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let added = base
        .configuration()
        .prepare_edit(
            base.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: base.configuration().data().principal,
                position: 0,
                name: "reserved".into(),
            },
        )
        .unwrap();
    let binding = added.configuration().data().buttons[0].binding;
    let retired = added
        .configuration()
        .prepare_edit(
            added.registry(),
            Origin::Editor,
            Edit::DeleteButton {
                button: binding.identity(),
            },
        )
        .unwrap();
    let mut data = added.configuration().data().clone();
    data.buttons[0].binding = Binding::new(
        ShortId::new(EntityKind::Button, 999).unwrap(),
        binding.identity(),
    );
    let registry = IdentityRegistry::new(77)
        .unwrap()
        .restore(&[data.folders[0].binding, data.buttons[0].binding])
        .unwrap();
    let config = Configuration::from_data(data, &registry).unwrap();
    let mut bytes = Vec::new();
    export(
        ExportSnapshot {
            configuration: &config,
            media: &mut Empty,
        },
        &mut bytes,
    )
    .unwrap();
    let (root, area) = area();
    assert!(matches!(
        prepare(&mut bytes.as_slice(), &area).unwrap().confirm(
            retired.configuration(),
            retired.registry(),
            Origin::Editor,
            true
        ),
        Err(PackageError::IdentityCollision)
    ));
    assert_eq!(area.usage().unwrap(), (0, 0));
    std::fs::remove_dir(root).unwrap();
}
