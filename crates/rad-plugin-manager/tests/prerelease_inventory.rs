#[allow(dead_code)]
mod support;
use rad_plugin_manager::{CommitOutcome, Error, Manager, Operation};
use support::{Directory, TARGET};

#[test]
fn stored_prerelease_inventory_is_read_only_and_updatable_but_never_executable() {
    for manifest in [
        include_bytes!("fixtures/audio-prerelease.json").as_slice(),
        include_bytes!("fixtures/obs-prerelease.json").as_slice(),
    ] {
        let metadata: serde_json::Value = serde_json::from_slice(manifest).unwrap();
        let id = metadata["id"].as_str().unwrap();
        let dir = Directory::new();
        let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
        let package = support::package_for(id, 1);
        let mut change = Some(manager.prepare(Operation::Install, Some(&package)).unwrap());
        assert_eq!(manager.commit(&mut change).unwrap(), CommitOutcome::Applied);
        drop(manager);
        // Genuine metadata from the preceding repository version; controlled ELF fixture.
        let saved = dir.0.join("v0000000000000001/manifest.json");
        std::fs::write(&saved, manifest).unwrap();
        let index_before = std::fs::read(dir.0.join("index.json")).unwrap();
        let mut manager = Manager::open(&dir.0, 1, TARGET)
            .expect("an incompatible installed plugin must not block package administration");
        assert_eq!(
            std::fs::read(dir.0.join("index.json")).unwrap(),
            index_before
        );
        let inventory = manager.installed().unwrap();
        assert_eq!(inventory[0].manifest.id, id);
        assert!(inventory[0].manifest.validate(1, TARGET).is_err());
        assert!(matches!(manager.executable(id), Err(Error::Incompatible)));
        assert_eq!(std::fs::read(&saved).unwrap(), manifest);
        let replacement = support::package_for(id, 2);
        let mut update = Some(
            manager
                .prepare(Operation::Update(id.into()), Some(&replacement))
                .unwrap(),
        );
        assert_eq!(manager.commit(&mut update).unwrap(), CommitOutcome::Applied);
        assert!(manager.executable(id).is_ok());
        assert_eq!(
            manager.installed().unwrap()[0]
                .previous
                .as_ref()
                .unwrap()
                .version,
            serde_json::from_value(metadata["version"].clone()).unwrap()
        );
        assert_eq!(std::fs::read(&saved).unwrap(), manifest);
        drop(manager);
        assert!(Manager::open(&dir.0, 1, TARGET).is_ok());
    }
}
