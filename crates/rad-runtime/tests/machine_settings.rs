use rad_runtime::machine_settings::MachineSettings;
#[test]
fn machine_options_are_local_and_roundtrip_without_enabling_defaults() {
    let root = std::env::temp_dir().join(format!("rad-machine-settings-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    assert_eq!(
        MachineSettings::load(&root).unwrap(),
        MachineSettings::default()
    );
    let options = MachineSettings {
        use_stream_deck: true,
        start_with_session: false,
    };
    options.save(&root).unwrap();
    assert_eq!(MachineSettings::load(&root).unwrap(), options);
    assert!(!root.join("state.sqlite3").exists());
    std::fs::write(root.join("machine.json"), b"{\"version\":99}").unwrap();
    assert!(MachineSettings::load(&root).is_err());
    assert_eq!(
        std::fs::read(root.join("machine.json")).unwrap(),
        b"{\"version\":99}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn machine_settings_save_can_retry_after_publication_failure() {
    let root = std::env::temp_dir().join(format!("rad-machine-retry-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("machine.json");
    std::fs::create_dir(&path).unwrap();
    let selected = MachineSettings {
        use_stream_deck: true,
        start_with_session: false,
    };
    assert!(selected.save(&root).is_err());
    assert_eq!(
        std::fs::read_dir(&root).unwrap().count(),
        1,
        "failed save must remove its staging file"
    );
    std::fs::remove_dir(&path).unwrap();
    let retried = selected.save(&root);
    let loaded = MachineSettings::load(&root);
    std::fs::remove_dir_all(&root).unwrap();
    retried.unwrap();
    assert_eq!(loaded.unwrap(), selected);
}
