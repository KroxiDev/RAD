use rad_core::{configuration::Configuration, identity::IdentityRegistry};
use rad_store::{MediaEntry, MediaReadError, MediaSource};
struct Empty;
impl MediaSource for Empty {
    fn catalog(&self) -> &[MediaEntry] {
        &[]
    }
    fn read(&mut self, _: &[u8; 32]) -> Result<rad_media::NormalizedImage, MediaReadError> {
        panic!("empty source")
    }
}
fn root() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "rad-transfers-B-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
#[test]
fn verified_restore_data_moves_to_authority_without_stale_candidate() {
    let candidate = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let mut bytes = Vec::new();
    rad_package::export(
        rad_package::ExportSnapshot {
            configuration: candidate.configuration(),
            registry: candidate.registry(),
            media: &mut Empty,
        },
        &mut bytes,
    )
    .unwrap();
    let path = root();
    let area = rad_package::StagingArea::create_new(&path).unwrap();
    let prepared = rad_package::prepare(&mut bytes.as_slice(), &area).unwrap();
    let (data, media) = prepared.into_verified_data().unwrap();
    assert_eq!(&data.data, candidate.configuration().data());
    assert!(media.catalog().is_empty());
    assert_eq!(area.usage().unwrap().0, 1);
    media.discard().unwrap();
    assert_eq!(area.usage().unwrap(), (0, 0));
    std::fs::remove_dir(path).unwrap();
}

use rad_core::configuration::Origin;
use rad_runtime::transfers::*;
fn ctx(connection: u64, now_ms: u64) -> Context {
    Context {
        connection,
        now_ms,
        origin: Origin::Editor,
    }
}
fn fixture() -> (std::path::PathBuf, rad_store::Store, Transfers) {
    let path = root();
    std::fs::create_dir(&path).unwrap();
    let c = Configuration::prepare_new(&IdentityRegistry::new(88).unwrap()).unwrap();
    let store = rad_store::Store::create(&path.join("db"), &c).unwrap();
    let transfers = Transfers::create(&path.join("transfers"), 123).unwrap();
    (path, store, transfers)
}
fn begin(
    t: &mut Transfers,
    s: &mut rad_store::Store,
    c: Context,
    kind: Kind,
    size: Option<u64>,
) -> String {
    match t
        .execute(
            s,
            c,
            Operation::Begin {
                kind,
                size,
                export_revision: None,
                export_snapshot: None,
            },
        )
        .unwrap()
    {
        Output::Begun { transfer_id } => transfer_id,
        _ => panic!("begin"),
    }
}
#[test]
fn quotas_origin_connection_and_expiry_are_enforced_before_io() {
    let (path, mut store, mut t) = fixture();
    assert!(matches!(
        t.execute(
            &mut store,
            Context {
                origin: Origin::Automation,
                ..ctx(1, 0)
            },
            Operation::Begin {
                kind: Kind::Restore,
                size: None,
                export_revision: None,
                export_snapshot: None,
            }
        ),
        Err(Error::NotAllowed)
    ));
    let id = begin(&mut t, &mut store, ctx(1, 0), Kind::Restore, None);
    assert_eq!(t.metadata().reserved_bytes, 512 * 1024 * 1024);
    assert!(matches!(
        t.execute(
            &mut store,
            ctx(2, 0),
            Operation::Begin {
                kind: Kind::ImageImport,
                size: Some(1),
                export_revision: None,
                export_snapshot: None,
            }
        ),
        Err(Error::Capacity)
    ));
    assert!(matches!(
        t.execute(
            &mut store,
            ctx(2, 0),
            Operation::Drop {
                transfer_id: id.clone()
            }
        ),
        Err(Error::NotFound)
    ));
    t.execute(&mut store, ctx(1, 30_000), Operation::Reap)
        .unwrap();
    assert!(t.metadata().handles.is_empty());
    assert_eq!(t.take_released(), vec![id]);
    let first = begin(
        &mut t,
        &mut store,
        ctx(1, 30_001),
        Kind::ImageImport,
        Some(1),
    );
    assert!(matches!(
        t.execute(
            &mut store,
            ctx(1, 30_001),
            Operation::Begin {
                kind: Kind::ImageImport,
                size: Some(1),
                export_revision: None,
                export_snapshot: None,
            }
        ),
        Err(Error::Capacity)
    ));
    let second = begin(
        &mut t,
        &mut store,
        ctx(2, 30_001),
        Kind::ImageImport,
        Some(1),
    );
    assert!(matches!(
        t.execute(
            &mut store,
            ctx(3, 30_001),
            Operation::Begin {
                kind: Kind::ImageImport,
                size: Some(1),
                export_revision: None,
                export_snapshot: None,
            }
        ),
        Err(Error::Capacity)
    ));
    t.execute(
        &mut store,
        ctx(1, 30_002),
        Operation::Drop { transfer_id: first },
    )
    .unwrap();
    t.execute(
        &mut store,
        ctx(2, 30_002),
        Operation::Drop {
            transfer_id: second,
        },
    )
    .unwrap();
    drop(t);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240, 31, 0,
    5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
fn with_image(store: &rad_store::Store, key: String) -> rad_core::configuration::Candidate {
    use rad_core::configuration::{Appearance, Edit};
    let loaded = store.load().unwrap();
    let c = loaded
        .configuration
        .prepare_edit(
            &loaded.registry,
            Origin::Editor,
            Edit::CreateButton {
                folder: loaded.configuration.data().principal,
                position: 0,
                name: "Image".into(),
            },
        )
        .unwrap();
    c.configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetAppearance {
                button: c.configuration().data().buttons[0].binding.identity(),
                appearance: Appearance {
                    text: "Image".into(),
                    image: Some(key),
                    text_position: None,
                },
            },
        )
        .unwrap()
}
#[test]
fn image_upload_normalizes_then_single_store_commit_publishes_owned_media() {
    let (path, mut store, mut t) = fixture();
    let id = begin(
        &mut t,
        &mut store,
        ctx(1, 0),
        Kind::ImageImport,
        Some(PNG.len() as u64),
    );
    assert!(matches!(
        t.execute(
            &mut store,
            ctx(1, 1),
            Operation::Write {
                transfer_id: id.clone(),
                offset: 1,
                bytes: PNG.to_vec()
            }
        ),
        Err(Error::Invalid)
    ));
    t.execute(
        &mut store,
        ctx(1, 2),
        Operation::Write {
            transfer_id: id.clone(),
            offset: 0,
            bytes: PNG.to_vec(),
        },
    )
    .unwrap();
    let key = match t
        .execute(
            &mut store,
            ctx(1, 3),
            Operation::Finish {
                transfer_id: id.clone(),
            },
        )
        .unwrap()
    {
        Output::ImagePrepared { media_ref } => media_ref,
        _ => panic!("image finish"),
    };
    assert!(store.image(&key).is_err());
    let candidate = with_image(&store, key.clone());
    let outcome = t.commit(&mut store, &candidate, 1, &id);
    assert!(matches!(
        outcome,
        rad_store::CommitOutcome::Applied { changed: true, .. }
    ));
    assert!(t.metadata().handles[0].pending);
    t.resolved(&id, &outcome).unwrap();
    assert!(t.metadata().handles.is_empty());
    let chunk = t
        .execute(
            &mut store,
            ctx(1, 4),
            Operation::MediaRead {
                resolved_key: key.clone(),
                offset: 0,
            },
        )
        .unwrap();
    match chunk {
        Output::Chunk { bytes, eof } => {
            assert!(eof);
            assert_eq!(bytes, store.image(&key).unwrap().png_bytes());
        }
        _ => panic!("read"),
    };
    drop(t);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn export_prepare_confirm_uses_current_ledger_and_store_without_actor_file_reads() {
    let (path, mut store, mut t) = fixture();
    let image = rad_media::normalize(PNG).unwrap();
    let image_candidate = with_image(&store, rad_store::media_key(&image));
    assert!(matches!(
        store.commit_with_media(&image_candidate, &[image]),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let loaded = store.load().unwrap();
    let export = match t
        .execute(
            &mut store,
            ctx(1, 0),
            Operation::Begin {
                kind: Kind::Export,
                size: None,
                export_revision: Some(loaded.revision),
                export_snapshot: Some(image_candidate.clone()),
            },
        )
        .unwrap()
    {
        Output::Begun { transfer_id } => transfer_id,
        _ => panic!(),
    };
    assert_eq!(t.metadata().handles[0].revision, Some(loaded.revision));
    let size = match t
        .execute(
            &mut store,
            ctx(1, 1),
            Operation::Finish {
                transfer_id: export.clone(),
            },
        )
        .unwrap()
    {
        Output::ExportReady { size, .. } => size,
        _ => panic!(),
    };
    let mut bytes = Vec::new();
    loop {
        match t
            .execute(
                &mut store,
                ctx(1, 2),
                Operation::Read {
                    transfer_id: export.clone(),
                    offset: bytes.len() as u64,
                },
            )
            .unwrap()
        {
            Output::Chunk { bytes: b, eof } => {
                assert!(b.len() <= 8192);
                bytes.extend(b);
                if eof {
                    break;
                }
            }
            _ => panic!(),
        }
    }
    assert_eq!(size, bytes.len() as u64);
    assert!(t.metadata().handles.is_empty());
    assert_eq!(t.take_released(), vec![export]);
    let restore = begin(
        &mut t,
        &mut store,
        ctx(2, 3),
        Kind::Restore,
        Some(bytes.len() as u64),
    );
    for (i, b) in bytes.chunks(8192).enumerate() {
        t.execute(
            &mut store,
            ctx(2, 4),
            Operation::Write {
                transfer_id: restore.clone(),
                offset: (i * 8192) as u64,
                bytes: b.to_vec(),
            },
        )
        .unwrap();
    }
    assert!(matches!(
        t.execute(
            &mut store,
            ctx(2, 5),
            Operation::Finish {
                transfer_id: restore.clone()
            }
        )
        .unwrap(),
        Output::RestorePrepared { .. }
    ));
    assert!(matches!(
        t.execute(
            &mut store,
            ctx(2, 6),
            Operation::RestoreData {
                transfer_id: restore.clone(),
                confirmed: false
            }
        ),
        Err(Error::ConfirmationRequired)
    ));
    let data = match t
        .execute(
            &mut store,
            ctx(2, 7),
            Operation::RestoreData {
                transfer_id: restore.clone(),
                confirmed: true,
            },
        )
        .unwrap()
    {
        Output::RestoreData { data } => data,
        _ => panic!(),
    };
    // Authority's exact domain path, evaluated against state at its turn, not prepare time.
    let before = store.load().unwrap();
    let newer = before
        .configuration
        .prepare_edit(
            &before.registry,
            Origin::Editor,
            rad_core::configuration::Edit::CreateButton {
                folder: before.configuration.data().principal,
                position: 1,
                name: "Created after prepare".into(),
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit(&newer),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let current = store.load().unwrap();
    let candidate = current
        .configuration
        .prepare_portable_restore(
            &current.registry,
            Origin::Editor,
            true,
            data.data,
            data.identity.installation,
            data.identity.counters,
        )
        .unwrap();
    let result = t.commit(&mut store, &candidate.candidate, 2, &restore);
    assert!(matches!(result, rad_store::CommitOutcome::Applied { .. }));
    t.resolved(&restore, &result).unwrap();
    assert_eq!(store.load().unwrap().configuration.data().buttons.len(), 1);
    let after = store.load().unwrap();
    assert_eq!(
        after.registry.records().count(),
        current.registry.records().count()
    );
    let removed = current.configuration.data().buttons[1].binding;
    assert!(
        after
            .registry
            .records()
            .any(|r| r.binding() == removed && r.retired())
    );
    assert!(t.metadata().handles.is_empty());
    drop(t);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn prepared_source_survives_disconnect_expiry_and_uncertain_until_resolved() {
    let (path, mut store, mut t) = fixture();
    let id = begin(
        &mut t,
        &mut store,
        ctx(1, 0),
        Kind::ImageImport,
        Some(PNG.len() as u64),
    );
    t.execute(
        &mut store,
        ctx(1, 1),
        Operation::Write {
            transfer_id: id.clone(),
            offset: 0,
            bytes: PNG.to_vec(),
        },
    )
    .unwrap();
    let key = match t
        .execute(
            &mut store,
            ctx(1, 2),
            Operation::Finish {
                transfer_id: id.clone(),
            },
        )
        .unwrap()
    {
        Output::ImagePrepared { media_ref } => media_ref,
        _ => panic!(),
    };
    let c = with_image(&store, key);
    let outcome = t.commit(&mut store, &c, 1, &id);
    assert!(matches!(outcome, rad_store::CommitOutcome::Applied { .. }));
    // Lifecycle seam test, not a claim to induce SQLite failure here: Store's real
    // uncertainty tests are preserved separately.
    t.resolved(
        &id,
        &rad_store::CommitOutcome::Uncertain {
            revision: 2,
            error: rad_store::StoreError::Io,
        },
    )
    .unwrap();
    t.execute(&mut store, ctx(1, 200_000), Operation::Disconnect)
        .unwrap();
    assert_eq!(t.metadata().handles.len(), 1);
    assert!(t.metadata().handles[0].pending);
    assert!(matches!(
        t.execute(
            &mut store,
            ctx(1, 200_001),
            Operation::Drop {
                transfer_id: id.clone()
            }
        ),
        Err(Error::Busy)
    ));
    t.resolved(&id, &outcome).unwrap();
    assert!(t.metadata().handles.is_empty());
    drop(t);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn chunk_limit_and_prepared_ttl_do_not_publish() {
    let (path, mut store, mut t) = fixture();
    let id = begin(&mut t, &mut store, ctx(1, 0), Kind::ImageImport, None);
    assert!(matches!(
        t.execute(
            &mut store,
            ctx(1, 1),
            Operation::Write {
                transfer_id: id.clone(),
                offset: 0,
                bytes: vec![0; 8193]
            }
        ),
        Err(Error::Invalid)
    ));
    t.execute(
        &mut store,
        ctx(1, 2),
        Operation::Write {
            transfer_id: id.clone(),
            offset: 0,
            bytes: PNG.to_vec(),
        },
    )
    .unwrap();
    t.execute(
        &mut store,
        ctx(1, 3),
        Operation::Finish {
            transfer_id: id.clone(),
        },
    )
    .unwrap();
    t.execute(&mut store, ctx(1, 60_003), Operation::Reap)
        .unwrap();
    assert!(t.metadata().handles.is_empty());
    assert_eq!(store.load().unwrap().revision, 1);
    drop(t);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn startup_recovery_discards_abandoned_uploads_without_publication() {
    let (path, mut store, mut t) = fixture();
    let id = begin(&mut t, &mut store, ctx(1, 0), Kind::ImageImport, None);
    t.execute(
        &mut store,
        ctx(1, 1),
        Operation::Write {
            transfer_id: id,
            offset: 0,
            bytes: PNG.to_vec(),
        },
    )
    .unwrap();
    drop(t);
    let t = Transfers::recover(&path.join("transfers"), 456).unwrap();
    assert!(t.metadata().handles.is_empty());
    assert_eq!(store.load().unwrap().revision, 1);
    assert_eq!(
        std::fs::read_dir(path.join("transfers")).unwrap().count(),
        1
    );
    drop(t);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn admitted_image_lease_survives_client_close_before_queued_commit() {
    let (path, mut store, mut t) = fixture();
    let id = begin(&mut t, &mut store, ctx(1, 0), Kind::ImageImport, None);
    t.execute(
        &mut store,
        ctx(1, 1),
        Operation::Write {
            transfer_id: id.clone(),
            offset: 0,
            bytes: PNG.to_vec(),
        },
    )
    .unwrap();
    let key = match t
        .execute(
            &mut store,
            ctx(1, 2),
            Operation::Finish {
                transfer_id: id.clone(),
            },
        )
        .unwrap()
    {
        Output::ImagePrepared { media_ref } => media_ref,
        _ => panic!(),
    };
    assert!(matches!(
        t.execute(
            &mut store,
            ctx(1, 3),
            Operation::RetainImage {
                media_ref: key.clone()
            }
        )
        .unwrap(),
        Output::Retained { .. }
    ));
    t.execute(&mut store, ctx(1, 4), Operation::Disconnect)
        .unwrap();
    let c = with_image(&store, key);
    let result = t.commit(&mut store, &c, 1, &id);
    assert!(matches!(result, rad_store::CommitOutcome::Applied { .. }));
    t.resolved(&id, &result).unwrap();
    drop(t);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn expired_foreign_or_unauthorized_request_cannot_release_admitted_source() {
    let (path, mut store, mut t) = fixture();
    let id = begin(&mut t, &mut store, ctx(1, 0), Kind::ImageImport, None);
    t.execute(
        &mut store,
        ctx(1, 1),
        Operation::Write {
            transfer_id: id.clone(),
            offset: 0,
            bytes: PNG.to_vec(),
        },
    )
    .unwrap();
    let key = match t
        .execute(
            &mut store,
            ctx(1, 2),
            Operation::Finish {
                transfer_id: id.clone(),
            },
        )
        .unwrap()
    {
        Output::ImagePrepared { media_ref } => media_ref,
        _ => panic!(),
    };
    t.execute(
        &mut store,
        ctx(1, 3),
        Operation::RetainImage { media_ref: key },
    )
    .unwrap();
    let _ = t.execute(
        &mut store,
        Context {
            origin: Origin::Automation,
            ..ctx(2, 200_000)
        },
        Operation::Finish {
            transfer_id: id.clone(),
        },
    );
    assert_eq!(t.metadata().handles.len(), 1);
    let _ = t.execute(
        &mut store,
        ctx(2, 200_001),
        Operation::Finish {
            transfer_id: id.clone(),
        },
    );
    assert_eq!(t.metadata().handles.len(), 1);
    let _ = t.execute(
        &mut store,
        ctx(1, 200_002),
        Operation::Finish {
            transfer_id: id.clone(),
        },
    );
    assert_eq!(t.metadata().handles.len(), 1);
    t.resolved(
        &id,
        &rad_store::CommitOutcome::NotApplied(rad_store::StoreError::Invalid),
    )
    .unwrap();
    drop(t);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn automation_can_read_root_resolved_visible_media_but_not_manage_transfers() {
    let (path, mut store, mut t) = fixture();
    let image = rad_media::normalize(PNG).unwrap();
    let key = rad_store::media_key(&image);
    let c = with_image(&store, key.clone());
    assert!(matches!(
        store.commit_with_media(&c, &[image]),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let automation = Context {
        origin: Origin::Automation,
        ..ctx(2, 0)
    };
    assert!(matches!(
        t.execute(
            &mut store,
            automation,
            Operation::MediaRead {
                resolved_key: key,
                offset: 0
            }
        )
        .unwrap(),
        Output::Chunk { eof: true, .. }
    ));
    assert!(matches!(
        t.execute(
            &mut store,
            automation,
            Operation::Begin {
                kind: Kind::Export,
                size: None,
                export_revision: Some(2),
                export_snapshot: None,
            }
        ),
        Err(Error::NotAllowed)
    ));
    assert!(matches!(
        t.execute(&mut store, automation, Operation::Reap),
        Err(Error::NotAllowed)
    ));
    drop(t);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn internal_load_visual_returns_owned_normalized_image_only_to_trusted_editor() {
    let (path, mut store, mut t) = fixture();
    let image = rad_media::normalize(PNG).unwrap();
    let key = rad_store::media_key(&image);
    let c = with_image(&store, key.clone());
    assert!(matches!(
        store.commit_with_media(&c, std::slice::from_ref(&image)),
        rad_store::CommitOutcome::Applied { .. }
    ));
    match t
        .execute(
            &mut store,
            ctx(1, 0),
            Operation::LoadVisual {
                resolved_key: key.clone(),
            },
        )
        .unwrap()
    {
        Output::Visual { image: loaded } => assert_eq!(loaded, image),
        _ => panic!(),
    }
    assert!(matches!(
        t.execute(
            &mut store,
            Context {
                origin: Origin::Automation,
                ..ctx(2, 1)
            },
            Operation::LoadVisual { resolved_key: key }
        ),
        Err(Error::NotAllowed)
    ));
    drop(t);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn export_uses_pinned_registry_not_later_store_high_water_marks() {
    let path = root();
    std::fs::create_dir(&path).unwrap();
    let base = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let mut store = rad_store::Store::create(&path.join("db"), &base).unwrap();
    let later = base
        .configuration()
        .prepare_edit(
            base.registry(),
            Origin::Editor,
            rad_core::configuration::Edit::CreateButton {
                folder: base.configuration().data().principal,
                position: 0,
                name: "later".into(),
            },
        )
        .unwrap();
    assert!(matches!(
        store.commit(&later),
        rad_store::CommitOutcome::Applied { .. }
    ));
    let mut transfers = Transfers::create(&path.join("transfers"), 77).unwrap();
    let output = transfers
        .execute(
            &mut store,
            ctx(1, 0),
            Operation::Begin {
                kind: Kind::Export,
                size: None,
                export_revision: Some(1),
                export_snapshot: Some(base.clone()),
            },
        )
        .unwrap();
    let Output::Begun { transfer_id } = output else {
        panic!()
    };
    let Output::Chunk { bytes, .. } = transfers
        .execute(
            &mut store,
            ctx(1, 1),
            Operation::Read {
                transfer_id,
                offset: 0,
            },
        )
        .unwrap()
    else {
        panic!()
    };
    let area = rad_package::StagingArea::create_new(&path.join("verify")).unwrap();
    let (verified, media) = rad_package::prepare(&mut bytes.as_slice(), &area)
        .unwrap()
        .into_verified_data()
        .unwrap();
    assert_eq!(verified.identity.counters, base.registry().counters());
    drop(media);
    drop(area);
    drop(transfers);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn export_metadata_uses_installed_selection_and_preserves_absent_hints_without_mutating_deck() {
    use rad_core::configuration::{PluginDependency, PluginSettings, PluginVersion};
    let initial = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let old = PluginDependency {
        version: Some(PluginVersion {
            major: 1,
            minor: 0,
            patch: 0,
        }),
        origin: Some("https://github.com/example/old".into()),
    };
    let new = PluginDependency {
        version: Some(PluginVersion {
            major: 2,
            minor: 0,
            patch: 0,
        }),
        origin: Some("https://github.com/example/new".into()),
    };
    let mut data = initial.configuration().data().clone();
    for id in ["present", "missing"] {
        data.plugin_settings
            .insert(id.into(), PluginSettings::new(vec![]).unwrap());
        data.plugin_dependencies.insert(id.into(), old.clone());
    }
    data.plugin_dependencies
        .insert("unused".into(), old.clone());
    let base = initial
        .configuration()
        .prepare_restore(initial.registry(), Origin::Editor, true, data)
        .unwrap();
    let snapshot = export_snapshot(
        &base,
        [
            ("present".into(), new.clone()),
            ("unrelated".into(), new.clone()),
        ]
        .into(),
    )
    .unwrap();
    let deps = &snapshot.configuration().data().plugin_dependencies;
    assert_eq!(deps.len(), 2);
    assert_eq!(deps["present"], new);
    assert_eq!(deps["missing"], old);
    assert_eq!(
        base.configuration().data().plugin_dependencies["present"],
        old
    );
    assert_eq!(snapshot.registry(), base.registry());
}

#[test]
fn export_requires_a_snapshot_captured_by_its_owner() {
    let (path, mut store, mut transfers) = fixture();
    let result = transfers.execute(
        &mut store,
        ctx(1, 0),
        Operation::Begin {
            kind: Kind::Export,
            size: None,
            export_revision: Some(1),
            export_snapshot: None,
        },
    );
    assert!(matches!(result, Err(Error::Invalid)));
    assert!(transfers.metadata().handles.is_empty());
    drop(transfers);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn export_metadata_that_exceeds_document_budget_returns_specific_capacity_cause() {
    use rad_core::configuration::{PluginDependency, PluginSettings};
    let path = root();
    std::fs::create_dir(&path).unwrap();
    let initial = Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap();
    let mut data = initial.configuration().data().clone();
    for n in 0..100 {
        data.plugin_settings.insert(
            format!("plugin-{n}"),
            PluginSettings::new(vec![255; 4095]).unwrap(),
        );
    }
    let base = initial
        .configuration()
        .prepare_restore(initial.registry(), Origin::Editor, true, data)
        .unwrap();
    let mut store = rad_store::Store::create(&path.join("db"), &base).unwrap();
    let installed = (0..100)
        .map(|n| {
            (
                format!("plugin-{n}"),
                PluginDependency {
                    version: None,
                    origin: Some("x".repeat(4000)),
                },
            )
        })
        .collect();
    let snapshot = export_snapshot(&base, installed).unwrap();
    let mut transfers = Transfers::create(&path.join("transfers"), 77).unwrap();
    let result = transfers.execute(
        &mut store,
        ctx(1, 0),
        Operation::Begin {
            kind: Kind::Export,
            size: None,
            export_revision: Some(1),
            export_snapshot: Some(snapshot),
        },
    );
    assert!(matches!(result, Err(Error::DocumentLimit)), "{result:?}");
    assert!(transfers.metadata().handles.is_empty());
    drop(transfers);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
