use super::*;
use rad_store::{MediaEntry, MediaReadError, MediaSource};
use std::collections::BTreeSet;
struct Source<'a> {
    store: &'a Store,
    entries: Vec<MediaEntry>,
}
fn key(d: &[u8; 32]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}
impl MediaSource for Source<'_> {
    fn catalog(&self) -> &[MediaEntry] {
        &self.entries
    }
    fn read(&mut self, d: &[u8; 32]) -> Result<rad_media::NormalizedImage, MediaReadError> {
        self.store.image(&key(d)).map_err(|_| MediaReadError::Io)
    }
}
pub(super) fn write(store: &Store, revision: i64, out: &mut impl Write) -> Result<u64, Error> {
    let loaded = store.revision(revision).map_err(|_| Error::NotFound)?;
    let refs: BTreeSet<_> = loaded
        .configuration
        .data()
        .buttons
        .iter()
        .filter_map(|b| b.appearance.image.as_ref())
        .collect();
    let mut entries = Vec::with_capacity(refs.len());
    for key in refs {
        let image = store.image(key).map_err(|_| Error::Io)?;
        let mut digest = [0; 32];
        if key.len() != 64 {
            return Err(Error::Invalid);
        }
        for (i, pair) in key.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            digest[i] =
                u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| Error::Invalid)?, 16)
                    .map_err(|_| Error::Invalid)?;
        }
        entries.push(MediaEntry {
            digest,
            length: image.png_bytes().len() as u32,
        });
    }
    let mut source = Source { store, entries };
    rad_package::export(
        rad_package::ExportSnapshot {
            configuration: &loaded.configuration,
            media: &mut source,
        },
        out,
    )
    .map(|r| r.total_len)
    .map_err(|_| Error::Io)
}
