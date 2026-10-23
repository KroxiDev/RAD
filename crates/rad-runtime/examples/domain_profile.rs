//! Offline phase-4 instrumentation. No runtime services, devices or user data.
//! The sole argument is a NEW directory below this checkout's target/performance.
use rad_authority::{Authority, Command, Effect, Outcome};
use rad_core::{configuration::*, identity::*};
use rad_store::{CommitOutcome, Store};
use serde_json::json;
use std::{
    hint::black_box,
    num::NonZeroU128,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    time::Instant,
};

const REPETITIONS: usize = 12;

fn populated(count: usize, media: &[rad_media::NormalizedImage]) -> Candidate {
    let mut candidate = Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap();
    for position in 0..if count == 32 { 1 } else { 16 } {
        candidate = candidate
            .configuration()
            .prepare_edit(
                candidate.registry(),
                Origin::Editor,
                Edit::CreateFolder {
                    parent: candidate.configuration().data().principal,
                    position,
                    name: String::new(),
                },
            )
            .unwrap();
    }
    let mut data = candidate.configuration().data().clone();
    let mut registry = candidate.registry().clone();
    let mut total = registry.records().count();
    for folder in &mut data.folders {
        for slot in &mut folder.slots {
            if slot.is_some() || total == count {
                continue;
            }
            let binding = registry.allocate(EntityKind::Button).unwrap();
            total += 1;
            let mut actions = Vec::new();
            for _ in 0..MAX_ACTIONS.min(count - total) {
                actions.push(ActionData {
                    binding: registry.allocate(EntityKind::Action).unwrap(),
                    kind: ActionKind::Wait(Wait::new(0).unwrap()),
                    permissions: Default::default(),
                });
                total += 1;
            }
            *slot = Some(binding.identity());
            data.buttons.push(ButtonData {
                binding,
                name: String::new(),
                appearance: Default::default(),
                visual: VisualSource::Manual,
                permissions: Default::default(),
                mode: Default::default(),
                gesture: Default::default(),
                actions,
            });
        }
    }
    assert_eq!(total, count);
    for (index, button) in data.buttons.iter_mut().enumerate() {
        if !media.is_empty() {
            button.appearance.image = Some(rad_store::media_key(&media[index % media.len()]));
        }
    }
    let configuration = Configuration::from_data(data, &registry).unwrap();
    configuration
        .prepare_restore(
            &registry,
            Origin::Editor,
            true,
            configuration.data().clone(),
        )
        .unwrap()
}

// Tiny deterministic synthetic PNGs; normalization and checksums are production code.
// A one-pixel catalogue measures validation overhead, not real photographic decoding.
fn synthetic_image(color: u8) -> rad_media::NormalizedImage {
    fn crc(bytes: &[u8]) -> u32 {
        let mut value = u32::MAX;
        for byte in bytes {
            value ^= u32::from(*byte);
            for _ in 0..8 {
                value = (value >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(value & 1)));
            }
        }
        !value
    }
    fn chunk(png: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = png.len();
        png.extend_from_slice(name);
        png.extend_from_slice(data);
        let checksum = crc(&png[start..]);
        png.extend_from_slice(&checksum.to_be_bytes());
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    let pixels = [0, color, color.wrapping_mul(7), 31, 255];
    let mut adler_a = 1u32;
    let mut adler_b = 0u32;
    for byte in pixels {
        adler_a = (adler_a + u32::from(byte)) % 65521;
        adler_b = (adler_b + adler_a) % 65521;
    }
    let mut zlib = vec![0x78, 0x01, 0x01, 5, 0, 250, 255];
    zlib.extend_from_slice(&pixels);
    zlib.extend_from_slice(&((adler_b << 16) | adler_a).to_be_bytes());
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    rad_media::normalize(&png).unwrap()
}

fn sample(scenario: &str, count: usize, operation: &str, iteration: usize, start: Instant) {
    let ns = start.elapsed().as_nanos();
    println!(
        "{}",
        json!({"kind":"sample", "scenario":scenario, "identity_count":count,
            "operation":operation, "iteration":iteration, "ns":ns})
    );
}

fn run(root: &Path, count: usize, with_media: bool) {
    let scenario = if with_media { "media" } else { "plain" };
    let images: Vec<_> = if with_media {
        (0..16).map(synthetic_image).collect()
    } else {
        Vec::new()
    };
    let mut candidate = populated(count, &images);
    // The small case has fewer buttons than images. Only install referenced media.
    let images: Vec<_> = images
        .into_iter()
        .filter(|image| {
            let key = rad_store::media_key(image);
            candidate
                .configuration()
                .data()
                .buttons
                .iter()
                .any(|b| b.appearance.image.as_ref() == Some(&key))
        })
        .collect();
    let principal = candidate.configuration().data().principal;
    let child = candidate.configuration().data().folders[1]
        .binding
        .identity();
    let mut actor = Authority::new(candidate.clone(), 1, NonZeroU128::new(1).unwrap());
    let probe = actor.reserve(1, Origin::Editor).unwrap();
    let mut settings = candidate.configuration().data().settings;
    settings.brightness_percent = 41;
    let probe_effects = actor.submit(probe, Command::Edits(vec![Edit::SetSettings(settings)]));
    let document_bytes = probe_effects.iter().find_map(|e| match e {
        Effect::Persist { document, .. } => Some(document.len()),
        _ => None,
    });
    println!(
        "{}",
        json!({"kind":"configuration", "scenario":scenario, "identity_count":count,
            "ledger_count":candidate.registry().records().count(), "growth_ceiling":MAX_LIVE_IDENTITIES,
            "historical_above_growth_ceiling":count>MAX_LIVE_IDENTITIES,
            "document_bytes":document_bytes, "media_count":images.len(),
            "media_bytes":images.iter().map(|i|i.png_bytes().len()).sum::<usize>(),
            "repetitions":REPETITIONS})
    );
    // Recreate after the probe so publication state and store revision start together.
    actor = Authority::new(candidate.clone(), 1, NonZeroU128::new(1).unwrap());
    let mut store = document_bytes.map(|_| {
        Store::create_with_media(
            &root.join(format!("{scenario}-{count}.sqlite3")),
            &candidate,
            &images,
        )
        .unwrap()
    });
    if store.is_none() {
        println!(
            "{}",
            json!({"kind":"unmeasured", "scenario":scenario, "identity_count":count,
                "operations":["authority_submit", "store_commit", "authority_complete", "store_load"],
                "reason":"canonical_document_exceeds_1_mib", "probe_effects":format!("{probe_effects:?}")})
        );
    }
    for iteration in 0..REPETITIONS {
        let cloned = candidate.clone();
        let start = Instant::now();
        let projected = Authority::new(cloned, 1, NonZeroU128::new(2).unwrap());
        black_box(&projected);
        sample(scenario, count, "authority_new", iteration, start);
        drop(projected);

        let mut settings = candidate.configuration().data().settings;
        settings.brightness_percent = if iteration % 2 == 0 { 42 } else { 43 };
        let start = Instant::now();
        let prepared = candidate
            .configuration()
            .prepare_edit(
                candidate.registry(),
                Origin::Editor,
                Edit::SetSettings(settings),
            )
            .unwrap();
        black_box(&prepared);
        sample(scenario, count, "prepare_edit", iteration, start);

        if let Some(store) = &mut store {
            let ticket = actor.reserve(1, Origin::Editor).unwrap();
            let request = ticket.request();
            let start = Instant::now();
            let effects = actor.submit(ticket, Command::Edits(vec![Edit::SetSettings(settings)]));
            sample(scenario, count, "authority_submit", iteration, start);
            let (id, published) = effects
                .into_iter()
                .find_map(|e| match e {
                    Effect::Persist { id, candidate, .. } => Some((id, candidate)),
                    _ => None,
                })
                .expect("changed settings must produce a valid document");
            let start = Instant::now();
            let outcome = store.commit(&published);
            sample(scenario, count, "store_commit", iteration, start);
            let CommitOutcome::Applied { revision, changed } = outcome else {
                panic!("durable commit: {outcome:?}");
            };
            assert!(changed);
            let start = Instant::now();
            let completed = actor.complete(id, Outcome::Applied { revision, changed });
            black_box(&completed);
            sample(scenario, count, "authority_complete", iteration, start);
            assert!(actor.finish(request));
            candidate = published;
            let start = Instant::now();
            let loaded = store.load().unwrap();
            black_box(&loaded);
            sample(scenario, count, "store_load", iteration, start);
            assert_eq!(loaded.configuration, *candidate.configuration());
            assert_eq!(
                loaded.registry.records().filter(|r| !r.retired()).count(),
                count
            );
        }
        let destination = if iteration % 2 == 0 { child } else { principal };
        let start = Instant::now();
        let effects = actor.navigate(Origin::Editor, destination).unwrap();
        black_box(&effects);
        sample(scenario, count, "navigate", iteration, start);
        assert_eq!(
            actor.editor_view(Origin::Editor).unwrap().active,
            destination
        );
    }
    // Reopening checks that timings were not obtained by disabling durability/checksums.
    drop(store);
    if document_bytes.is_some() {
        let reopened = Store::open(&root.join(format!("{scenario}-{count}.sqlite3"))).unwrap();
        let loaded = reopened.load().unwrap();
        assert_eq!(loaded.configuration, *candidate.configuration());
    }
}

fn output_root() -> PathBuf {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(
        args.next()
            .expect("one new target/performance directory required"),
    );
    assert!(
        args.next().is_none(),
        "only one directory argument is accepted"
    );
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/performance")
        .canonicalize()
        .expect("create target/performance before running");
    let parent = path
        .parent()
        .expect("directory needs a parent")
        .canonicalize()
        .unwrap();
    assert!(
        parent.starts_with(&base),
        "output must be under target/performance"
    );
    assert!(path.file_name().is_some(), "output needs a directory name");
    let path = parent.join(path.file_name().unwrap());
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    path
}

fn main() {
    let root = output_root();
    for count in [32, 7000, 10000] {
        for with_media in [false, true] {
            run(&root, count, with_media);
        }
    }
}
