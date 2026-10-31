use rad_core::{configuration::Configuration, identity::IdentityRegistry};
use rad_runtime::transfers::*;

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

use rad_core::configuration::Origin;

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
                    states: Default::default(),
                    text: "Image".into(),
                    image: Some(key),
                    text_position: None,
                },
            },
        )
        .unwrap()
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
fn two_prepared_state_images_commit_together_and_survive_reload() {
    use rad_core::configuration::{Appearance, Edit, StateFace};
    let (path, mut store, mut t) = fixture();
    let mut images = vec![];
    for (index, bytes) in [
        include_bytes!("../../../plugins/audio/icons/microphone-muted.png").as_slice(),
        include_bytes!("../../../plugins/audio/icons/microphone-unmuted.png").as_slice(),
    ]
    .into_iter()
    .enumerate()
    {
        let id = begin(
            &mut t,
            &mut store,
            ctx(1, index as u64 * 3),
            Kind::ImageImport,
            Some(bytes.len() as u64),
        );
        t.execute(
            &mut store,
            ctx(1, index as u64 * 3 + 1),
            Operation::Write {
                transfer_id: id.clone(),
                offset: 0,
                bytes: bytes.to_vec(),
            },
        )
        .unwrap();
        let Output::ImagePrepared { media_ref } = t
            .execute(
                &mut store,
                ctx(1, index as u64 * 3 + 2),
                Operation::Finish {
                    transfer_id: id.clone(),
                },
            )
            .unwrap()
        else {
            panic!()
        };
        images.push((id, media_ref));
    }
    assert!(matches!(
        t.execute(
            &mut store,
            ctx(1, 6),
            Operation::Begin {
                kind: Kind::ImageImport,
                size: Some(1),
                export_revision: None,
                export_snapshot: None
            }
        ),
        Err(Error::Capacity)
    ));
    for refs in [
        vec![images[0].1.clone(), "missing".into()],
        vec![images[0].1.clone(); 3],
    ] {
        assert!(
            t.execute(
                &mut store,
                ctx(1, 6),
                Operation::RetainImages { media_refs: refs }
            )
            .is_err()
        );
    }
    assert!(matches!(
        t.execute(
            &mut store,
            ctx(2, 6),
            Operation::RetainImages {
                media_refs: vec![images[0].1.clone()]
            }
        ),
        Err(Error::NotFound)
    ));
    let c = with_image(&store, images[0].1.clone());
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetAppearance {
                button: c.configuration().data().buttons[0].binding.identity(),
                appearance: Appearance {
                    states: [
                        (
                            "muted".into(),
                            StateFace {
                                text: "Quiet".into(),
                                image: Some(images[0].1.clone()),
                            },
                        ),
                        (
                            "unmuted".into(),
                            StateFace {
                                text: "Live".into(),
                                image: Some(images[1].1.clone()),
                            },
                        ),
                    ]
                    .into(),
                    ..Default::default()
                },
            },
        )
        .unwrap();
    let Output::Retained { transfer_id } = t
        .execute(
            &mut store,
            ctx(1, 6),
            Operation::RetainImages {
                media_refs: images.iter().map(|(_, key)| key.clone()).collect(),
            },
        )
        .unwrap()
    else {
        panic!()
    };
    t.execute(&mut store, ctx(1, 7), Operation::Disconnect)
        .unwrap();
    t.execute(&mut store, ctx(2, 100_000), Operation::Reap)
        .unwrap();
    assert_eq!(t.metadata().handles.len(), 2);
    let outcome = t.commit(&mut store, &c, 1, &transfer_id);
    assert!(
        matches!(outcome, rad_store::CommitOutcome::Applied { .. }),
        "{outcome:?}"
    );
    assert!(
        matches!(t.unretain_images(&transfer_id), Err(Error::Busy)),
        "started persistence cannot be returned to prepared"
    );
    t.resolved(
        &transfer_id,
        &rad_store::CommitOutcome::Uncertain {
            revision: 2,
            error: rad_store::StoreError::Uncertain,
        },
    )
    .unwrap();
    t.execute(&mut store, ctx(2, 200_000), Operation::Reap)
        .unwrap();
    assert_eq!(t.metadata().handles.len(), 2);
    assert!(
        matches!(
            t.commit(&mut store, &c, 1, &transfer_id),
            rad_store::CommitOutcome::NotApplied(_)
        ),
        "an uncertain edit is never replayed"
    );
    t.resolved(&transfer_id, &outcome).unwrap();
    assert!(t.metadata().handles.is_empty());
    assert!(store.image(&images[0].1).is_ok());
    assert!(store.image(&images[1].1).is_ok());
    assert_eq!(
        store.load().unwrap().configuration.data().buttons[0]
            .appearance
            .states
            .len(),
        2
    );
    drop(t);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn view_conflict_unretains_both_images_without_extending_expiry() {
    for finish in ["expire", "cancel", "disconnect"] {
        let (path, mut store, mut transfers) = fixture();
        let mut images = vec![];
        for (index, bytes) in [
            include_bytes!("../../../plugins/audio/icons/microphone-muted.png").as_slice(),
            include_bytes!("../../../plugins/audio/icons/microphone-unmuted.png").as_slice(),
        ]
        .into_iter()
        .enumerate()
        {
            let id = begin(
                &mut transfers,
                &mut store,
                ctx(1, index as u64 * 3),
                Kind::ImageImport,
                Some(bytes.len() as u64),
            );
            transfers
                .execute(
                    &mut store,
                    ctx(1, index as u64 * 3 + 1),
                    Operation::Write {
                        transfer_id: id.clone(),
                        offset: 0,
                        bytes: bytes.to_vec(),
                    },
                )
                .unwrap();
            let Output::ImagePrepared { media_ref } = transfers
                .execute(
                    &mut store,
                    ctx(1, index as u64 * 3 + 2),
                    Operation::Finish {
                        transfer_id: id.clone(),
                    },
                )
                .unwrap()
            else {
                panic!()
            };
            images.push((id, media_ref));
        }
        let deadline = transfers.metadata().next_deadline_ms;
        for now in [10, 30_000, 59_000] {
            let Output::Retained { transfer_id } = transfers
                .execute(
                    &mut store,
                    ctx(1, now),
                    Operation::RetainImages {
                        media_refs: images.iter().map(|(_, key)| key.clone()).collect(),
                    },
                )
                .unwrap()
            else {
                panic!()
            };
            transfers.unretain_images(&transfer_id).unwrap();
            assert!(transfers.metadata().handles.iter().all(|h| !h.pending));
            assert_eq!(
                transfers.metadata().next_deadline_ms,
                deadline,
                "rejection must not renew original lease"
            );
        }
        match finish {
            "expire" => {
                transfers
                    .execute(&mut store, ctx(1, 60_006), Operation::Reap)
                    .unwrap();
            }
            "cancel" => {
                for (id, _) in &images {
                    transfers
                        .execute(
                            &mut store,
                            ctx(1, 59_001),
                            Operation::Drop {
                                transfer_id: id.clone(),
                            },
                        )
                        .unwrap();
                }
            }
            _ => {
                transfers
                    .execute(&mut store, ctx(1, 59_001), Operation::Disconnect)
                    .unwrap();
            }
        }
        assert!(transfers.metadata().handles.is_empty(), "{finish}");
        drop(transfers);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
}
