use rad_core::{configuration::Configuration, identity::IdentityRegistry};
use rad_media::{MediaEntry, MediaReadError, MediaSource};
use rad_package::*;
struct Empty;
impl MediaSource for Empty {
    fn catalog(&self) -> &[MediaEntry] {
        &[]
    }
    fn read(&mut self, _: &[u8; 32]) -> Result<rad_media::NormalizedImage, MediaReadError> {
        panic!("no media")
    }
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

fn empty_bytes() -> Vec<u8> {
    let c = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let mut b = Vec::new();
    export(
        ExportSnapshot {
            configuration: c.configuration(),
            registry: c.registry(),
            media: &mut Empty,
        },
        &mut b,
    )
    .unwrap();
    b
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
            registry: &reg,
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
            registry: c.registry(),
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
fn portable_backup_is_v2_and_restores_into_a_virgin_installation() {
    use rad_core::configuration::Origin;
    let bytes = empty_bytes();
    assert_eq!(&bytes[8..10], &2u16.to_be_bytes());
    let destination = Configuration::prepare_new(&IdentityRegistry::new(88).unwrap()).unwrap();
    let (root, area) = area();
    let prepared = prepare(&mut bytes.as_slice(), &area).unwrap();
    let (restored, media) = prepared
        .confirm(
            destination.configuration(),
            destination.registry(),
            Origin::Editor,
            true,
        )
        .unwrap();
    assert_eq!(restored.configuration().data().principal.installation(), 77);
    drop(media);
    drop(area);
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn real_v1_package_remains_portable_and_infers_present_identity_counters() {
    let bytes = include_bytes!("../../../docs/fixtures/v1/profile.radpkg");
    let (root, area) = area();
    let prepared = prepare(&mut bytes.as_slice(), &area).unwrap();
    let destination = Configuration::prepare_new(&IdentityRegistry::new(123).unwrap()).unwrap();
    let (candidate, media) = prepared
        .confirm(
            destination.configuration(),
            destination.registry(),
            rad_core::configuration::Origin::Editor,
            true,
        )
        .unwrap();
    assert_eq!(
        candidate.configuration().data().principal.installation(),
        77
    );
    assert_eq!(
        candidate.configuration().data().grid,
        rad_core::configuration::Grid::XL
    );
    drop(media);
    drop(area);
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn frozen_v2_package_imports_then_roundtrips_v3_with_portable_settings_and_counters() {
    use rad_core::identity::IdentityRecord;
    let bytes = include_bytes!("../../../docs/fixtures/v2/profile.radpkg");
    let (root, area) = area();
    let prepared = prepare(&mut bytes.as_slice(), &area).unwrap();
    let (verified, mut media) = prepared.into_verified_data().unwrap();
    assert!(!verified.data.plugin_settings.is_empty());
    assert!(!verified.data.plugin_dependencies.is_empty());
    let records =
        verified
            .data
            .folders
            .iter()
            .map(|f| f.binding)
            .chain(verified.data.buttons.iter().flat_map(|b| {
                std::iter::once(b.binding).chain(b.actions.iter().map(|a| a.binding))
            }))
            .map(|binding| IdentityRecord::new(binding, false));
    let registry = IdentityRegistry::from_records_with_counters(
        verified.identity.installation,
        records,
        verified.identity.counters,
    )
    .unwrap();
    let configuration = Configuration::from_data(verified.data, &registry).unwrap();
    assert_eq!(
        rad_codec::encode(&configuration).unwrap(),
        include_str!("../../../docs/fixtures/v2/profile.json")
            .replacen("\"version\":2", "\"version\":5", 1)
            .as_bytes()
    );
    let mut encoded = vec![];
    export(
        ExportSnapshot {
            configuration: &configuration,
            registry: &registry,
            media: &mut media,
        },
        &mut encoded,
    )
    .unwrap();
    assert_eq!(&encoded[8..10], &2u16.to_be_bytes());
    let prepared = prepare(&mut encoded.as_slice(), &area).unwrap();
    let (reimported, media_again) = prepared.into_verified_data().unwrap();
    assert_eq!(reimported.data, *configuration.data());
    assert_eq!(reimported.identity.counters, registry.counters());
    drop(media_again);
    drop(media);
    drop(area);
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn native_sleep_survives_a_portable_copy_and_restores_without_running_or_new_settings() {
    use rad_core::configuration::*;
    let c = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "Sleep".into(),
            },
        )
        .unwrap();
    let button = c.configuration().data().buttons[0].binding.identity();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::AddAction {
                button,
                kind: ActionKind::Sleep,
            },
        )
        .unwrap();
    let mut bytes = vec![];
    export(
        ExportSnapshot {
            configuration: c.configuration(),
            registry: c.registry(),
            media: &mut Empty,
        },
        &mut bytes,
    )
    .unwrap();
    let (root, area) = area();
    let prepared = prepare(&mut bytes.as_slice(), &area).unwrap();
    let target = Configuration::prepare_new(&IdentityRegistry::new(99).unwrap()).unwrap();
    let (restored, media) = prepared
        .confirm(
            target.configuration(),
            target.registry(),
            Origin::Editor,
            true,
        )
        .unwrap();
    assert_eq!(restored.configuration().data(), c.configuration().data());
    assert_eq!(
        restored.configuration().data().buttons[0].actions[0].kind,
        ActionKind::Sleep
    );
    assert!(media.catalog().is_empty());
    drop(media);
    drop(area);
    std::fs::remove_dir(root).unwrap();
}
