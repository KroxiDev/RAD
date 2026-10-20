mod support;
use rad_plugin_manager::{Manager, ManagerState};
use support::{Directory, TARGET};

#[test]
fn creates_and_reopens_empty_private_store_without_overwriting() {
    let dir = Directory::new();
    let manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    assert_eq!(manager.state(), ManagerState::Ready);
    assert!(manager.installed().unwrap().is_empty());
    assert!(
        Manager::open(&dir.0, 1, TARGET).is_err(),
        "exclusive owner while open"
    );
    drop(manager);
    assert!(
        Manager::create(&dir.0, 1, TARGET).is_err(),
        "create never resets existing data"
    );
    assert!(
        Manager::open(&dir.0, 1, TARGET)
            .unwrap()
            .installed()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn prepared_install_does_not_publish_until_durable_commit_and_survives_reopen() {
    use rad_plugin_manager::{CommitOutcome, Operation};
    let dir = Directory::new();
    let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    let bytes = support::package(1);
    let mut change = Some(manager.prepare(Operation::Install, Some(&bytes)).unwrap());
    assert_eq!(change.as_ref().unwrap().plugin_id(), "org.rad.fixture");
    assert_eq!(
        change.as_ref().unwrap().manifest().unwrap().version.major,
        1
    );
    assert!(manager.installed().unwrap().is_empty());
    assert_eq!(manager.commit(&mut change).unwrap(), CommitOutcome::Applied);
    assert!(change.is_none());
    assert_eq!(manager.installed().unwrap()[0].manifest.version.major, 1);
    drop(manager);
    let manager = Manager::open(&dir.0, 1, TARGET).unwrap();
    assert_eq!(manager.installed().unwrap()[0].manifest.version.major, 1);
    use std::io::Read;
    let mut executable = Vec::new();
    manager
        .executable("org.rad.fixture")
        .unwrap()
        .read_to_end(&mut executable)
        .unwrap();
    assert_eq!(
        executable,
        std::fs::read(std::env::current_exe().unwrap()).unwrap()
    );
}

#[test]
fn interrupted_prepare_recovers_without_publishing_or_leaving_residues() {
    use rad_plugin_manager::Operation;
    let dir = Directory::new();
    let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    let bytes = support::package(1);
    let _abandoned = manager.prepare(Operation::Install, Some(&bytes)).unwrap();
    drop(manager);
    let mut manager = Manager::open(&dir.0, 1, TARGET).unwrap();
    assert!(manager.installed().unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(&dir.0).unwrap().count(),
        1,
        "startup removes only own abandoned staging"
    );
    assert!(manager.prepare(Operation::Install, Some(&bytes)).is_ok());
}

#[test]
fn update_rollback_reinstall_uninstall_retain_only_one_previous() {
    use rad_plugin_manager::{CommitOutcome, Operation};
    let dir = Directory::new();
    let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    let id = "org.rad.fixture".to_string();
    for (op, version, previous) in [
        (Operation::Install, 1, None),
        (Operation::Update(id.clone()), 2, Some(1)),
        (Operation::Rollback(id.clone()), 1, Some(2)),
        // Reinstalling the selected version repairs it; the real previous one survives.
        (Operation::Reinstall(id.clone()), 1, Some(2)),
    ] {
        let bytes = support::package(version);
        let package = if matches!(op, Operation::Rollback(_)) {
            None
        } else {
            Some(bytes.as_slice())
        };
        let mut change = Some(manager.prepare(op, package).unwrap());
        assert_eq!(manager.commit(&mut change).unwrap(), CommitOutcome::Applied);
        let installed = manager.installed().unwrap();
        assert_eq!(installed[0].manifest.version.major, version);
        assert_eq!(
            installed[0].previous.as_ref().map(|m| m.version.major),
            previous
        );
        assert!(std::fs::read_dir(&dir.0).unwrap().count() <= 3);
    }
    let mut change = Some(manager.prepare(Operation::Uninstall(id), None).unwrap());
    assert_eq!(manager.commit(&mut change).unwrap(), CommitOutcome::Applied);
    assert!(manager.installed().unwrap().is_empty());
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
}

#[test]
fn replacing_the_selected_version_with_itself_never_evicts_the_previous_version() {
    use rad_plugin_manager::{CommitOutcome, Error, Operation};
    let dir = Directory::new();
    let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    let id = "org.rad.fixture".to_string();
    for (op, version) in [(Operation::Install, 1), (Operation::Update(id.clone()), 2)] {
        let mut change = Some(
            manager
                .prepare(op, Some(&support::package(version)))
                .unwrap(),
        );
        assert_eq!(manager.commit(&mut change).unwrap(), CommitOutcome::Applied);
    }
    let index = std::fs::read(dir.0.join("index.json")).unwrap();
    assert_eq!(
        manager
            .prepare(Operation::Update(id.clone()), Some(&support::package(2)))
            .err(),
        Some(Error::SameVersion)
    );
    assert_eq!(manager.state(), ManagerState::Ready);
    assert_eq!(std::fs::read(dir.0.join("index.json")).unwrap(), index);
    // An interrupted same-version reinstall must reopen cleanly, still on v2 over v1.
    let _abandoned = manager
        .prepare(Operation::Reinstall(id.clone()), Some(&support::package(2)))
        .unwrap();
    drop(manager);
    let mut manager = Manager::open(&dir.0, 1, TARGET).unwrap();
    assert_eq!(std::fs::read(dir.0.join("index.json")).unwrap(), index);
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 3);
    let mut change = Some(
        manager
            .prepare(Operation::Reinstall(id.clone()), Some(&support::package(2)))
            .unwrap(),
    );
    assert_eq!(manager.commit(&mut change).unwrap(), CommitOutcome::Applied);
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 3);
    drop(manager);
    let mut manager = Manager::open(&dir.0, 1, TARGET).unwrap();
    let mut change = Some(manager.prepare(Operation::Rollback(id), None).unwrap());
    assert_eq!(manager.commit(&mut change).unwrap(), CommitOutcome::Applied);
    let installed = manager.installed().unwrap();
    assert_eq!(installed[0].manifest.version.major, 1);
    assert_eq!(
        installed[0].previous.as_ref().map(|m| m.version.major),
        Some(2)
    );
}

#[test]
fn foreign_and_stale_tokens_cannot_commit_and_abort_keeps_active_data() {
    use rad_plugin_manager::{CommitOutcome, Error, Operation};
    let a = Directory::new();
    let b = Directory::new();
    let mut a = Manager::create(&a.0, 1, TARGET).unwrap();
    let mut b = Manager::create(&b.0, 1, TARGET).unwrap();
    let bytes = support::package(1);
    let mut first = Some(a.prepare(Operation::Install, Some(&bytes)).unwrap());
    let key = first.as_ref().unwrap().key();
    assert_eq!(b.commit(&mut first), Err(Error::WrongOwner));
    let mut own = Some(b.prepare(Operation::Install, Some(&bytes)).unwrap());
    assert_eq!(
        b.commit_recovering(&mut first),
        CommitOutcome::Uncertain(Error::WrongOwner)
    );
    assert_eq!(
        b.state(),
        ManagerState::Prepared,
        "foreign token cannot reconcile own pending work"
    );
    b.abort(&mut own).unwrap();
    assert!(first.is_some());
    a.abort(&mut first).unwrap();
    assert!(first.is_none());
    let mut next = Some(a.prepare(Operation::Install, Some(&bytes)).unwrap());
    assert!(key != next.as_ref().unwrap().key());
    assert_eq!(a.commit(&mut next).unwrap(), CommitOutcome::Applied);
    let mut update = Some(
        a.prepare(
            Operation::Update("org.rad.fixture".into()),
            Some(&support::package(2)),
        )
        .unwrap(),
    );
    a.abort(&mut update).unwrap();
    assert_eq!(a.installed().unwrap()[0].manifest.version.major, 1);
}

#[test]
fn corrupt_journal_never_authorizes_deleting_preexisting_slot() {
    use rad_plugin_manager::Operation;
    let dir = Directory::new();
    let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    let bytes = support::package(1);
    let _change = manager.prepare(Operation::Install, Some(&bytes)).unwrap();
    drop(manager);
    let path = dir.0.join("journal.json"); // Build original canonical JSON, changing only the numeric slot.
    let original = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
    std::fs::write(&path, original.replace("\"new_slot\":1", "\"new_slot\":99")).unwrap();
    let foreign = dir.0.join("v0000000000000063");
    std::fs::create_dir(&foreign).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&foreign, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(foreign.join("plugin"), b"preexisting").unwrap();
    std::fs::set_permissions(
        foreign.join("plugin"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert!(Manager::open(&dir.0, 1, TARGET).is_err());
    assert_eq!(
        std::fs::read(foreign.join("plugin")).unwrap(),
        b"preexisting"
    );
}

#[test]
fn collisions_and_symlinks_preserve_preexisting_data_and_never_escape() {
    use rad_plugin_manager::Operation;
    use std::os::unix::fs::symlink;
    let bytes = support::package(1);
    for name in ["index.next", "journal.json", "v0000000000000001"] {
        let dir = Directory::new();
        let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
        let outside = Directory::new();
        std::fs::write(outside.0.join("sentinel"), b"keep").unwrap();
        symlink(outside.0.join("sentinel"), dir.0.join(name)).unwrap();
        assert!(
            manager.prepare(Operation::Install, Some(&bytes)).is_err(),
            "{name}"
        );
        assert!(
            std::fs::symlink_metadata(dir.0.join(name))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(outside.0.join("sentinel")).unwrap(), b"keep");
        assert!(manager.installed().unwrap().is_empty());
    }
}

#[test]
fn reinstall_after_uninstall_accepts_same_identity_without_existing_binary() {
    use rad_plugin_manager::{CommitOutcome, Operation};
    let dir = Directory::new();
    let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    let bytes = support::package(1);
    let mut install = Some(manager.prepare(Operation::Install, Some(&bytes)).unwrap());
    manager.commit(&mut install).unwrap();
    let mut uninstall = Some(
        manager
            .prepare(Operation::Uninstall("org.rad.fixture".into()), None)
            .unwrap(),
    );
    manager.commit(&mut uninstall).unwrap();
    let mut reinstall = Some(
        manager
            .prepare(Operation::Reinstall("org.rad.fixture".into()), Some(&bytes))
            .unwrap(),
    );
    assert_eq!(
        manager.commit(&mut reinstall).unwrap(),
        CommitOutcome::Applied
    );
    assert_eq!(manager.installed().unwrap()[0].manifest.version.major, 1);
    assert!(manager.installed().unwrap()[0].previous.is_none());
}

#[test]
fn incompatible_or_wrong_identity_packages_leave_active_and_previous_bytes_unchanged() {
    use rad_plugin_manager::Operation;
    let dir = Directory::new();
    let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    let original = support::package(1);
    let mut install = Some(
        manager
            .prepare(Operation::Install, Some(&original))
            .unwrap(),
    );
    manager.commit(&mut install).unwrap();
    let index = std::fs::read(dir.0.join("index.json")).unwrap();
    for (field, value) in [
        ("host_min", serde_json::json!(2)),
        ("id", serde_json::json!("org.other")),
        ("targets", serde_json::json!(["aarch64-unknown-linux-gnu"])),
    ] {
        let len = u32::from_be_bytes(original[8..12].try_into().unwrap()) as usize;
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&original[20..20 + len]).unwrap();
        manifest[field] = value;
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let mut bytes = original[..20].to_vec();
        bytes[8..12].copy_from_slice(&(manifest.len() as u32).to_be_bytes());
        bytes.extend(manifest);
        bytes.extend(&original[20 + len..]);
        assert!(
            manager
                .prepare(Operation::Update("org.rad.fixture".into()), Some(&bytes))
                .is_err()
        );
        assert_eq!(std::fs::read(dir.0.join("index.json")).unwrap(), index);
        assert_eq!(manager.installed().unwrap()[0].manifest.version.major, 1);
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 2);
    }
}

#[test]
fn ordinary_rejections_name_their_cause_and_keep_the_manager_ready() {
    use rad_plugin_manager::{Error, Operation};
    let dir = Directory::new();
    let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    let bytes = support::package(1);
    let mut install = Some(manager.prepare(Operation::Install, Some(&bytes)).unwrap());
    manager.commit(&mut install).unwrap();
    let index = std::fs::read(dir.0.join("index.json")).unwrap();
    assert_eq!(
        manager.prepare(Operation::Install, Some(&bytes)).err(),
        Some(Error::AlreadyInstalled)
    );
    let foreign = support::package_for("org.rad.other", 2);
    for operation in [
        Operation::Update("org.rad.fixture".into()),
        Operation::Reinstall("org.rad.fixture".into()),
    ] {
        assert_eq!(
            manager.prepare(operation, Some(&foreign)).err(),
            Some(Error::PluginMismatch)
        );
    }
    assert_eq!(manager.state(), ManagerState::Ready);
    assert_eq!(std::fs::read(dir.0.join("index.json")).unwrap(), index);
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 2);
}

#[test]
fn unsafe_ancestors_root_links_and_hardlinked_payloads_are_rejected() {
    use rad_plugin_manager::Operation;
    use std::os::unix::fs::{PermissionsExt, symlink};
    let parent = Directory::new();
    let real = parent.0.join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&real, parent.0.join("link")).unwrap();
    assert!(Manager::create(&parent.0.join("link"), 1, TARGET).is_err());
    std::fs::set_permissions(&parent.0, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(Manager::create(&real, 1, TARGET).is_err());
    std::fs::set_permissions(&parent.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut manager = Manager::create(&real, 1, TARGET).unwrap();
    let mut change = Some(
        manager
            .prepare(Operation::Install, Some(&support::package(1)))
            .unwrap(),
    );
    manager.commit(&mut change).unwrap();
    drop(manager);
    let binary = real.join("v0000000000000001/plugin");
    std::fs::hard_link(&binary, parent.0.join("hardlink")).unwrap();
    assert!(Manager::open(&real, 1, TARGET).is_err());
    assert!(binary.exists());
}

#[test]
fn commit_rechecks_selector_and_does_not_replace_unexpected_data() {
    use rad_plugin_manager::Operation;
    let dir = Directory::new();
    let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    let mut change = Some(
        manager
            .prepare(Operation::Install, Some(&support::package(1)))
            .unwrap(),
    );
    std::fs::write(dir.0.join("index.json"), b"unexpected data").unwrap();
    assert!(manager.commit(&mut change).is_err());
    assert_eq!(
        std::fs::read(dir.0.join("index.json")).unwrap(),
        b"unexpected data"
    );
    assert!(change.is_some());
}

#[test]
fn unexpected_payload_children_are_preserved_and_rejected_on_startup() {
    use rad_plugin_manager::Operation;
    let dir = Directory::new();
    let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    let mut change = Some(
        manager
            .prepare(Operation::Install, Some(&support::package(1)))
            .unwrap(),
    );
    manager.commit(&mut change).unwrap();
    drop(manager);
    let foreign = dir.0.join("v0000000000000001/foreign");
    std::fs::write(&foreign, b"preserve").unwrap();
    assert!(Manager::open(&dir.0, 1, TARGET).is_err());
    assert_eq!(std::fs::read(&foreign).unwrap(), b"preserve");
}

#[test]
fn incomplete_journal_next_is_owned_orphan_and_open_recovers() {
    let dir = support::Directory::new();
    drop(Manager::create(&dir.0, 1, support::TARGET).unwrap());
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir.0.join("journal.next"))
            .unwrap()
            .write_all(b"{incomplete")
            .unwrap();
    }
    let manager = Manager::open(&dir.0, 1, support::TARGET).unwrap();
    assert!(manager.installed().unwrap().is_empty());
    assert!(!dir.0.join("journal.next").exists());
}

#[test]
fn inventory_uses_validated_manifest_cache_but_reopen_revalidates_bytes() {
    use rad_plugin_manager::Operation;
    let dir = Directory::new();
    let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    let mut p = Some(
        manager
            .prepare(Operation::Install, Some(&support::package(1)))
            .unwrap(),
    );
    manager.commit(&mut p).unwrap();
    assert_eq!(manager.installed().unwrap().len(), 1);
    std::fs::write(dir.0.join("v0000000000000001/plugin"), b"bad").unwrap();
    assert_eq!(
        manager.installed().unwrap().len(),
        1,
        "inventory does not rescan executable"
    );
    drop(manager);
    assert!(
        Manager::open(&dir.0, 1, TARGET).is_err(),
        "new owner validates disk once"
    );
}

#[test]
fn confirmed_source_is_normalized_and_survives_reopen_and_rollback() {
    use rad_plugin_manager::{GithubSource, Operation};
    let dir = Directory::new();
    let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    let source = GithubSource::parse("https://github.com/owner/repo.git/releases/latest").unwrap();
    let mut p = Some(
        manager
            .prepare_from_source(
                Operation::Install,
                Some(&support::package(1)),
                Some(&source),
            )
            .unwrap(),
    );
    manager.commit(&mut p).unwrap();
    assert_eq!(
        manager.installed().unwrap()[0].source.as_deref(),
        Some("https://github.com/owner/repo")
    );
    drop(manager);
    let mut manager = Manager::open(&dir.0, 1, TARGET).unwrap();
    assert_eq!(
        manager.installed().unwrap()[0].source.as_deref(),
        Some("https://github.com/owner/repo")
    );
    let mut p = Some(
        manager
            .prepare(
                Operation::Update("org.rad.fixture".into()),
                Some(&support::package(2)),
            )
            .unwrap(),
    );
    manager.commit(&mut p).unwrap();
    let mut p = Some(
        manager
            .prepare(Operation::Rollback("org.rad.fixture".into()), None)
            .unwrap(),
    );
    manager.commit(&mut p).unwrap();
    assert_eq!(
        manager.installed().unwrap()[0].source.as_deref(),
        Some("https://github.com/owner/repo")
    );
}
