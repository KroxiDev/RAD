use rad_authority::{Authority, Command, Effect, plugins::PluginError};
use rad_core::{configuration::*, execution::*, identity::*};
use rad_plugin_contract::{Definition, Manifest, Schema, Version};
fn manifest(major: u32) -> Manifest {
    Manifest {
        resources: vec![],
        capabilities: rad_plugin_contract::SUPPORTED_CAPABILITIES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        settings: None,
        lifecycle: Default::default(),
        contract: 1,
        id: "fixture".into(),
        version: Version {
            major,
            minor: 0,
            patch: 0,
        },
        host_min: 1,
        host_max: 1,
        targets: vec!["x86_64-unknown-linux-gnu".into()],
        actions: vec![Definition {
            gestures: vec![rad_plugin_contract::Gesture::Press],
            response_timeout: None,
            id: "run".into(),
            label: "Run".into(),
            schema: Schema {
                version: 1,
                fields: vec![],
            },
            credential_slots: vec![],
        }],
        visuals: vec![],
        translations: Default::default(),
    }
}
fn edit(c: Candidate, e: Edit) -> Candidate {
    c.configuration()
        .prepare_edit(c.registry(), Origin::Editor, e)
        .unwrap()
}
fn fixture() -> Candidate {
    let c = Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap();
    let folder = c.configuration().data().principal;
    let c = edit(
        c,
        Edit::CreateButton {
            folder,
            position: 0,
            name: "Run".into(),
        },
    );
    let button = c.configuration().data().buttons[0].binding.identity();
    let c = edit(
        c,
        Edit::AddAction {
            button,
            kind: ActionKind::Wait(Wait::new(100).unwrap()),
        },
    );
    edit(
        c,
        Edit::AddAction {
            button,
            kind: ActionKind::External {
                type_key: ExternalType::new("fixture".into(), "run".into()).unwrap(),
                parameters: br#"{"schema_version":1,"values":{}}"#.to_vec(),
            },
        },
    )
}
fn submit(a: &mut Authority, cmd: impl Into<Command>) -> Vec<Effect> {
    let t = a.reserve(1, Origin::Editor).unwrap();
    a.submit(t, cmd)
}

fn credential_values(value: &str) -> std::sync::Arc<rad_plugin_contract::CredentialValues> {
    std::sync::Arc::new(
        rad_plugin_contract::CredentialValues::new(vec![rad_plugin_contract::CredentialValue {
            slot: "password".into(),
            value: value.into(),
        }])
        .unwrap(),
    )
}
fn credential_manifest() -> Manifest {
    let mut m = manifest(1);
    m.host_min = 1;
    m.host_max = 1;
    m.actions[0].credential_slots.push("password".into());
    m
}
#[test]
fn credentials_are_captured_before_wait_and_replacement_does_not_change_the_job() {
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(772).unwrap());
    a.install_plugin(credential_manifest(), 1, "x86_64-unknown-linux-gnu")
        .unwrap();
    a.publish_plugin_credentials("fixture", 1, 1, credential_values("old-canary"))
        .unwrap();
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Accepted { .. })));
    a.publish_plugin_credentials("fixture", 1, 2, credential_values("new-canary"))
        .unwrap();
    let dispatch = a
        .before_event(100)
        .unwrap()
        .into_iter()
        .find_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d)
            } else {
                None
            }
        })
        .unwrap();
    let captured = a
        .job_credentials(dispatch.id)
        .expect("admission captured credentials before wait");
    assert_eq!(captured.revision(), 1);
    assert_eq!(captured.values().as_slice()[0].value, "old-canary");
    assert_eq!(
        a.current_plugin_credentials("fixture", 1)
            .unwrap()
            .revision(),
        2
    );
    assert!(!format!("{captured:?}").contains("canary"));
}

#[test]
fn unknown_blocks_only_new_snapshots_and_keymissing_can_publish_captured_absence() {
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(774).unwrap());
    a.install_plugin(credential_manifest(), 1, "x86_64-unknown-linux-gnu")
        .unwrap();
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(
        matches!(effects.as_slice(), [Effect::Rejected { .. }]),
        "no authenticated map has been published yet"
    );
    let empty = std::sync::Arc::new(rad_plugin_contract::CredentialValues::new(vec![]).unwrap());
    a.publish_plugin_credentials("fixture", 1, 1, empty)
        .unwrap();
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Accepted { .. })));
    a.invalidate_plugin_credentials("fixture", 1).unwrap();
    assert!(a.current_plugin_credentials("fixture", 1).is_none());
    let dispatch = a
        .before_event(100)
        .unwrap()
        .into_iter()
        .find_map(|e| {
            if let Effect::Dispatch(d) = e {
                Some(d)
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        a.job_credentials(dispatch.id)
            .unwrap()
            .values()
            .as_slice()
            .is_empty()
    );
    a.job_complete(dispatch.id, Completion::Succeeded).unwrap();
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(matches!(effects.as_slice(), [Effect::Rejected { .. }]));
    a.publish_plugin_credentials("fixture", 1, 2, credential_values("reentered"))
        .unwrap();
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Accepted { .. })));
}

#[test]
fn trusted_credential_publication_rejects_stale_selection_revision_and_undeclared_slots() {
    let mut a = Authority::new(fixture(), 1, std::num::NonZeroU128::new(776).unwrap());
    a.install_plugin(credential_manifest(), 9, "x86_64-unknown-linux-gnu")
        .unwrap();
    let selection = a.plugin_manifests().next().unwrap().1;
    assert!(
        a.publish_plugin_credentials("missing", 9, 1, credential_values("x"))
            .is_err()
    );
    assert!(
        a.publish_plugin_credentials("fixture", 8, 1, credential_values("x"))
            .is_err()
    );
    assert!(
        a.publish_plugin_credentials("fixture", selection, 0, credential_values("x"))
            .is_err()
    );
    a.publish_plugin_credentials("fixture", selection, 1, credential_values("original"))
        .unwrap();
    assert!(
        a.publish_plugin_credentials("fixture", selection, 1, credential_values("replacement"))
            .is_err()
    );
    let other = std::sync::Arc::new(
        rad_plugin_contract::CredentialValues::new(vec![rad_plugin_contract::CredentialValue {
            slot: "foreign".into(),
            value: "x".into(),
        }])
        .unwrap(),
    );
    assert!(
        a.publish_plugin_credentials("fixture", selection, 2, other)
            .is_err()
    );
    assert_eq!(
        a.current_plugin_credentials("fixture", selection)
            .unwrap()
            .values()
            .as_slice()[0]
            .value,
        "original"
    );
    assert!(a.invalidate_plugin_credentials("fixture", 8).is_err());
    a.invalidate_plugin_credentials("fixture", selection)
        .unwrap();
    assert!(
        a.publish_plugin_credentials("fixture", selection, 1, credential_values("reused"))
            .is_err()
    );
}

#[test]
fn dormant_metadata_admits_wait_and_pins_selection_credentials_and_generation_once() {
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let root = c.configuration().data().principal;
    let c = edit(
        c,
        Edit::CreateFolder {
            parent: root,
            position: 1,
            name: "empty".into(),
        },
    );
    let empty = c.configuration().data().folders[1].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(779).unwrap());
    a.register_plugin(credential_manifest(), "x86_64-unknown-linux-gnu")
        .unwrap();
    let (metadata, selection, ready) = a.plugin_manifests().next().unwrap();
    assert_eq!(metadata, &credential_manifest());
    assert!(!ready);
    assert_eq!(a.plugin_generation("fixture"), None);
    assert!(a.plugin_available("fixture", selection));
    a.publish_plugin_credentials("fixture", selection, 1, credential_values("before"))
        .unwrap();
    assert_eq!(a.plugin_demand(), ["fixture".to_owned()].into());
    a.navigate(Origin::Editor, empty).unwrap();
    assert!(a.plugin_demand().is_empty());
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Accepted { .. })));
    assert_eq!(
        a.plugin_demand(),
        ["fixture".to_owned()].into(),
        "future step after Wait owns demand outside active folder"
    );
    assert_eq!(
        a.register_plugin(credential_manifest(), "x86_64-unknown-linux-gnu"),
        Err(PluginError::InUse)
    );
    a.publish_plugin_credentials("fixture", selection, 2, credential_values("after"))
        .unwrap();
    a.activate_plugin("fixture", selection, 71).unwrap();
    assert_eq!(a.deactivate_plugin("fixture", 71), Err(PluginError::InUse));
    a.plugin_failed("fixture", 71);
    a.activate_plugin("fixture", selection, 72).unwrap();
    let dispatch = a
        .before_event(100)
        .unwrap()
        .into_iter()
        .find_map(|e| match e {
            Effect::Dispatch(d) => Some(d),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        a.job_plugin_selection(dispatch.id),
        Some(("fixture", selection))
    );
    assert_eq!(
        a.job_plugin(dispatch.id),
        Some(("fixture", 71)),
        "failed generation is never replayed on a replacement"
    );
    assert_eq!(
        a.job_credentials(dispatch.id).unwrap().values().as_slice()[0].value,
        "before"
    );
    a.before_event(5100).unwrap();
    assert_eq!(
        a.plugin_change_allowed("fixture"),
        Err(PluginError::Residual)
    );
    assert_eq!(
        a.plugin_demand(),
        ["fixture".to_owned()].into(),
        "residual owns demand after result is terminal"
    );
    a.job_complete(dispatch.id, Completion::Succeeded).unwrap();
    assert!(a.plugin_demand().is_empty());
    a.deactivate_plugin("fixture", 72).unwrap();
    assert_eq!(a.plugin_manifests().next().unwrap().1, selection);
    assert_eq!(
        a.current_plugin_credentials("fixture", selection)
            .unwrap()
            .revision(),
        2
    );
}

#[test]
fn admission_during_normal_stop_waits_for_new_generation_without_rebinding_old_work() {
    let c = fixture();
    let button = c.configuration().data().buttons[0].binding.identity();
    let mut a = Authority::new(c, 1, std::num::NonZeroU128::new(782).unwrap());
    a.install_plugin(manifest(1), 41, "x86_64-unknown-linux-gnu")
        .unwrap();
    let selection = a.plugin_manifests().next().unwrap().1;
    a.begin_plugin_stop("fixture", 41).unwrap();
    assert_eq!(
        a.plugin_generation("fixture"),
        None,
        "retiring process cannot capture a new admission"
    );
    let effects = submit(
        &mut a,
        Command::Run {
            target: Target::Button(button),
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Accepted { .. })));
    assert_eq!(a.plugin_change_allowed("fixture"), Err(PluginError::InUse));
    let dispatch = a
        .before_event(100)
        .unwrap()
        .into_iter()
        .find_map(|e| match e {
            Effect::Dispatch(d) => Some(d),
            _ => None,
        })
        .unwrap();
    assert_eq!(a.job_plugin(dispatch.id), None);
    assert_eq!(dispatch.deadline_ms, 5100);
    // Caller has now reaped generation 41 and completed the new handshake.
    a.activate_plugin("fixture", selection, 42).unwrap();
    assert_eq!(a.job_plugin(dispatch.id), Some(("fixture", 42)));
    assert_eq!(a.begin_plugin_stop("fixture", 42), Err(PluginError::InUse));
    a.before_event(5100).unwrap();
    assert_eq!(
        a.begin_plugin_stop("fixture", 42),
        Err(PluginError::Residual)
    );
    a.job_complete(dispatch.id, Completion::Succeeded).unwrap();
    a.begin_plugin_stop("fixture", 42).unwrap();
    assert_eq!(a.plugin_manifests().next().unwrap().1, selection);
}
