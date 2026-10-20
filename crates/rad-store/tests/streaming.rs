use rad_core::{
    configuration::{Appearance, Candidate, Configuration, Edit, Origin},
    identity::IdentityRegistry,
};
use rad_store::{CommitOutcome, MediaEntry, MediaReadError, MediaSource, Store, StoreError};
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
fn path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "rad-stream-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
#[test]
fn streaming_reads_twice_and_commits_same_tuple_as_slice() {
    let p = path();
    let base = initial();
    let images = [image(1)];
    let candidate = with_images(&base, &images);
    let mut s = Store::create(&p, &base).unwrap();
    let mut source = Source::new(&images);
    assert_eq!(
        s.commit_from_media(&candidate, &mut source),
        CommitOutcome::Applied {
            revision: 2,
            changed: true
        }
    );
    assert_eq!(source.calls, 2);
    assert_eq!(s.load().unwrap().configuration, *candidate.configuration());
    assert_eq!(
        s.image(&rad_store::media_key(&images[0])).unwrap(),
        images[0]
    );
    assert_eq!(
        s.commit_with_media(&candidate, &images),
        CommitOutcome::Applied {
            revision: 2,
            changed: false
        }
    );
    drop(s);
    std::fs::remove_file(p).unwrap();
}
struct Changing(Source);
impl MediaSource for Changing {
    fn catalog(&self) -> &[MediaEntry] {
        self.0.catalog()
    }
    fn read(&mut self, key: &[u8; 32]) -> Result<rad_media::NormalizedImage, MediaReadError> {
        let image = self.0.read(key)?;
        if self.0.calls == 1 {
            self.0.catalog[0].length += 1;
        }
        Ok(image)
    }
}
#[test]
fn changed_catalog_during_reads_rejects_whole_candidate() {
    let p = path();
    let base = initial();
    let images = [image(2)];
    let candidate = with_images(&base, &images);
    let mut s = Store::create(&p, &base).unwrap();
    assert_eq!(
        s.commit_from_media(&candidate, &mut Changing(Source::new(&images))),
        CommitOutcome::NotApplied(StoreError::Invalid)
    );
    assert_eq!(s.load().unwrap().revision, 1);
    assert_eq!(s.load().unwrap().registry.records().count(), 1);
    drop(s);
    std::fs::remove_file(p).unwrap();
}
struct Failing {
    source: Source,
    at: usize,
    error: MediaReadError,
    wrong_bytes: bool,
}
impl MediaSource for Failing {
    fn catalog(&self) -> &[MediaEntry] {
        self.source.catalog()
    }
    fn read(&mut self, key: &[u8; 32]) -> Result<rad_media::NormalizedImage, MediaReadError> {
        if self.source.calls + 1 == self.at {
            self.source.calls += 1;
            return if self.wrong_bytes {
                Ok(image(250))
            } else {
                Err(self.error)
            };
        }
        self.source.read(key)
    }
}
#[test]
fn both_passes_fail_at_every_read_without_partial_tuple_and_io_reconciles() {
    for at in 1..=4 {
        for (error, wrong_bytes) in [
            (MediaReadError::Invalid, false),
            (MediaReadError::Io, false),
            (MediaReadError::Invalid, true),
        ] {
            let p = path();
            let base = initial();
            let images = [image(3), image(4)];
            let candidate = with_images(&base, &images);
            let mut s = Store::create(&p, &base).unwrap();
            let mut source = Failing {
                source: Source::new(&images),
                at,
                error,
                wrong_bytes,
            };
            let result = s.commit_from_media(&candidate, &mut source);
            if error == MediaReadError::Io && !wrong_bytes {
                assert!(matches!(result, CommitOutcome::Uncertain { .. }));
                assert!(s.load().is_err());
                assert_eq!(s.reconcile(), CommitOutcome::NotApplied(StoreError::Io));
            } else {
                assert!(matches!(result, CommitOutcome::NotApplied(_)));
            }
            assert_eq!(source.source.calls, at);
            assert_eq!(s.load().unwrap().revision, 1);
            let db = rusqlite::Connection::open(&p).unwrap();
            assert_eq!(
                db.query_row::<i64, _, _>("SELECT count(*) FROM media", [], |r| r.get(0))
                    .unwrap(),
                0
            );
            assert_eq!(s.load().unwrap().registry.records().count(), 1);
            drop(db);
            drop(s);
            std::fs::remove_file(p).unwrap();
        }
    }
}
#[test]
fn create_streaming_and_slice_duplicates_share_the_validated_pipeline() {
    let p = path();
    let base = initial();
    let images = [image(8)];
    let candidate = with_images(&base, &images);
    let mut source = Source::new(&images);
    let mut s = Store::create_from_media(&p, &candidate, &mut source).unwrap();
    assert_eq!(source.calls, 2);
    assert_eq!(s.load().unwrap().configuration, *candidate.configuration());
    assert_eq!(
        s.commit_with_media(&candidate, &[images[0].clone(), images[0].clone()]),
        CommitOutcome::Applied {
            revision: 1,
            changed: false
        }
    );
    let mut duplicate = Source::new(&[images[0].clone(), images[0].clone()]);
    assert_eq!(
        s.commit_from_media(&candidate, &mut duplicate),
        CommitOutcome::NotApplied(StoreError::Invalid)
    );
    assert_eq!(duplicate.calls, 0);
    drop(s);
    std::fs::remove_file(p).unwrap();
}
#[test]
fn catalog_bounds_and_missing_refs_reject_before_reading_any_source() {
    let p = path();
    let base = initial();
    let images = [image(9)];
    let candidate = with_images(&base, &images);
    let mut s = Store::create(&p, &base).unwrap();
    for (count, length) in [(513, 1), (1, 0), (1, 524289)] {
        let mut source = Source::new(&images);
        source.catalog = vec![
            MediaEntry {
                digest: source.catalog[0].digest,
                length
            };
            count
        ];
        assert!(matches!(
            s.commit_from_media(&candidate, &mut source),
            CommitOutcome::NotApplied(_)
        ));
        assert_eq!(source.calls, 0);
    }
    let mut empty = Source::new(&[]);
    assert_eq!(
        s.commit_from_media(&candidate, &mut empty),
        CommitOutcome::NotApplied(StoreError::Invalid)
    );
    assert_eq!(s.load().unwrap().revision, 1);
    drop(s);
    std::fs::remove_file(p).unwrap();
}
#[test]
fn initial_streaming_io_failure_preserves_incomplete_material_without_claiming_clean_failure() {
    let p = path();
    let base = initial();
    let images = [image(15), image(16)];
    let candidate = with_images(&base, &images);
    let mut source = Failing {
        source: Source::new(&images),
        at: 4,
        error: MediaReadError::Io,
        wrong_bytes: false,
    };
    assert!(matches!(
        Store::create_from_media(&p, &candidate, &mut source),
        Err(StoreError::Uncertain)
    ));
    assert!(p.exists());
    assert!(Store::open(&p).is_err());
    let db = rusqlite::Connection::open(&p).unwrap();
    assert_eq!(
        db.query_row::<i64, _, _>("SELECT count(*) FROM heads", [], |r| r.get(0))
            .unwrap(),
        0
    );
    drop(db);
    std::fs::remove_file(p).unwrap();
}
