//! Public-API-only phase-4 probes: no devices, services, users or production changes.
//! Pass one NEW directory below this checkout's target/performance; optionally
//! add --representative for smaller configurations and compressible media.
use rad_authority::{Authority, Command, Effect, Outcome};
use rad_core::{configuration::*, identity::*};
use rad_media::NormalizedImage;
use rad_store::{CommitOutcome, Store};
use serde_json::json;
use std::{
    hint::black_box,
    num::NonZeroU128,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    time::Instant,
};

const REPETITIONS: usize = 10;

struct Scenario {
    name: String,
    identity_count: usize,
    media_count: usize,
    media_bytes: usize,
    pictured_buttons: usize,
    document_bytes: usize,
    media_pattern: &'static str,
    source_bytes: usize,
    raw_rgba_bytes: usize,
}
impl Scenario {
    fn configuration(&self, detail: &str) {
        println!(
            "{}",
            json!({"kind":"configuration", "scenario":self.name,
                "identity_count":self.identity_count,"media_count":self.media_count,
                "media_bytes":self.media_bytes,"pictured_buttons":self.pictured_buttons,
                "document_bytes":self.document_bytes,"repetitions":REPETITIONS,"detail":detail,
                "media_pattern":self.media_pattern,"source_bytes":self.source_bytes,
                "raw_rgba_bytes":self.raw_rgba_bytes,"source_png_encoding":"stored_deflate",
                "normalized_bytes_per_rgba_milli":(self.media_bytes * 1000).checked_div(self.raw_rgba_bytes)})
        );
    }
    fn timed<T>(&self, operation: &str, iteration: usize, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let value = black_box(f());
        let ns = start.elapsed().as_nanos();
        println!(
            "{}",
            json!({"kind":"sample","scenario":self.name,"identity_count":self.identity_count,
                "media_count":self.media_count,"media_bytes":self.media_bytes,
                "document_bytes":self.document_bytes,"operation":operation,
                "iteration":iteration,"ns":ns})
        );
        value
    }
}

fn populated(count: usize, media: &[NormalizedImage], shared: bool) -> Candidate {
    let mut base = Configuration::prepare_new(&IdentityRegistry::new(2).unwrap()).unwrap();
    for position in 0..if count == 32 { 1 } else { 16 } {
        base = base
            .configuration()
            .prepare_edit(
                base.registry(),
                Origin::Editor,
                Edit::CreateFolder {
                    parent: base.configuration().data().principal,
                    position,
                    name: String::new(),
                },
            )
            .unwrap();
    }
    let mut data = base.configuration().data().clone();
    data.settings.brightness_percent = 40;
    let mut registry = base.registry().clone();
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
    assert!(data.buttons.len() >= media.len());
    let keys: Vec<_> = media.iter().map(rad_store::media_key).collect();
    for (index, button) in data.buttons.iter_mut().enumerate() {
        if !keys.is_empty() && (shared || index < keys.len()) {
            button.appearance.image = Some(keys[index % keys.len()].clone());
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

fn crc(bytes: &[u8]) -> u32 {
    let mut value = u32::MAX;
    for byte in bytes {
        value ^= u32::from(*byte);
        for _ in 0..8 {
            value = (value >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(value & 1));
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
#[derive(Clone, Copy)]
enum Pattern {
    Noise,
    Flat,
    Bands,
}
impl Pattern {
    fn name(self) -> &'static str {
        match self {
            Self::Noise => "noise",
            Self::Flat => "flat",
            Self::Bands => "bands",
        }
    }
}
// Stored DEFLATE avoids encoder dependencies. Only the normalized PNG's
// compressibility is representative: source decoding still reads stored blocks.
fn synthetic_png(seed: u32, side: u32, pattern: Pattern) -> Vec<u8> {
    let stride = (side * 4 + 1) as usize;
    let mut pixels = vec![0; side as usize * stride];
    let mut state = seed.wrapping_add(1).wrapping_mul(0x9e37_79b9);
    for (y, row) in pixels.chunks_mut(stride).enumerate() {
        // Byte zero is PNG filter None; fill the rest with deterministic RGBA.
        for rgba in row[1..].as_chunks_mut::<4>().0 {
            for value in &mut rgba[..3] {
                *value = match pattern {
                    Pattern::Noise => {
                        state ^= state << 13;
                        state ^= state >> 17;
                        state ^= state << 5;
                        state as u8
                    }
                    Pattern::Flat => seed.wrapping_mul(13) as u8,
                    Pattern::Bands => {
                        seed.wrapping_mul(13)
                            .wrapping_add(((y / 8) % 2) as u32 * 31) as u8
                    }
                };
            }
            rgba[3] = 255;
        }
    }
    let mut a = 1u32;
    let mut b = 0u32;
    for byte in &pixels {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks = pixels.len().div_ceil(65535);
    for (i, bytes) in pixels.chunks(65535).enumerate() {
        zlib.push(u8::from(i + 1 == blocks));
        let length = u16::try_from(bytes.len()).unwrap();
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(bytes);
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::from(side.to_be_bytes());
    header.extend_from_slice(&side.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

fn document_size(candidate: &Candidate) -> usize {
    let mut actor = Authority::new(candidate.clone(), 1, NonZeroU128::new(1).unwrap());
    let ticket = actor.reserve(1, Origin::Editor).unwrap();
    let mut settings = candidate.configuration().data().settings;
    settings.brightness_percent = 41;
    actor
        .submit(ticket, Command::Edits(vec![Edit::SetSettings(settings)]))
        .into_iter()
        .find_map(|e| match e {
            Effect::Persist { document, .. } => Some(document.len()),
            _ => None,
        })
        .expect("profile must fit the current document budget")
}
fn scenario(name: String, candidate: &Candidate, images: &[NormalizedImage]) -> Scenario {
    Scenario {
        name,
        identity_count: candidate.configuration().data().entity_count(),
        media_count: images.len(),
        media_bytes: images.iter().map(|i| i.png_bytes().len()).sum(),
        pictured_buttons: candidate
            .configuration()
            .data()
            .buttons
            .iter()
            .filter(|b| b.appearance.image.is_some())
            .count(),
        document_bytes: document_size(candidate),
        media_pattern: "none",
        source_bytes: 0,
        raw_rgba_bytes: 0,
    }
}
fn changed(candidate: &Candidate, brightness: u8) -> Candidate {
    let mut settings = candidate.configuration().data().settings;
    settings.brightness_percent = brightness;
    candidate
        .configuration()
        .prepare_edit(
            candidate.registry(),
            Origin::Editor,
            Edit::SetSettings(settings),
        )
        .unwrap()
}
fn applied(outcome: CommitOutcome) -> (i64, bool) {
    let CommitOutcome::Applied { revision, changed } = outcome else {
        panic!("profile did not obtain a durable commit: {outcome:?}");
    };
    assert!(changed);
    (revision, changed)
}

fn domain(root: &Path, count: usize) {
    let mut candidate = populated(count, &[], false);
    let meta = scenario(format!("domain-{count}"), &candidate, &[]);
    meta.configuration("public APIs; empty installed-types setter includes full refresh_views; private substeps are not directly timed");
    let principal = candidate.configuration().data().principal;
    let child = candidate.configuration().data().folders[1]
        .binding
        .identity();
    let ids: Vec<_> = candidate
        .configuration()
        .data()
        .buttons
        .iter()
        .flat_map(|b| {
            std::iter::once(b.binding.short()).chain(b.actions.iter().map(|a| a.binding.short()))
        })
        .collect();
    let button_ids: Vec<_> = candidate
        .configuration()
        .data()
        .buttons
        .iter()
        .map(|b| b.binding.short())
        .collect();
    let folder_ids: Vec<_> = candidate
        .configuration()
        .data()
        .folders
        .iter()
        .map(|f| f.binding.short())
        .collect();
    let path = root.join(format!("{}.sqlite3", meta.name));
    let mut store = Store::create(&path, &candidate).unwrap();
    let mut actor = Authority::new(candidate.clone(), 1, NonZeroU128::new(2).unwrap());
    for iteration in 0..REPETITIONS {
        drop(meta.timed("candidate_clone", iteration, || candidate.clone()));
        drop(meta.timed("configuration_data_clone", iteration, || {
            candidate.configuration().data().clone()
        }));
        drop(meta.timed("registry_clone", iteration, || candidate.registry().clone()));
        let data = candidate.configuration().data().clone();
        let validated = meta.timed("configuration_validate_owned", iteration, || {
            Configuration::from_data(data, candidate.registry()).unwrap()
        });
        assert_eq!(&validated, candidate.configuration());
        drop(validated);
        let next = meta.timed("prepare_edit", iteration, || {
            changed(&candidate, if iteration % 2 == 0 { 42 } else { 43 })
        });
        let revision = actor.view_revision(Origin::Editor);
        meta.timed("refresh_views_via_installed_types_empty", iteration, || {
            actor.installed_types(Vec::new());
        });
        assert_eq!(revision, actor.view_revision(Origin::Editor));
        drop(meta.timed("query_entities_automation_all", iteration, || {
            ids.iter()
                .map(|id| actor.get(Origin::Automation, *id))
                .collect::<Vec<_>>()
        }));
        drop(
            meta.timed("query_appearance_automation_all", iteration, || {
                button_ids
                    .iter()
                    .map(|id| actor.appearance(Origin::Automation, *id))
                    .collect::<Vec<_>>()
            }),
        );
        drop(meta.timed("query_folders_editor_all", iteration, || {
            folder_ids
                .iter()
                .map(|id| actor.folder(Origin::Editor, *id))
                .collect::<Vec<_>>()
        }));
        drop(meta.timed("navigate", iteration, || {
            actor
                .navigate(
                    Origin::Editor,
                    if iteration % 2 == 0 { child } else { principal },
                )
                .unwrap()
        }));
        let ticket = actor.reserve(1, Origin::Editor).unwrap();
        let request = ticket.request();
        let settings = next.configuration().data().settings;
        let effects = meta.timed("authority_submit", iteration, || {
            actor.submit(ticket, Command::Edits(vec![Edit::SetSettings(settings)]))
        });
        let (id, published) = effects
            .into_iter()
            .find_map(|e| match e {
                Effect::Persist { id, candidate, .. } => Some((id, candidate)),
                _ => None,
            })
            .expect("changed setting must be persistible");
        assert_eq!(next.configuration(), published.configuration());
        let outcome = meta.timed("store_commit_existing", iteration, || {
            store.commit(&published)
        });
        let (revision, changed) = applied(outcome);
        drop(meta.timed("authority_complete", iteration, || {
            actor.complete(id, Outcome::Applied { revision, changed })
        }));
        assert!(actor.finish(request));
        candidate = published;
        let loaded = meta.timed("store_load", iteration, || store.load().unwrap());
        assert_eq!(loaded.configuration, *candidate.configuration());
    }
    drop(store);
    assert_eq!(
        Store::open(&path).unwrap().load().unwrap().configuration,
        *candidate.configuration()
    );
}

fn media(
    root: &Path,
    entity_count: usize,
    image_count: usize,
    side: u32,
    shared: bool,
    pattern: Pattern,
) {
    let raw: Vec<_> = (0..image_count)
        .map(|i| synthetic_png(u32::try_from(i).unwrap(), side, pattern))
        .collect();
    let images: Vec<_> = raw
        .iter()
        .map(|b| rad_media::normalize(b).unwrap())
        .collect();
    assert!(images.iter().all(|i| i.dimensions() == (side, side)));
    let mut candidate = populated(entity_count, &images, shared);
    let policy = if shared { "shared" } else { "unique" };
    let name = if matches!(pattern, Pattern::Noise) {
        format!("media-{policy}-{image_count}-{side}")
    } else {
        format!(
            "media-{entity_count}-{}-{policy}-{image_count}-{side}",
            pattern.name()
        )
    };
    let mut meta = scenario(name, &candidate, &images);
    meta.media_pattern = pattern.name();
    meta.source_bytes = raw.iter().map(Vec::len).sum();
    meta.raw_rgba_bytes = image_count * side as usize * side as usize * 4;
    let unique: std::collections::BTreeSet<_> = images.iter().map(rad_store::media_key).collect();
    assert_eq!(
        unique.len(),
        image_count,
        "catalogue must not silently deduplicate"
    );
    meta.configuration("existing catalogue vs same catalogue resupplied; shared pictures all buttons, unique pictures exactly one button per image; no cache eviction");
    let keys: Vec<_> = images.iter().map(rad_store::media_key).collect();
    let path = root.join(format!("{}.sqlite3", meta.name));
    let mut store = Store::create_with_media(&path, &candidate, &images).unwrap();
    for iteration in 0..REPETITIONS {
        drop(meta.timed("media_normalize_catalog", iteration, || {
            raw.iter()
                .map(|b| rad_media::normalize(b).unwrap())
                .collect::<Vec<_>>()
        }));
        drop(meta.timed("media_validate_catalog", iteration, || {
            images
                .iter()
                .map(|i| NormalizedImage::validate(i.png_bytes()).unwrap())
                .collect::<Vec<_>>()
        }));
        drop(meta.timed("media_digest_catalog", iteration, || {
            images.iter().map(rad_store::media_key).collect::<Vec<_>>()
        }));
        drop(meta.timed("store_read_catalog", iteration, || {
            keys.iter()
                .map(|key| store.image(key).unwrap())
                .collect::<Vec<_>>()
        }));
        let next = changed(&candidate, if iteration % 2 == 0 { 42 } else { 43 });
        applied(meta.timed("store_commit_existing", iteration, || store.commit(&next)));
        candidate = next;
        let next = changed(&candidate, if iteration % 2 == 0 { 44 } else { 45 });
        applied(meta.timed("store_commit_resupplied", iteration, || {
            store.commit_with_media(&next, &images)
        }));
        candidate = next;
        let loaded = meta.timed("store_load", iteration, || store.load().unwrap());
        assert_eq!(loaded.configuration, *candidate.configuration());
    }
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened.load().unwrap().configuration,
        *candidate.configuration()
    );
    for (key, image) in keys.iter().zip(&images) {
        assert_eq!(&reopened.image(key).unwrap(), image);
    }
}

fn output_root() -> (PathBuf, bool) {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(
        args.next()
            .expect("one new target/performance directory required"),
    );
    let representative = match args.next() {
        None => false,
        Some(flag) if flag == "--representative" => true,
        Some(_) => panic!("only optional --representative is accepted"),
    };
    assert!(args.next().is_none(), "unexpected extra argument");
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/performance")
        .canonicalize()
        .unwrap();
    let parent = path
        .parent()
        .expect("directory needs a parent")
        .canonicalize()
        .unwrap();
    assert!(
        parent.starts_with(&base),
        "output must be under target/performance"
    );
    let path = parent.join(path.file_name().expect("output needs a directory name"));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    (path, representative)
}
fn main() {
    let (root, representative) = output_root();
    println!(
        "{}",
        json!({"kind":"metadata","suite":if representative {"representative"} else {"historical"},
        "path":"Authority + real Store public APIs","clock":"Instant wall monotonic",
        "limits":["synthetic profiles, not a census of user configurations","source PNG uses stored DEFLATE; normalized PNG uses real compression","low pixel entropy does not guarantee small canonical Fast/NoFilter PNG","independent operation samples, not additive percentiles"]})
    );
    for count in if representative {
        [32, 256, 1000]
    } else {
        [32, 1000, 7000]
    } {
        domain(&root, count);
    }
    if representative {
        for (entities, count, side, shared, pattern) in [
            (32, 1, 64, true, Pattern::Flat),
            (256, 4, 64, true, Pattern::Bands),
            (256, 4, 64, false, Pattern::Bands),
            (1000, 16, 128, true, Pattern::Bands),
            (1000, 16, 128, false, Pattern::Bands),
        ] {
            media(&root, entities, count, side, shared, pattern);
        }
    } else {
        for (count, side, shared) in [
            (1, 256, true),
            (16, 256, true),
            (16, 256, false),
            (128, 64, false),
            (128, 256, false),
        ] {
            media(&root, 7000, count, side, shared, Pattern::Noise);
        }
    }
    println!("{}", json!({"kind":"complete","durable_reopen":true}));
}
