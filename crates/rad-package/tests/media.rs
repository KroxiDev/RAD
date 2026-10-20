use rad_core::{
    configuration::{Appearance, Candidate, Configuration, Edit, Language, Origin, SettingsData},
    identity::IdentityRegistry,
};
use rad_package::*;
use rad_store::{CommitOutcome, MediaEntry, MediaReadError, MediaSource, Store};
use sha2::{Digest, Sha256};
fn image(value: u8) -> rad_media::NormalizedImage {
    use image::ImageEncoder;
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&[value, 34, 56, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    rad_media::normalize(&bytes).unwrap()
}
fn initial() -> Candidate {
    Configuration::prepare_new(&IdentityRegistry::new(77).unwrap()).unwrap()
}
fn with_images(initial: &Candidate, images: &[rad_media::NormalizedImage]) -> Candidate {
    let mut c = initial.clone();
    for (i, image) in images.iter().enumerate() {
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::CreateButton {
                    folder: c.configuration().data().principal,
                    position: i,
                    name: format!("image{i}"),
                },
            )
            .unwrap();
        let id = c
            .configuration()
            .data()
            .buttons
            .last()
            .unwrap()
            .binding
            .identity();
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::SetAppearance {
                    button: id,
                    appearance: Appearance {
                        text: "".into(),
                        image: Some(rad_store::media_key(image)),
                        text_position: None,
                    },
                },
            )
            .unwrap();
    }
    c
}
struct Source {
    catalog: Vec<MediaEntry>,
    bytes: Vec<Vec<u8>>,
    calls: usize,
}
impl Source {
    fn new(images: &[rad_media::NormalizedImage]) -> Self {
        Self {
            catalog: images
                .iter()
                .map(|i| MediaEntry {
                    digest: Sha256::digest(i.png_bytes()).into(),
                    length: i.png_bytes().len() as u32,
                })
                .collect(),
            bytes: images.iter().map(|i| i.png_bytes().to_vec()).collect(),
            calls: 0,
        }
    }
}
impl MediaSource for Source {
    fn catalog(&self) -> &[MediaEntry] {
        &self.catalog
    }
    fn read(&mut self, key: &[u8; 32]) -> Result<rad_media::NormalizedImage, MediaReadError> {
        self.calls += 1;
        let i = self
            .catalog
            .iter()
            .position(|e| &e.digest == key)
            .ok_or(MediaReadError::Invalid)?;
        rad_media::NormalizedImage::validate(&self.bytes[i]).map_err(|_| MediaReadError::Invalid)
    }
}
fn area() -> (std::path::PathBuf, StagingArea) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "rad-package-media-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let a = StagingArea::create_new(&p).unwrap();
    (p, a)
}
fn bytes(c: &Candidate, images: &[rad_media::NormalizedImage]) -> Vec<u8> {
    let mut b = Vec::new();
    export(
        ExportSnapshot {
            configuration: c.configuration(),
            media: &mut Source::new(images),
        },
        &mut b,
    )
    .unwrap();
    b
}
#[test]
fn full_settings_shared_media_roundtrip_into_real_store() {
    let base = initial();
    let images = [image(1), image(2)];
    let mut c = with_images(&base, &images);
    c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetSettings(SettingsData {
                brightness_percent: 17,
                sleep_ms: u64::MAX,
                press_threshold_ms: u64::MAX,
                gui_language: Language::Es,
            }),
        )
        .unwrap();
    c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::DuplicateButton {
                button: c.configuration().data().buttons[0].binding.identity(),
                folder: c.configuration().data().principal,
                position: 2,
            },
        )
        .unwrap();
    let b = bytes(&c, &images);
    assert_eq!(u16::from_be_bytes(b[24..26].try_into().unwrap()), 2);
    let (root, area) = area();
    let db = root.join("store.db");
    let mut store = Store::create(&db, &base).unwrap();
    let prepared = prepare(&mut b.as_slice(), &area).unwrap();
    assert_eq!(prepared.summary().media_count, 2);
    assert_eq!(store.load().unwrap().configuration, *base.configuration());
    let (candidate, mut media) = prepared
        .confirm(base.configuration(), base.registry(), Origin::Editor, true)
        .unwrap();
    assert_eq!(
        store.commit_from_media(&candidate, &mut media),
        CommitOutcome::Applied {
            revision: 2,
            changed: true
        }
    );
    assert_eq!(store.load().unwrap().configuration, *c.configuration());
    drop(media);
    drop(store);
    let reopened = Store::open(&db).unwrap();
    assert_eq!(reopened.load().unwrap().configuration, *c.configuration());
    drop(reopened);
    std::fs::remove_file(db).unwrap();
    assert_eq!(area.usage().unwrap(), (0, 0));
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn staged_media_changes_after_confirmation_cannot_partially_publish() {
    use std::io::{Seek, SeekFrom, Write};
    let base = initial();
    let images = [image(1), image(2)];
    let c = with_images(&base, &images);
    let b = bytes(&c, &images);
    let (root, area) = area();
    let db = root.join("store.db");
    let mut store = Store::create(&db, &base).unwrap();
    let p = prepare(&mut b.as_slice(), &area).unwrap();
    let (candidate, mut media) = p
        .confirm(base.configuration(), base.registry(), Origin::Editor, true)
        .unwrap();
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(root.join("transfer-0000000000000000"))
        .unwrap();
    let offset = 60 + u32::from_be_bytes(b[20..24].try_into().unwrap()) as u64 + 36;
    f.seek(SeekFrom::Start(offset)).unwrap();
    f.write_all(b"bad").unwrap();
    drop(f);
    assert!(matches!(
        store.commit_from_media(&candidate, &mut media),
        CommitOutcome::NotApplied(_)
    ));
    assert_eq!(store.load().unwrap().configuration, *base.configuration());
    drop(media);
    drop(store);
    std::fs::remove_file(db).unwrap();
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn export_catalog_and_output_failures_never_return_receipt() {
    let base = initial();
    let images = [image(1)];
    let c = with_images(&base, &images);
    let mut missing = Source::new(&[]);
    assert_eq!(
        export(
            ExportSnapshot {
                configuration: c.configuration(),
                media: &mut missing
            },
            &mut Vec::new()
        ),
        Err(PackageError::InvalidPackage)
    );
    let mut extra = Source::new(&images);
    assert_eq!(
        export(
            ExportSnapshot {
                configuration: base.configuration(),
                media: &mut extra
            },
            &mut Vec::new()
        ),
        Err(PackageError::InvalidPackage)
    );
    let mut duplicate = Source::new(&images);
    duplicate.catalog.push(duplicate.catalog[0]);
    assert_eq!(
        export(
            ExportSnapshot {
                configuration: c.configuration(),
                media: &mut duplicate
            },
            &mut Vec::new()
        ),
        Err(PackageError::InvalidPackage)
    );
    struct Fails(usize);
    impl std::io::Write for Fails {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if self.0 == 0 {
                return Err(std::io::ErrorKind::StorageFull.into());
            }
            let n = b.len().min(self.0).min(7);
            self.0 -= n;
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let complete = bytes(&c, &images);
    for n in [0, 1, 60, complete.len() - 1] {
        assert_eq!(
            export(
                ExportSnapshot {
                    configuration: c.configuration(),
                    media: &mut Source::new(&images)
                },
                &mut Fails(n)
            ),
            Err(PackageError::Io)
        );
    }
}
fn rehash(b: &mut [u8]) {
    let n = b.len();
    let digest = Sha256::digest(&b[..n - 32]);
    b[n - 32..].copy_from_slice(&digest);
}
#[test]
fn corrupt_media_framing_checksums_duplicates_order_and_exact_references_reject() {
    let images = [image(1), image(2)];
    let c = with_images(&initial(), &images);
    let original = bytes(&c, &images);
    let start = 60 + u32::from_be_bytes(original[20..24].try_into().unwrap()) as usize;
    let first_len =
        u32::from_be_bytes(original[start + 32..start + 36].try_into().unwrap()) as usize;
    let second = start + 36 + first_len;
    let (root, area) = area();
    for end in [
        start,
        start + 31,
        start + 35,
        start + 36,
        second - 1,
        second,
        original.len() - 33,
        original.len() - 1,
    ] {
        assert!(prepare(&mut &original[..end], &area).is_err());
    }
    for offset in [
        start,
        start + 32,
        start + 35,
        start + 36,
        second - 1,
        second,
        second + 36,
    ] {
        let mut b = original.clone();
        b[offset] ^= 1;
        rehash(&mut b);
        assert!(prepare(&mut b.as_slice(), &area).is_err(), "offset{offset}");
    }
    let mut duplicate = original.clone();
    duplicate[second..second + 32].copy_from_slice(&original[start..start + 32]);
    rehash(&mut duplicate);
    assert!(prepare(&mut duplicate.as_slice(), &area).is_err());
    let mut unordered = original[..start].to_vec();
    unordered.extend_from_slice(&original[second..original.len() - 32]);
    unordered.extend_from_slice(&original[start..second]);
    unordered.extend_from_slice(&[0; 32]);
    rehash(&mut unordered);
    assert!(prepare(&mut unordered.as_slice(), &area).is_err());
    for len in [0, 512 * 1024 + 1, u32::MAX] {
        let mut b = original.clone();
        b[start + 32..start + 36].copy_from_slice(&len.to_be_bytes());
        rehash(&mut b);
        assert!(matches!(
            prepare(&mut b.as_slice(), &area),
            Err(PackageError::LimitExceeded)
        ));
    }
    assert_eq!(area.usage().unwrap(), (0, 0));
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn valid_digest_of_noncanonical_png_still_rejects() {
    // Add a valid ancillary text chunk to a normalized PNG, updating both package digests.
    // CRC32 is computed here, independently of rad-media's canonical validator.
    fn crc(b: &[u8]) -> u32 {
        let mut c = !0u32;
        for v in b {
            c ^= *v as u32;
            for _ in 0..8 {
                c = (c >> 1) ^ if c & 1 != 0 { 0xedb88320 } else { 0 };
            }
        }
        !c
    }
    let image = image(1);
    let mut png = image.png_bytes().to_vec();
    let at = png.len() - 12;
    let chunk = b"tEXtcomment\0not canonical";
    let mut encoded = ((chunk.len() - 4) as u32).to_be_bytes().to_vec();
    encoded.extend_from_slice(chunk);
    encoded.extend_from_slice(&crc(chunk).to_be_bytes());
    png.splice(at..at, encoded);
    let mut c = with_images(&initial(), &[image]);
    let key: String = Sha256::digest(&png)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::SetAppearance {
                button: c.configuration().data().buttons[0].binding.identity(),
                appearance: Appearance {
                    text: "".into(),
                    image: Some(key),
                    text_position: None,
                },
            },
        )
        .unwrap();
    let doc = rad_codec::encode(c.configuration()).unwrap();
    let mut b = b"RADPKG\r\n".to_vec();
    b.extend_from_slice(&[0, 1, 0, 0]);
    b.extend_from_slice(&(92 + doc.len() as u64 + 36 + png.len() as u64).to_be_bytes());
    b.extend_from_slice(&(doc.len() as u32).to_be_bytes());
    b.extend_from_slice(&[0, 1, 0, 0]);
    b.extend_from_slice(&Sha256::digest(&doc));
    b.extend(doc);
    b.extend_from_slice(&Sha256::digest(&png));
    b.extend_from_slice(&(png.len() as u32).to_be_bytes());
    b.extend(png);
    b.extend_from_slice(&[0; 32]);
    rehash(&mut b);
    let (root, area) = area();
    assert!(matches!(
        prepare(&mut b.as_slice(), &area),
        Err(PackageError::InvalidPackage)
    ));
    assert_eq!(area.usage().unwrap(), (0, 0));
    std::fs::remove_dir(root).unwrap();
}
#[test]
fn source_errors_mutation_and_catalog_caps_reject_without_success() {
    let images = [image(1)];
    let c = with_images(&initial(), &images);
    struct Failing {
        inner: Source,
        error: Option<MediaReadError>,
        mutate: bool,
    }
    impl MediaSource for Failing {
        fn catalog(&self) -> &[MediaEntry] {
            self.inner.catalog()
        }
        fn read(&mut self, k: &[u8; 32]) -> Result<rad_media::NormalizedImage, MediaReadError> {
            if let Some(e) = self.error {
                return Err(e);
            }
            let value = self.inner.read(k)?;
            if self.mutate {
                self.inner.catalog.clear();
            }
            Ok(value)
        }
    }
    for (error, mutate, expected) in [
        (Some(MediaReadError::Io), false, PackageError::Io),
        (
            Some(MediaReadError::Invalid),
            false,
            PackageError::InvalidPackage,
        ),
        (None, true, PackageError::InvalidPackage),
    ] {
        let mut source = Failing {
            inner: Source::new(&images),
            error,
            mutate,
        };
        assert_eq!(
            export(
                ExportSnapshot {
                    configuration: c.configuration(),
                    media: &mut source
                },
                &mut Vec::new()
            ),
            Err(expected)
        );
    }
    for (count, length) in [(513, 1), (1, 512 * 1024 + 1), (512, 262145)] {
        let mut source = Source::new(&images);
        source.catalog = (0..count)
            .map(|i| {
                let mut digest = [0; 32];
                digest[..8].copy_from_slice(&(i as u64).to_be_bytes());
                MediaEntry { digest, length }
            })
            .collect();
        assert_eq!(
            export(
                ExportSnapshot {
                    configuration: c.configuration(),
                    media: &mut source
                },
                &mut Vec::new()
            ),
            Err(PackageError::LimitExceeded)
        );
        assert_eq!(source.calls, 0);
    }
}
#[test]
fn inclusive_catalog_caps_are_not_confused_with_missing_references() {
    let base = initial();
    for extra in [0, 1] {
        let mut source = Source::new(&[]);
        source.catalog = (0..512u64)
            .map(|i| {
                let mut digest = [0; 32];
                digest[..8].copy_from_slice(&i.to_be_bytes());
                MediaEntry {
                    digest,
                    length: 262144 + if i == 511 { extra } else { 0 },
                }
            })
            .collect();
        // This synthetic catalogue probes arithmetic only, not fictitious valid PNG payloads.
        let expected = if extra == 0 {
            PackageError::InvalidPackage
        } else {
            PackageError::LimitExceeded
        };
        assert_eq!(
            export(
                ExportSnapshot {
                    configuration: base.configuration(),
                    media: &mut source
                },
                &mut Vec::new()
            ),
            Err(expected)
        );
        assert_eq!(source.calls, 0);
    }
}
#[test]
fn last_second_pass_failure_rolls_back_and_uncertain_retains_prepared_ownership() {
    use std::io::{Seek, SeekFrom, Write};
    struct Late {
        media: PreparedMedia,
        path: std::path::PathBuf,
        offset: u64,
        calls: usize,
        io: bool,
    }
    impl MediaSource for Late {
        fn catalog(&self) -> &[MediaEntry] {
            self.media.catalog()
        }
        fn read(&mut self, key: &[u8; 32]) -> Result<rad_media::NormalizedImage, MediaReadError> {
            self.calls += 1;
            if self.calls == 4 {
                if self.io {
                    return Err(MediaReadError::Io);
                }
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&self.path)
                    .unwrap();
                file.seek(SeekFrom::Start(self.offset)).unwrap();
                file.write_all(b"bad").unwrap();
            }
            self.media.read(key)
        }
    }
    for io in [false, true] {
        let base = initial();
        let images = [image(1), image(2)];
        let c = with_images(&base, &images);
        let b = bytes(&c, &images);
        let start = 60 + u32::from_be_bytes(b[20..24].try_into().unwrap()) as usize;
        let first = u32::from_be_bytes(b[start + 32..start + 36].try_into().unwrap()) as usize;
        let offset = (start + 36 + first + 36) as u64;
        let (root, area) = area();
        let db = root.join("store.db");
        let mut store = Store::create(&db, &base).unwrap();
        let (candidate, media) = prepare(&mut b.as_slice(), &area)
            .unwrap()
            .confirm(base.configuration(), base.registry(), Origin::Editor, true)
            .unwrap();
        let mut source = Late {
            media,
            path: root.join("transfer-0000000000000000"),
            offset,
            calls: 0,
            io,
        };
        let result = store.commit_from_media(&candidate, &mut source);
        assert_eq!(source.calls, 4);
        if io {
            assert!(matches!(result, CommitOutcome::Uncertain { .. }));
            assert_eq!(area.usage().unwrap().0, 1);
            assert!(store.load().is_err());
            assert_eq!(
                store.reconcile(),
                CommitOutcome::NotApplied(rad_store::StoreError::Io)
            );
            assert_eq!(source.calls, 4);
        } else {
            assert!(matches!(result, CommitOutcome::NotApplied(_)));
        }
        assert_eq!(store.load().unwrap().configuration, *base.configuration());
        assert_eq!(store.load().unwrap().registry, *base.registry());
        assert_eq!(area.usage().unwrap().0, 1);
        drop(source);
        assert_eq!(area.usage().unwrap(), (0, 0));
        drop(store);
        std::fs::remove_file(db).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
