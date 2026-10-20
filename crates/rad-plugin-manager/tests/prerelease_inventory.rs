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

fn open_stored(bytes: &[u8]) -> Result<Manager, Error> {
    let dir = Directory::new();
    let mut manager = Manager::create(&dir.0, 1, TARGET).unwrap();
    let package = support::package_for("org.rad.audio", 1);
    let mut change = Some(manager.prepare(Operation::Install, Some(&package)).unwrap());
    manager.commit(&mut change).unwrap();
    drop(manager);
    let saved = dir.0.join("v0000000000000001/manifest.json");
    std::fs::write(&saved, bytes).unwrap();
    let result = Manager::open(&dir.0, 1, TARGET);
    assert_eq!(std::fs::read(saved).unwrap(), bytes);
    result
}

#[test]
fn prerelease_inventory_rejects_positional_structures_without_normalizing_corruption() {
    use serde_json::{Value, json};
    let base: Value =
        serde_json::from_slice(include_bytes!("fixtures/audio-prerelease.json")).unwrap();
    let root = json!([
        "on_demand",
        base["contract"],
        base["id"],
        base["version"],
        base["host_min"],
        base["host_max"],
        base["targets"],
        base["actions"],
        base["visuals"]
    ]);
    let mut variants = vec![("root", root)];
    for (label, pointer, replacement) in [
        ("version", "/version", json!([0, 1, 0])),
        (
            "definition",
            "/actions/0",
            json!([
                ["press"],
                "output",
                "Audio output",
                base["actions"][0]["schema"],
                []
            ]),
        ),
        (
            "schema",
            "/actions/0/schema",
            json!([1, base["actions"][0]["schema"]["fields"]]),
        ),
        (
            "field",
            "/actions/0/schema/fields/0",
            json!([
                "operation",
                "Operation",
                true,
                base["actions"][0]["schema"]["fields"][0]["kind"]
            ]),
        ),
    ] {
        let mut value = base.clone();
        *value.pointer_mut(pointer).unwrap() = replacement;
        variants.push((label, value));
    }
    let accepted: Vec<_> = variants
        .into_iter()
        .filter_map(|(label, value)| {
            (!matches!(
                open_stored(&serde_json::to_vec(&value).unwrap()),
                Err(Error::CorruptState)
            ))
            .then_some(label)
        })
        .collect();
    assert!(
        accepted.is_empty(),
        "accepted corrupt structures: {accepted:?}"
    );
}

#[test]
fn prerelease_inventory_preserves_defaults_nulls_lists_and_strict_boundaries() {
    use serde_json::{Value, json};
    let base: Value =
        serde_json::from_slice(include_bytes!("fixtures/audio-prerelease.json")).unwrap();
    let mut valid = base.clone();
    valid["lifecycle"] = json!("on_demand");
    valid["actions"][0]["response_timeout"] = Value::Null;
    valid["actions"][0]["gestures"] = json!(["press"]);
    assert!(open_stored(&serde_json::to_vec(&valid).unwrap()).is_ok());
    for (pointer, replacement) in [
        ("/host_min", json!(4)),
        ("/host_max", json!(4)),
        ("/contract", json!(2)),
    ] {
        let mut value = base.clone();
        *value.pointer_mut(pointer).unwrap() = replacement;
        assert!(matches!(
            open_stored(&serde_json::to_vec(&value).unwrap()),
            Err(Error::CorruptState)
        ));
    }
    for pointer in [
        "",
        "/version",
        "/actions/0",
        "/actions/0/schema",
        "/actions/0/schema/fields/0",
    ] {
        let mut value = base.clone();
        value.pointer_mut(pointer).unwrap()["unknown"] = json!(1);
        assert!(matches!(
            open_stored(&serde_json::to_vec(&value).unwrap()),
            Err(Error::CorruptState)
        ));
    }
    let bytes = serde_json::to_string(&base).unwrap();
    for (field, replacement) in [
        ("\"contract\":1", "\"contract\":1,\"contract\":1"),
        ("\"major\":0", "\"major\":0,\"major\":0"),
        ("\"required\":true", "\"required\":true,\"required\":true"),
    ] {
        assert!(bytes.contains(field));
        assert!(matches!(
            open_stored(bytes.replacen(field, replacement, 1).as_bytes()),
            Err(Error::CorruptState)
        ));
    }
}
